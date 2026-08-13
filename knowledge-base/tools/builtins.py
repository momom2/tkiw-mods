#!/usr/bin/env python3
"""
Recover the GameMaker *runtime builtin* function table (2,767 entries).

Distinct from `tkiw.gml_function_table()`, which covers compiled GML. The
runtime's own API -- instance_find, variable_instance_get, asset_get_index,
json_stringify, ... -- is registered at startup by a handful of generated
functions that each call

    Function_Add(const char *name, void *fn, int argc, int flags)

with `name` and `fn` in rcx/rdx as rip-relative leas and `argc` as an
immediate. Walking every call site of Function_Add therefore yields
name -> (native rva, argc) for the whole runtime API.

  python builtins.py                 build/refresh the cache, print a summary
  python builtins.py instance        print matching entries

Other scripts do:  import builtins as gmlapi; t = gmlapi.load(pe)
"""
import os
import pickle
import sys

import numpy as np

import tkiw

CACHE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "builtins.pickle")

# Function_Add. Found from the string "instance_find": its single xref lands
# mid-sequence in a long run of identical 5-instruction registration blocks.
FUNCTION_ADD = 0x1B6BCF0


def _call_sites(pe, target, sec_name=".text"):
    """rvas of every `call rel32` in `sec_name` whose target is `target`."""
    sec = pe.section(sec_name)
    raw, n, va = sec["raw"], sec["rawsize"], sec["va"]
    b = np.frombuffer(pe.d, dtype=np.uint8, count=n + 5, offset=raw).astype(np.int64)
    rel = b[1:n + 1] | (b[2:n + 2] << 8) | (b[3:n + 3] << 16) | (b[4:n + 4] << 24)
    rel = np.where(rel >= 0x80000000, rel - 0x100000000, rel)
    nxt = va + np.arange(n, dtype=np.int64) + 5
    return set(int(va + i) for i in np.flatnonzero((b[0:n] == 0xE8) & (nxt + rel == target)))


def build(pe, target=FUNCTION_ADD):
    sites = _call_sites(pe, target)
    fidx = tkiw.FunctionIndex(pe, symbols={})
    funcs = sorted(set(f for f in (fidx.enclosing(r) for r in sites) if f))
    md = tkiw.make_disassembler()

    tbl = {}
    for start, end in funcs:
        name_rva = fn_rva = argc = None
        for insn in tkiw.disasm_function(pe, md, start, end):
            rva, m, ops = pe.va2rva(insn.address), insn.mnemonic, insn.op_str
            if m == "lea":
                tgts = tkiw.rip_targets(pe, insn)
                dst = ops.split(",")[0].strip()
                if tgts:
                    if dst == "rcx":
                        name_rva = tgts[0]
                    elif dst == "rdx":
                        fn_rva = tgts[0]
                elif dst in ("r8d", "r8"):
                    # lea r8d, [r9 + N]  with r9 zeroed just above
                    argc = int(ops.split("+")[1].strip(" ]"), 0) if "+" in ops else 0
            elif m == "mov" and ops.startswith("r8d,"):
                try:
                    argc = int(ops.split(",")[1].strip(), 0)
                except ValueError:
                    pass
            elif m == "xor" and ops.startswith("r8d, r8d"):
                argc = 0
            elif m == "call" and rva in sites:
                nm = pe.is_printable_cstr(name_rva, 80) if name_rva else None
                if nm:
                    tbl[nm] = (fn_rva, argc)
                name_rva = fn_rva = argc = None
    return tbl


def load(pe=None, refresh=False):
    if not refresh and os.path.isfile(CACHE):
        with open(CACHE, "rb") as fh:
            return pickle.load(fh)
    tbl = build(pe or tkiw.load())
    with open(CACHE, "wb") as fh:
        pickle.dump(tbl, fh)
    return tbl


def main():
    pe = tkiw.load()
    tbl = load(pe, refresh=True)
    print(f"{len(tbl)} builtin functions")
    pat = sys.argv[1] if len(sys.argv) > 1 else None
    for name in sorted(tbl):
        if pat and pat not in name:
            continue
        rva, argc = tbl[name]
        if pat:
            print(f"  {rva:08x}  argc={argc if argc is not None else -1:<3} {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
