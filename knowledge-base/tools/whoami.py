#!/usr/bin/env python3
"""Identify raw addresses from a profile: enclosing function, nearest builtin,
strings it references. Usage: python whoami.py 0x1c9fd30 0x1ca3ab0 ..."""
import bisect
import pickle
import sys

TOOLS = __file__.rsplit("\\", 1)[0] if "\\" in __file__ else "."
sys.path.insert(0, TOOLS)
import tkiw

pe = tkiw.load()
tbl = pickle.load(open(TOOLS + "/builtins.pickle", "rb"))
byrva = {}
for n, (r, a) in tbl.items():
    if r:
        byrva.setdefault(r, n)
keys = sorted(byrva)
syms = tkiw.gml_function_table(pe)
by_start = {r: n for n, r in syms.items()}
fidx = tkiw.FunctionIndex(pe, symbols=syms)
md = tkiw.make_disassembler()

probes = [int(a, 16) for a in sys.argv[1:]]
for p in probes:
    f = fidx.enclosing(p)
    span = f"{f[0]:#x}..{f[1]:#x}" if f else "no .pdata entry"
    i = bisect.bisect_right(keys, p) - 1
    near = f"{byrva[keys[i]]}@{keys[i]:#x}" if i >= 0 else "-"
    exact = byrva.get(p) or (byrva.get(f[0]) if f else None)
    gml = by_start.get(p) or (by_start.get(f[0]) if f else None)
    print(f"\n=== {p:#x}")
    print(f"    function      : {span}")
    if gml:
        print(f"    GML symbol    : {gml}")
    if exact:
        print(f"    builtin       : {exact}")
    print(f"    nearest builtin at or below: {near}")
    if f:
        strs, calls = [], []
        for insn in tkiw.disasm_function(pe, md, f[0], min(f[1], f[0] + 0x900)):
            for t in tkiw.rip_targets(pe, insn):
                s = pe.is_printable_cstr(t, 100)
                if s and len(s) > 3:
                    strs.append(s)
            if insn.mnemonic in ("call", "jmp") and insn.op_str.startswith("0x"):
                r = pe.va2rva(int(insn.op_str, 16))
                nm = by_start.get(r) or byrva.get(r)
                if nm:
                    calls.append(nm)
        if strs:
            print(f"    strings       : {sorted(set(strs))[:6]}")
        if calls:
            print(f"    named callees : {sorted(set(calls))[:8]}")
