#!/usr/bin/env python3
"""
Offline analysis toolkit for "The King is Watching" (GameMaker YYC build).

The game compiles all GML to native x86-64, so there is no bytecode to
decompile. But the executable is effectively self-symbolising, via two tables
this module recovers:

  * a 24-byte-stride table in .data pairing `gml_*` name strings with function
    pointers  ->  a full symbol table for the compiled GML
  * each GML variable name string is followed 8 bytes later by its variable-id
    slot, so a rip-relative operand whose target minus 8 holds a pointer to a
    C string identifies the variable being touched

Plus .pdata, which hands over function boundaries for free.

Usage as a library; the scripts next to it answer specific questions.
"""
import bisect
import os
import struct
import sys

GAME_DIR_DEFAULT = r"C:\Program Files (x86)\Steam\steamapps\common\The King is Watching"
EXE_NAME = "The King is Watching.exe"
# the resume-morale-fix stores the pristine exe here; prefer it, so the
# analysis describes the shipped game rather than someone's patched copy
PRISTINE_HINTS = [
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                 "tkiw-morale-fix", EXE_NAME + ".orig"),
]


def find_image(arg=None):
    if arg:
        return arg
    for hint in PRISTINE_HINTS:
        if os.path.isfile(hint):
            return os.path.normpath(hint)
    cand = os.path.join(GAME_DIR_DEFAULT, EXE_NAME)
    if os.path.isfile(cand):
        return cand
    sys.exit("error: could not find the executable; pass a path")


class PE:
    def __init__(self, path):
        self.path = path
        with open(path, "rb") as fh:
            self.d = fh.read()
        d = self.d
        self.peo = struct.unpack_from("<I", d, 0x3C)[0]
        if d[self.peo:self.peo + 4] != b"PE\0\0":
            sys.exit("error: not a PE executable")
        self.nsec = struct.unpack_from("<H", d, self.peo + 6)[0]
        self.opt = self.peo + 24
        if struct.unpack_from("<H", d, self.opt)[0] != 0x20B:
            sys.exit("error: expected PE32+")
        self.image_base = struct.unpack_from("<Q", d, self.opt + 24)[0]
        self.sectab = self.opt + struct.unpack_from("<H", d, self.peo + 20)[0]
        self.sections = []
        for i in range(self.nsec):
            o = self.sectab + i * 40
            vsize, va, rawsize, raw = struct.unpack_from("<IIII", d, o + 8)
            self.sections.append(dict(
                name=d[o:o + 8].rstrip(b"\0").decode(), vsize=vsize, va=va,
                rawsize=rawsize, raw=raw,
                chars=struct.unpack_from("<I", d, o + 36)[0]))
        self._dd = {}
        for i in range(16):
            self._dd[i] = struct.unpack_from("<II", d, self.opt + 112 + i * 8)

    # ---- address conversion
    def sec_of_rva(self, rva):
        for s in self.sections:
            if s["va"] <= rva < s["va"] + max(s["vsize"], s["rawsize"]):
                return s
        return None

    def rva2off(self, rva):
        s = self.sec_of_rva(rva)
        if s is None:
            return None
        delta = rva - s["va"]
        return s["raw"] + delta if delta < s["rawsize"] else None

    def off2rva(self, off):
        for s in self.sections:
            if s["raw"] and s["raw"] <= off < s["raw"] + s["rawsize"]:
                return s["va"] + (off - s["raw"])
        return None

    def section(self, name):
        for s in self.sections:
            if s["name"] == name:
                return s
        return None

    def va2rva(self, va):
        return va - self.image_base

    def rva2va(self, rva):
        return rva + self.image_base

    # ---- reads (rva-addressed)
    def u8(self, rva):
        o = self.rva2off(rva)
        return None if o is None else self.d[o]

    def u32(self, rva):
        o = self.rva2off(rva)
        return None if o is None else struct.unpack_from("<I", self.d, o)[0]

    def u64(self, rva):
        o = self.rva2off(rva)
        return None if o is None else struct.unpack_from("<Q", self.d, o)[0]

    def bytes_at(self, rva, n):
        o = self.rva2off(rva)
        return None if o is None else self.d[o:o + n]

    def cstr(self, rva, limit=512):
        o = self.rva2off(rva)
        if o is None:
            return None
        end = self.d.find(b"\0", o, o + limit)
        if end < 0:
            return None
        raw = self.d[o:end]
        try:
            return raw.decode("utf-8")
        except UnicodeDecodeError:
            return raw.decode("latin-1")

    def is_printable_cstr(self, rva, limit=512):
        s = self.cstr(rva, limit)
        if not s:
            return None
        return s if all(0x20 <= ord(c) < 0x7F for c in s) else None

    def in_section(self, rva, name):
        s = self.sec_of_rva(rva)
        return s is not None and s["name"] == name


