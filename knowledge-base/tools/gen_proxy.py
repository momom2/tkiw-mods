#!/usr/bin/env python3
"""
Pick and generate a proxy-DLL export list.

A mod gets into this game by placing a DLL in the game folder whose name matches
one the game statically imports and which is **not** a KnownDLL. The copy in the
application directory then wins the loader search and is mapped before the game's
entry point. In exchange the mod owes the process a working version of that DLL,
so every export has to be forwarded to the real one in System32.

Two jobs:

  python gen_proxy.py --survey [exe]        which DLLs are candidates, ranked
  python gen_proxy.py winmm.dll             the Rust forwarder list to paste

## Choosing a slot: coverage, not export count

The obvious metric -- "fewest exports to forward" -- is the wrong one. A proxy can
only provide exports it can *name*, and many system DLLs export a large fraction
of their table by ordinal only. An import a proxy cannot satisfy does not degrade
gracefully: the importing DLL fails to load. So rank candidates by how many
exports are ordinal-only, and treat the named count as mere typing.

On the 2026-08-10 build this is what the survey says, and why the momomod kit
uses `winmm.dll`:

    version.dll   17 named,  0 ordinal-only   (taken by the auto-picker)
    winmm.dll    180 named,  1 ordinal-only   <- the kit
    dbghelp.dll  252 named, 16 ordinal-only
    dwmapi.dll    44 named, 75 ordinal-only   <- looks cheapest, is unusable

## The uniform-signature trampoline

Generated forwarders all take ten `usize` parameters whatever the real arity, and
pass all ten on. The x64 ABI puts the first four in registers and the rest on the
caller's stack, so extra parameters read mapped stack the callee then ignores.
One trampoline instead of hundreds of transcribed signatures.

**Integers and pointers only.** A float or double argument travels in XMM, which
a trampoline declared in terms of `usize` may clobber. Check the candidate has no
floating-point exports before reusing this.
"""
import os
import struct
import sys

# Loaded from the System32 image before anything else can claim the name, so a
# proxy in the application directory never wins for these.
KNOWN_DLLS = {
    "advapi32.dll", "clbcatq.dll", "combase.dll", "comdlg32.dll",
    "coremessaging.dll", "difxapi.dll", "gdi32.dll", "gdiplus.dll",
    "imagehlp.dll", "imm32.dll", "kernel32.dll", "msctf.dll", "msvcrt.dll",
    "normaliz.dll", "nsi.dll", "ole32.dll", "oleaut32.dll", "psapi.dll",
    "rpcrt4.dll", "sechost.dll", "setupapi.dll", "shcore.dll", "shell32.dll",
    "shlwapi.dll", "user32.dll", "wintrust.dll", "wldap32.dll", "ws2_32.dll",
}


