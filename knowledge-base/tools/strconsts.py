#!/usr/bin/env python3
"""
Recover the GML string-constant pool.

YYC emits each GML string literal as a lazily-constructed static: a tiny
function does `ctor(storage, "literal")` once, and every use of the constant
then refers to `storage`. Recovering storage -> literal turns those otherwise
opaque data references into readable strings, which is what makes functions
like reward_library legible.

  python strconsts.py [--grep SUBSTR]
"""
import os
import pickle
import sys

import capstone

import tkiw
import xrefs

CACHE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "strconsts.pickle")
# the string-constant constructor, located from a known literal's initialiser
CTOR_HINT_STRING = b"unit_class_stat\0"


def find_ctor(pe):
    """The constructor address, via the initialiser of a known literal."""
    i = pe.d.find(CTOR_HINT_STRING)
    while i > 0 and pe.d[i - 1] != 0:
        i = pe.d.find(CTOR_HINT_STRING, i + 1)
    if i < 0:
        return None
    rva = pe.off2rva(i)
    found, fidx = xrefs.xref(pe, [rva])
    hits = found.get(rva) or []
    if not hits:
        return None
    _, fstart, _ = hits[0]
    f = fidx.enclosing(fstart)
    md = tkiw.make_disassembler()
    for insn in tkiw.disasm_function(pe, md, f[0], f[1]):
        if insn.mnemonic == "call" and insn.op_str.startswith("0x"):
            return pe.va2rva(int(insn.op_str, 16))
    return None


def build(pe, verbose=True):
    ctor = find_ctor(pe)
    if ctor is None:
        sys.exit("error: could not locate the string constructor")
    if verbose:
        print(f"string ctor at {ctor:#x}", file=sys.stderr)

    # call rel32 ends with its disp32, so the same arithmetic sweep finds them
    cands = xrefs.candidate_offsets(pe, [ctor])[ctor]
    fidx = tkiw.FunctionIndex(pe)
    md = tkiw.make_disassembler()

    funcs = set()
    for o in cands:
        rva = pe.off2rva(o)
        if rva is None:
            continue
        f = fidx.enclosing(rva)
        if f:
            funcs.add(f)
    if verbose:
        print(f"{len(funcs)} candidate initialiser functions", file=sys.stderr)

    out = {}
    for start, end in funcs:
        last = {}
        for insn in tkiw.disasm_function(pe, md, start, end):
            if insn.mnemonic == "lea" and len(insn.operands) == 2:
                dst = insn.operands[0]
                tgts = tkiw.rip_targets(pe, insn)
                if tgts and dst.type == capstone.x86.X86_OP_REG:
                    last[insn.reg_name(dst.reg)] = tgts[0]
            elif insn.mnemonic == "call" and insn.op_str.startswith("0x"):
                if pe.va2rva(int(insn.op_str, 16)) != ctor:
                    continue
                storage, lit = last.get("rcx"), last.get("rdx")
                if storage is None or lit is None:
                    continue
                s = pe.is_printable_cstr(lit, 400)
                if s is not None:
                    out[storage] = s
                last = {}
    return out


def load(pe=None, refresh=False):
    if not refresh and os.path.exists(CACHE):
        with open(CACHE, "rb") as fh:
            return pickle.load(fh)
    pe = pe or tkiw.load()
    tbl = build(pe)
    with open(CACHE, "wb") as fh:
        pickle.dump(tbl, fh, protocol=4)
    return tbl


def main():
    pe = tkiw.load()
    tbl = load(pe, refresh="--refresh" in sys.argv)
    print(f"{len(tbl)} string constants")
    if "--grep" in sys.argv:
        needle = sys.argv[sys.argv.index("--grep") + 1].lower()
        for rva, s in sorted(tbl.items(), key=lambda kv: kv[1]):
            if needle in s.lower():
                print(f"  {rva:#010x}  {s!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
