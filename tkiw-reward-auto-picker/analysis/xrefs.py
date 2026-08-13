#!/usr/bin/env python3
"""
Find every instruction in .text that references a given address.

The GML symbol table only covers ~12.8k of the ~86k functions in the binary,
so index.py misses references made from runtime helpers and from compiled code
that never got a `gml_*` name. This scans the whole of .text instead.

Two passes:
  1. an arithmetic sweep over the raw bytes, which finds candidate rip-relative
     displacement fields without disassembling anything (a disp32 at file
     offset `o` targets `T` iff  disp32(o) + o == T - V - 4 - tail + R)
  2. capstone verification of only the functions the sweep implicated, which
     drops the false positives the sweep inevitably produces

  python xrefs.py --str "some string"    locate a string, then xref it
  python xrefs.py --rva 0x23a7850
"""
import sys

import numpy as np

import tkiw

# instruction tails after the disp32 field: none, imm8, imm16, imm32
TAILS = (0, 1, 2, 4)


def _disp32_plus_offset(pe, sec):
    """A[i] = signed disp32 at raw offset (raw+i), plus that absolute offset."""
    raw, n = sec["raw"], sec["rawsize"]
    b = np.frombuffer(pe.d, dtype=np.uint8, count=n + 4, offset=raw).astype(np.int64)
    v = (b[0:n] | (b[1:n + 1] << 8) | (b[2:n + 2] << 16) | (b[3:n + 3] << 24))
    v = np.where(v >= 0x80000000, v - 0x100000000, v)      # to signed
    return v + (raw + np.arange(n, dtype=np.int64))


def candidate_offsets(pe, targets, sec_name=".text", _cache={}):
    """{target_rva: [file offsets of candidate disp32 fields]}"""
    sec = pe.section(sec_name)
    key = (id(pe), sec_name)
    if key not in _cache:
        _cache[key] = _disp32_plus_offset(pe, sec)
    A = _cache[key]
    V, R = sec["va"], sec["raw"]
    out = {}
    for T in targets:
        hits = []
        for tail in TAILS:
            C = T - V - 4 - tail + R
            hits.append(np.flatnonzero(A == C))
        out[T] = (np.unique(np.concatenate(hits)) + R).tolist()
    return out


def verify(pe, targets, cands, symbols=None, md=None):
    """Disassemble the implicated functions; keep only real references."""
    md = md or tkiw.make_disassembler()
    fidx = tkiw.FunctionIndex(pe, symbols=symbols or {})
    want = set(targets)

    funcs = set()
    for offs in cands.values():
        for o in offs:
            rva = pe.off2rva(o)
            if rva is None:
                continue
            f = fidx.enclosing(rva)
            if f:
                funcs.add(f)

    found = {t: [] for t in targets}
    for start, end in sorted(funcs):
        for insn in tkiw.disasm_function(pe, md, start, end):
            for t in tkiw.rip_targets(pe, insn):
                if t in want:
                    found[t].append((pe.va2rva(insn.address), start,
                                     f"{insn.mnemonic} {insn.op_str}"))
    return found, fidx


def xref(pe, targets, symbols=None):
    cands = candidate_offsets(pe, targets)
    return verify(pe, targets, cands, symbols)


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return 1
    pe = tkiw.load()
    syms = tkiw.gml_function_table(pe)

    targets = []
    if args[0] == "--str":
        needle = args[1].encode() + b"\0"
        start = 0
        while True:
            i = pe.d.find(needle, start)
            if i < 0:
                break
            start = i + 1
            # only standalone strings: must be preceded by a NUL
            if i and pe.d[i - 1] == 0:
                rva = pe.off2rva(i)
                if rva is not None:
                    targets.append(rva)
        print(f"string {args[1]!r} at rvas: {[hex(t) for t in targets]}")
    elif args[0] == "--rva":
        targets = [int(args[1], 0)]
    else:
        print(__doc__)
        return 1

    if not targets:
        print("not found")
        return 1

    found, fidx = xref(pe, targets, syms)
    for t, hits in found.items():
        print(f"\n== {t:#x}  ({len(hits)} references)")
        for rva, fstart, text in hits:
            print(f"   {rva:08x}  {fidx.describe(rva):<70} {text}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
