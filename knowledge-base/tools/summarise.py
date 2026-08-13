#!/usr/bin/env python3
"""
What a compiled GML function *does*, in about twenty lines instead of two
thousand.

`gmldis.py` gives you every instruction. That is the right tool once you know
which instruction you care about, and the wrong one for "what is this function
for" -- YYC emits fifteen instructions of RValue housekeeping per line of GML,
and the shape of the function is buried in them.

This prints only the calls, in order, each annotated with the variables read
and string constants seen since the previous call. That reads roughly like the
original GML's control flow.

  python summarise.py <symbol>              the calls, with their context
  python summarise.py <symbol> --callers    who calls it, first
  python summarise.py <symbol> --full       also every annotated instruction
  python summarise.py <symbol> --noise      keep the RValue housekeeping

A partial symbol name is accepted when it matches exactly one function.

## Reading the output

`*qword ptr [rax + 8]` and `[rax + 0x10]` are the variable getter and
get-for-write from the global/instance/struct vtable -- so a line
`*qword ptr [rax + 8]  [var:pending_rewards]` is a read of that variable.

Call targets named `sub_NNNN` are unnamed runtime routines. The frequent ones
are listed in NOISE below or in RUNTIME; anything else is worth identifying by
its error strings (see `runtime-internals.md`).

**Builtin calls do not appear here as names, and cannot.** Compiled GML never
calls the `Function_Add`-registered builtins -- see `runtime-internals.md`.
"""
import os
import pickle
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import tkiw
from gmldis import Annotator
from index import Index

# RValue housekeeping, present in every function, says nothing about what the
# function does. Hidden unless --noise.
NOISE = {
    0x8f580,    # RValue release
    0x8f6b0,    # RValue copy
    0x8f4e0,    # RValue assign
    0x8fe20,    # RValue concat helper
    0x1e9ff30,  # GML stack frame push
    0x1e9fc10,  # GML stack frame pop
    0x1e9fb00,  # once-guard for a static initialiser
    0x1e9ff18, 0x1e9faa0,
    0x1aa4280,  # runtime error
    0x1aa46c0,  # RValue from a C string literal
    0x1aa4090,  # GML string constructor
}

# Unnamed runtime routines identified by their error strings or by which
# builtin wrapper reaches them. Extend this as you identify more; it is the
# single highest-leverage thing you can do for readability.
RUNTIME = {
    0x1a8c880: "YYGetReal",         # "REAL argument incorrect type %s"
    0x1a8bf50: "YYGetInt32",        # "I32 argument incorrect type %d"
    0x1a8c0d0: "YYGetInt64",
    0x1a8a940: "YYGetBool",
    0x1a8ee40: "YYGetRef",          # unpacks a kind-15 ref, checks its type
    0x1ac46e0: "member_get",        # (struct_or_inst, var_id) -> RValue*
    0x1aa47f0: "method_invoke",     # see calling-into-the-game.md
    0x1aa4c90: "static_get",
    0x1b0d3a0: "ds_get",            # "Data structure with index does not exist."
    0x1af1390: "to_string",
    0x1aa0df0: "Object_Find",
    0x1b31600: "Object_FindIndexByName",
}


def builtin_names():
    """rva -> builtin name, for the callable wrappers. Rarely hits in compiled
    GML (they are not called from there) but does hit when reading the runtime
    itself."""
    cache = os.path.join(HERE, "builtins.pickle")
    if not os.path.isfile(cache):
        return {}
    with open(cache, "rb") as fh:
        table = pickle.load(fh)
    out = {}
    for name, (rva, _argc) in table.items():
        if rva:
            out.setdefault(rva, name)
    return out


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    if not args:
        print(__doc__)
        return 1

    pe = tkiw.load()
    ann = Annotator(pe)
    api = builtin_names()

    name = args[0]
    if name not in ann.syms:
        cands = [n for n in ann.syms if name in n]
        if len(cands) != 1:
            print(f"{len(cands)} symbols match {name!r}"
                  f"{':' if cands else '; try gmldis.py --grep'}")
            for c in sorted(cands)[:40]:
                print("  ", c)
            return 1
        name = cands[0]

    if "--callers" in flags:
        ix = Index.load()
        callers = sorted(ix.callers.get(name, ()))
        print(f"; {len(callers)} caller(s) of {name}:")
        for c in callers:
            print("   ", c)
        print()

    def label_for(rva):
        n = ann.call_name(rva)
        if not n.startswith(("sub_", "loc_")):
            return n
        return RUNTIME.get(rva) or api.get(rva) or n

    start, end = ann.function_range(name)
    print(f"; {name}   rva {start:#x}..{end:#x}  ({end - start} bytes)")
    pending = []
    for insn in tkiw.disasm_function(pe, ann.md, start, end):
        notes = [a for a in (ann.annotate_target(t) for t in tkiw.rip_targets(pe, insn))
                 if a and not a.startswith("data:")]
        pending.extend(notes)
        if insn.mnemonic != "call":
            if "--full" in flags and notes:
                rva = pe.va2rva(insn.address)
                print(f"{rva:08x}  {insn.mnemonic:<7} {insn.op_str:<32} ; {'  '.join(notes)}")
            continue
        rva = pe.va2rva(insn.address)
        if insn.op_str.startswith("0x"):
            target = pe.va2rva(int(insn.op_str, 16))
            if pe.in_section(target, ".text"):
                if target in NOISE and "--noise" not in flags:
                    continue
                label = label_for(target)
            else:
                label = f"<{target:#x}>"
        else:
            label = f"*{insn.op_str}"      # indirect: usually a vtable getter
        ctx = ("  [" + ", ".join(dict.fromkeys(pending)) + "]") if pending else ""
        print(f"{rva:08x}  {label}{ctx}")
        pending = []
    if pending:
        print(f"  (after the last call: {', '.join(dict.fromkeys(pending))})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
