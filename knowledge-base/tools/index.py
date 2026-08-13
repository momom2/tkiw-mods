#!/usr/bin/env python3
"""
Build (and cache) a cross-reference index over every compiled GML function.

For each named GML function, records the variables it touches, the string
constants it references, and the functions it calls -- plus the inverse maps,
which are what actually answer questions like "who produces the string
'unit_class_stat'" or "who reads run_rerolls_left".

  python index.py            build/refresh the cache, print a summary
  python index.py --stats    just the summary

Other scripts do:  from index import Index; ix = Index.load()
"""
import os
import pickle
import re
import sys
import time
from collections import defaultdict

import tkiw

CACHE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "xref.pickle")
RIP = re.compile(r"\[rip ([+-]) (0x[0-9a-f]+)\]")
CALL = re.compile(r"^0x([0-9a-f]+)$")


class Index:
    def __init__(self):
        self.exe = None
        self.syms = {}          # name -> start rva
        self.by_rva = {}        # start rva -> name
        self.slots = {}         # slot rva -> variable name
        self.func_vars = {}     # name -> set(variable names)
        self.func_strs = {}     # name -> set(strings)
        self.func_calls = {}    # name -> set(callee names)
        self.var_funcs = defaultdict(set)
        self.str_funcs = defaultdict(set)
        self.callers = defaultdict(set)

    # ------------------------------------------------------------- building
    @staticmethod
    def build(pe, verbose=True):
        import capstone
        ix = Index()
        ix.exe = pe.path
        ix.syms = tkiw.gml_function_table(pe)
        ix.slots = tkiw.gml_variable_slots(pe)
        for n, r in ix.syms.items():
            ix.by_rva.setdefault(r, n)
        fidx = tkiw.FunctionIndex(pe, symbols=ix.syms)

        md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        md.detail = False

        t0, done = time.time(), 0
        for name, start in ix.syms.items():
            f = fidx.enclosing(start)
            if not f:
                continue
            end = f[1]
            code = pe.bytes_at(start, min(end - start, 0x40000))
            if not code:
                continue
            vars_, strs, calls = set(), set(), set()
            for insn in md.disasm(code, pe.rva2va(start)):
                op = insn.op_str
                if "rip" in op:
                    for sign, hexoff in RIP.findall(op):
                        disp = int(hexoff, 16) * (1 if sign == "+" else -1)
                        tgt = pe.va2rva(insn.address + insn.size + disp)
                        if tgt in ix.slots:
                            vars_.add(ix.slots[tgt])
                            continue
                        s = _string_at(pe, tgt)
                        if s:
                            strs.add(s)
                elif insn.mnemonic == "call":
                    m = CALL.match(op)
                    if m:
                        tgt = pe.va2rva(int(m.group(1), 16))
                        callee = ix.by_rva.get(tgt)
                        if callee:
                            calls.add(callee)
            ix.func_vars[name] = vars_
            ix.func_strs[name] = strs
            ix.func_calls[name] = calls
            done += 1
            if verbose and done % 2000 == 0:
                print(f"  {done}/{len(ix.syms)}  ({time.time() - t0:.0f}s)",
                      file=sys.stderr)

        for name in ix.syms:
            for v in ix.func_vars.get(name, ()):
                ix.var_funcs[v].add(name)
            for s in ix.func_strs.get(name, ()):
                ix.str_funcs[s].add(name)
            for c in ix.func_calls.get(name, ()):
                ix.callers[c].add(name)
        if verbose:
            print(f"  built in {time.time() - t0:.0f}s", file=sys.stderr)
        return ix

    # -------------------------------------------------------------- caching
    #
    # The cache holds a plain dict of fields, not a pickled `Index`. Pickling
    # the instance records the class as `__main__.Index` when this file is run
    # as a script, and the cache then fails to load from anything that imports
    # it -- which is every other tool. Data in, object out.
    FIELDS = ("exe", "syms", "by_rva", "slots", "func_vars", "func_strs",
              "func_calls", "var_funcs", "str_funcs", "callers")

    def save(self, path=CACHE):
        self.var_funcs = dict(self.var_funcs)
        self.str_funcs = dict(self.str_funcs)
        self.callers = dict(self.callers)
        with open(path, "wb") as fh:
            pickle.dump({f: getattr(self, f) for f in self.FIELDS}, fh, protocol=4)

    @staticmethod
    def load(path=CACHE, exe=None):
        if os.path.exists(path):
            with open(path, "rb") as fh:
                blob = pickle.load(fh)
            if isinstance(blob, Index):        # a cache from an older build
                return blob
            ix = Index()
            for f, v in blob.items():
                setattr(ix, f, v)
            return ix
        pe = tkiw.load(exe)
        ix = Index.build(pe)
        ix.save(path)
        return ix

    # -------------------------------------------------------------- queries
    def funcs_with_string(self, needle, exact=False):
        out = set()
        for s, fs in self.str_funcs.items():
            if (s == needle) if exact else (needle in s):
                out |= fs
        return out

    def funcs_with_var(self, name):
        return set(self.var_funcs.get(name, ()))

    def strings_of(self, func):
        return sorted(self.func_strs.get(func, ()))

    def vars_of(self, func):
        return sorted(self.func_vars.get(func, ()))


def _string_at(pe, rva):
    """A .rdata C string, or a constant RValue holding one."""
    s = pe.is_printable_cstr(rva, 200)
    if s and 1 < len(s) < 160:
        return s
    import struct
    raw = pe.bytes_at(rva, 16)
    if raw and len(raw) == 16:
        ptr, _flags, kind = struct.unpack("<QII", raw)
        if kind == 1 and ptr:
            srva = pe.va2rva(ptr)
            for probe in (srva, srva + 8, srva + 12, srva + 16):
                s2 = pe.is_printable_cstr(probe, 200)
                if s2 and 1 < len(s2) < 160:
                    return s2
    return None


def main():
    if "--stats" in sys.argv and os.path.exists(CACHE):
        ix = Index.load()
    else:
        pe = tkiw.load()
        print(f"image: {pe.path}")
        ix = Index.build(pe)
        ix.save()
    print(f"functions indexed : {len(ix.func_vars)}")
    print(f"distinct variables: {len(ix.var_funcs)}")
    print(f"distinct strings  : {len(ix.str_funcs)}")
    print(f"call edges        : {sum(len(v) for v in ix.func_calls.values())}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
