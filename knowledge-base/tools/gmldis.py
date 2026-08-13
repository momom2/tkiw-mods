#!/usr/bin/env python3
"""
Annotated disassembly of a compiled GML function.

Resolves, inline:
  var:NAME     a rip-relative read of a GML variable-id slot
  "str"        a rip-relative reference to a C string, or to a constant
               RValue holding one
  -> symbol    a call to a named GML function or a known runtime helper

Usage:
  python dis.py <name-or-substring> [--limit N] [--exe PATH]
  python dis.py --grep <substring>          list matching symbol names
"""
import re
import sys

import tkiw

# GameMaker RValue is 16 bytes: value(8) then flags(4) then kind(4).
RV_KIND_STRING = 1


def decode_rvalue_string(pe, rva):
    """If `rva` looks like a constant RValue holding a string, return it."""
    raw = pe.bytes_at(rva, 16)
    if raw is None or len(raw) < 16:
        return None
    import struct
    ptr, _flags, kind = struct.unpack("<QII", raw)
    if kind != RV_KIND_STRING or not ptr:
        return None
    srva = pe.va2rva(ptr)
    if not pe.sec_of_rva(srva):
        return None
    # GM string objects are refcounted: {refcount, size, char data...}
    for probe in (srva, srva + 8, srva + 12, srva + 16):
        s = pe.is_printable_cstr(probe, 200)
        if s and len(s) > 1:
            return s
    return None


class Annotator:
    def __init__(self, pe, strings=True):
        self.pe = pe
        self.syms = tkiw.gml_function_table(pe)
        self.slots = tkiw.gml_variable_slots(pe)
        self.strs = {}
        if strings:
            try:
                import strconsts
                self.strs = strconsts.load(pe)
            except Exception:
                pass
        self.by_rva = {}
        for n, r in self.syms.items():
            self.by_rva.setdefault(r, n)
        self.idx = tkiw.FunctionIndex(pe, symbols=self.syms)
        self.md = tkiw.make_disassembler()

    def annotate_target(self, rva):
        pe = self.pe
        if rva in self.slots:
            return f"var:{self.slots[rva]}"
        if rva in self.strs:
            return f"str {self.strs[rva]!r}"
        s = pe.is_printable_cstr(rva, 200)
        if s and len(s) > 1:
            return f'"{s}"'
        rv = decode_rvalue_string(pe, rva)
        if rv is not None:
            return f'RValue "{rv}"'
        # a pointer to a string?
        q = pe.u64(rva)
        if q and pe.sec_of_rva(pe.va2rva(q)):
            s2 = pe.is_printable_cstr(pe.va2rva(q), 200)
            if s2 and len(s2) > 1:
                return f'-> "{s2}"'
        return None

    def call_name(self, rva):
        n = self.by_rva.get(rva)
        if n:
            return n
        f = self.idx.enclosing(rva)
        if f and f[0] == rva:
            return f"sub_{rva:x}"
        return f"loc_{rva:x}"

    def function_range(self, name):
        if name not in self.syms:
            return None
        start = self.syms[name]
        f = self.idx.enclosing(start)
        return (start, f[1]) if f else (start, start + 0x400)

    def dump(self, name, limit=None, out=sys.stdout):
        rng = self.function_range(name)
        if rng is None:
            print(f"no such symbol: {name}", file=out)
            return
        start, end = rng
        print(f"; {name}", file=out)
        print(f"; rva {start:#x}..{end:#x}  ({end - start} bytes)", file=out)
        n = 0
        for insn in tkiw.disasm_function(self.pe, self.md, start, end):
            notes = []
            for t in tkiw.rip_targets(self.pe, insn):
                a = self.annotate_target(t)
                notes.append(a if a else f"data:{t:#x}")
            if insn.mnemonic in ("call", "jmp") and insn.op_str.startswith("0x"):
                tgt = self.pe.va2rva(int(insn.op_str, 16))
                if self.pe.in_section(tgt, ".text"):
                    notes.append(f"-> {self.call_name(tgt)}")
            rva = self.pe.va2rva(insn.address)
            line = f"{rva:08x}  {insn.mnemonic:<7} {insn.op_str}"
            if notes:
                line = f"{line:<58} ; {'  '.join(notes)}"
            print(line, file=out)
            n += 1
            if limit and n >= limit:
                print("...", file=out)
                break


def main():
    args = sys.argv[1:]
    exe = None
    if "--exe" in args:
        i = args.index("--exe")
        exe = args[i + 1]
        del args[i:i + 2]
    limit = None
    if "--limit" in args:
        i = args.index("--limit")
        limit = int(args[i + 1])
        del args[i:i + 2]

    pe = tkiw.load(exe)
    if args and args[0] == "--grep":
        syms = tkiw.gml_function_table(pe)
        pat = re.compile(args[1], re.I)
        for n in sorted(syms):
            if pat.search(n):
                print(f"{syms[n]:08x}  {n}")
        return 0

    if not args:
        print(__doc__)
        return 1

    ann = Annotator(pe)
    want = args[0]
    if want in ann.syms:
        ann.dump(want, limit)
        return 0
    cands = [n for n in ann.syms if want in n]
    if not cands:
        print(f"no symbol matching {want!r}")
        return 1
    if len(cands) > 1:
        exact = [c for c in cands if c.endswith(want)]
        if len(exact) == 1:
            cands = exact
    if len(cands) > 1:
        print(f"{len(cands)} matches:")
        for c in sorted(cands)[:40]:
            print("  ", c)
        return 1
    ann.dump(cands[0], limit)
    return 0


if __name__ == "__main__":
    sys.exit(main())