# --------------------------------------------------------------- gml symbols
def gml_function_table(pe, stride=24):
    """Recover {name: rva} for every compiled GML function.

    Walks the initialised part of .data looking for records whose first qword
    points at a `gml_` string and whose second points into .text, then confirms
    the record stride by requiring runs of consecutive hits.
    """
    data = pe.section(".data")
    text = pe.section(".text")
    if data is None or text is None:
        return {}
    text_lo, text_hi = text["va"], text["va"] + text["vsize"]

    def record_at(rva):
        name_va, func_va = pe.u64(rva), pe.u64(rva + 8)
        if not name_va or not func_va:
            return None
        name_rva, func_rva = pe.va2rva(name_va), pe.va2rva(func_va)
        if not (text_lo <= func_rva < text_hi):
            return None
        if not pe.in_section(name_rva, ".rdata"):
            return None
        name = pe.is_printable_cstr(name_rva)
        if not name or not name.startswith("gml_"):
            return None
        return name, func_rva

    out, seen = {}, set()
    lo = data["va"]
    hi = data["va"] + data["rawsize"]
    rva = lo - (lo % 8) + (8 if lo % 8 else 0)
    while rva < hi:
        rec = record_at(rva)
        if rec is None:
            rva += 8
            continue
        # confirm: require at least three consecutive records at `stride`
        run, probe = [], rva
        while probe < hi:
            r = record_at(probe)
            if r is None:
                break
            run.append(r)
            probe += stride
        if len(run) >= 3:
            for name, func_rva in run:
                if name in seen and out.get(name) != func_rva:
                    continue  # keep the first binding for duplicated names
                seen.add(name)
                out.setdefault(name, func_rva)
            rva = probe
        else:
            rva += 8
    return out


def gml_variable_slots(pe):
    """Recover {slot_rva: variable_name}.

    Each variable name string is followed 8 bytes later by its id slot. The
    strings live in .rdata; the slots live in .data and hold 0xFFFFFFFF on
    disk (they are resolved at startup).
    """
    d = pe.d
    out = {}
    data = pe.section(".data")
    if data is None:
        return out
    lo, hi = data["raw"], data["raw"] + data["rawsize"]
    off = lo
    while off + 16 <= hi:
        name_va = struct.unpack_from("<Q", d, off)[0]
        if name_va:
            name_rva = pe.va2rva(name_va)
            if pe.in_section(name_rva, ".rdata"):
                slot_off = off + 8
                if struct.unpack_from("<I", d, slot_off)[0] == 0xFFFFFFFF:
                    name = pe.is_printable_cstr(name_rva, 128)
                    if name and name.isidentifier():
                        out[pe.off2rva(slot_off)] = name
        off += 8
    return out


# ------------------------------------------------------------------- .pdata
def pdata_functions(pe):
    """[(start_rva, end_rva)] from the exception directory, sorted."""
    sec = pe.section(".pdata")
    if sec is None:
        return []
    d, out = pe.d, []
    n = sec["rawsize"] // 12
    for i in range(n):
        s, e, _u = struct.unpack_from("<III", d, sec["raw"] + i * 12)
        if s and e > s:
            out.append((s, e))
    out.sort()
    return out


class FunctionIndex:
    """Maps an rva to the enclosing function, and names it where possible."""

    def __init__(self, pe, funcs=None, symbols=None):
        self.pe = pe
        self.funcs = funcs if funcs is not None else pdata_functions(pe)
        self.starts = [f[0] for f in self.funcs]
        self.by_start = {}
        for name, rva in (symbols or {}).items():
            self.by_start.setdefault(rva, name)

    def enclosing(self, rva):
        i = bisect.bisect_right(self.starts, rva) - 1
        if i < 0:
            return None
        s, e = self.funcs[i]
        return (s, e) if s <= rva < e else None

    def name_of(self, start_rva):
        return self.by_start.get(start_rva)

    def describe(self, rva):
        f = self.enclosing(rva)
        if f is None:
            return f"<no function> @ {rva:#x}"
        name = self.name_of(f[0]) or f"sub_{f[0]:x}"
        return f"{name}+{rva - f[0]:#x}"


# -------------------------------------------------------------- disassembly
def make_disassembler():
    try:
        import capstone
    except ImportError:
        sys.exit("error: pip install capstone")
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    return md


def disasm_function(pe, md, start_rva, end_rva, max_bytes=0x20000):
    code = pe.bytes_at(start_rva, min(end_rva - start_rva, max_bytes))
    if code is None:
        return []
    return list(md.disasm(code, pe.rva2va(start_rva)))


def rip_targets(pe, insn):
    """rva targets of any rip-relative memory operand in `insn`."""
    import capstone
    out = []
    for op in insn.operands:
        if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
            out.append(pe.va2rva(insn.address + insn.size + op.mem.disp))
    return out


def load(path=None):
    pe = PE(find_image(path))
    return pe
