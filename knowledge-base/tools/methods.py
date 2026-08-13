#!/usr/bin/env python3
"""
Resolve anonymous GML methods to the variable names they are bound to.

Most methods on an object or struct are anonymous functions assigned to a
member: `spawn_resources_choice = function() {...}`. YYC names the compiled
function `anon@<n>@<parent>` and drops the member name, so the symbol table
alone cannot tell you which anon implements which method.

The binding is recoverable from the parent's own code: the member's
variable-id slot is loaded to address the member, and the `lea` of the
method's own address follows within a short window. Pairing those recovers
`anon@... -> member name`.

  python methods.py <parent-symbol-substring>
"""
import sys
from collections import OrderedDict

import capstone

import tkiw

WINDOW = 24          # instructions to look ahead for the slot load


def resolve(pe, parent, syms=None, slots=None, md=None):
    syms = syms if syms is not None else tkiw.gml_function_table(pe)
    slots = slots if slots is not None else tkiw.gml_variable_slots(pe)
    md = md or tkiw.make_disassembler()
    by_rva = {}
    for n, r in syms.items():
        by_rva.setdefault(r, n)
    idx = tkiw.FunctionIndex(pe, symbols=syms)

    if parent not in syms:
        cands = [n for n in syms if parent in n]
        if len(cands) != 1:
            return None, cands
        parent = cands[0]
    start = syms[parent]
    f = idx.enclosing(start)
    insns = tkiw.disasm_function(pe, md, f[0], f[1])

    # the member slot is loaded first, then the method's address is lea'd
    out, last_slot = OrderedDict(), None
    for i, insn in enumerate(insns):
        tgts = tkiw.rip_targets(pe, insn)
        for t in tgts:
            if t in slots:
                last_slot = (i, slots[t])
        if insn.mnemonic == "lea" and tgts and tgts[0] in by_rva:
            fn = by_rva[tgts[0]]
            if last_slot and i - last_slot[0] <= WINDOW and fn not in out:
                out[fn] = last_slot[1]
    return out, parent


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    pe = tkiw.load()
    out, parent = resolve(pe, sys.argv[1])
    if out is None:
        print("ambiguous parent; candidates:")
        for c in sorted(parent)[:40]:
            print("  ", c)
        return 1
    print(f"; methods of {parent}  ({len(out)} bound)")
    for fn, member in sorted(out.items(), key=lambda kv: kv[1]):
        print(f"  {member:<44} {fn}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