class Pe:
    def __init__(self, path):
        self.path = path
        with open(path, "rb") as fh:
            self.d = fh.read()
        d = self.d
        self.peo = struct.unpack_from("<I", d, 0x3C)[0]
        if d[self.peo:self.peo + 4] != b"PE\0\0":
            raise ValueError(f"{path}: not a PE file")
        nsec = struct.unpack_from("<H", d, self.peo + 6)[0]
        opt = self.peo + 24
        magic = struct.unpack_from("<H", d, opt)[0]
        self.ddir = opt + (0x70 if magic == 0x20B else 0x60)
        sectab = opt + struct.unpack_from("<H", d, self.peo + 20)[0]
        self.secs = []
        for i in range(nsec):
            o = sectab + i * 40
            vs, va, rs, ra = struct.unpack_from("<IIII", d, o + 8)
            self.secs.append((va, vs, ra, rs))

    def off(self, rva):
        for va, vs, ra, rs in self.secs:
            if va <= rva < va + max(vs, rs):
                return ra + (rva - va)
        return None

    def cstr(self, rva):
        o = self.off(rva)
        return self.d[o:self.d.index(b"\0", o)].decode("latin-1")

    def imports(self):
        """{dll_name_lower: [imported function names]}"""
        rva, _size = struct.unpack_from("<II", self.d, self.ddir + 8)
        o = self.off(rva)
        out = {}
        while self.d[o:o + 20] != b"\0" * 20:
            oft, _ts, _fc, nrva, fta = struct.unpack_from("<IIIII", self.d, o)
            name = self.cstr(nrva)
            fns, t = [], self.off(oft or fta)
            while True:
                e = struct.unpack_from("<Q", self.d, t)[0]
                if e == 0:
                    break
                if e & (1 << 63):
                    fns.append(f"@{e & 0xFFFF}")
                else:
                    fns.append(self.cstr((e & 0x7FFFFFFF) + 2))
                t += 8
            out[name.lower()] = fns
            o += 20
        return out

    def exports(self):
        """(named_in_ordinal_order, ordinal_only_ordinals, ordinal_base)"""
        rva, _size = struct.unpack_from("<II", self.d, self.ddir)
        if not rva:
            return [], [], 0
        eo = self.off(rva)
        base, nfun, nnam, afun, anam, aord = struct.unpack_from("<IIIIII", self.d, eo + 0x10)
        ao, no, oo = self.off(afun), self.off(anam), self.off(aord)
        named = {}
        for i in range(nnam):
            nr = struct.unpack_from("<I", self.d, no + i * 4)[0]
            named[struct.unpack_from("<H", self.d, oo + i * 2)[0]] = self.cstr(nr)
        unnamed = [
            base + i for i in range(nfun)
            if struct.unpack_from("<I", self.d, ao + i * 4)[0] and i not in named
        ]
        return [named[k] for k in sorted(named)], unnamed, base


def system32(name):
    return os.path.join(os.environ["SystemRoot"], "System32", name)


def survey(exe):
    pe = Pe(exe)
    rows = []
    for dll, fns in pe.imports().items():
        if dll in KNOWN_DLLS:
            continue
        path = system32(dll)
        if not os.path.isfile(path):
            rows.append((dll, len(fns), None, None, "not in System32"))
            continue
        try:
            named, unnamed, _ = Pe(path).exports()
        except Exception as e:
            rows.append((dll, len(fns), None, None, str(e)))
            continue
        by_ord = sum(1 for f in fns if f.startswith("@"))
        note = f"game imports {by_ord} by ordinal" if by_ord else ""
        rows.append((dll, len(fns), len(named), len(unnamed), note))
    rows.sort(key=lambda r: (r[3] is None, r[3] if r[3] is not None else 0, r[2] or 0))
    print(f"proxy candidates for {os.path.basename(exe)}")
    print(f"  {'dll':22} {'used':>4} {'named':>6} {'ord-only':>9}")
    for dll, used, named, unnamed, note in rows:
        n = "?" if named is None else named
        u = "?" if unnamed is None else unnamed
        flag = "  <- best" if unnamed == min(
            (r[3] for r in rows if r[3] is not None), default=-1) else ""
        print(f"  {dll:22} {used:>4} {n:>6} {u:>9}{flag}  {note}")
    print("\nrank by ord-only: an export a proxy cannot name is an import it")
    print("cannot satisfy, and that stops the importing DLL from loading.")


def emit(dll):
    named, unnamed, base = Pe(system32(dll)).exports()
    bad = [n for n in named if not n.isidentifier()]
    print(f"// {dll}: {len(named)} named exports, {len(unnamed)} ordinal-only")
    if unnamed:
        print(f"// NOT forwarded (ordinal-only, unprovidable): "
              f"{', '.join('@' + str(o) for o in unnamed)}")
    if bad:
        print(f"// WARNING: not valid Rust identifiers, fix by hand: {bad}")
    print(f"const N: usize = {len(named)};")
    print("forwarders! {")
    for i, n in enumerate(sorted(named)):
        print(f"    {i:3} => {n},")
    print("}")


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return 1
    if args[0] == "--survey":
        exe = args[1] if len(args) > 1 else None
        if exe is None:
            sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
            import tkiw
            exe = tkiw.find_image()
        survey(exe)
        return 0
    emit(args[0])
    return 0


if __name__ == "__main__":
    sys.exit(main())
