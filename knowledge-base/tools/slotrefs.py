#!/usr/bin/env python3
"""
Every reference to a GML variable, found exhaustively.

`index.py` answers "which functions touch this variable" from a cross-reference
built per named GML function. That is the convenient answer, and it is not a
complete one: 363 of the 13,132 named functions fail to index, and the .pdata
function list is three times larger than the name table, so a variable touched
only from an unnamed or unindexed function is invisible there.

This scans the raw bytes of .text instead. A variable id is always loaded with
`mov r32, dword ptr [rip + disp32]` addressing the variable's id slot, so every
reference is one of two byte shapes:

    8B /r  mod=00 rm=101  disp32          6 bytes
    REX 8B /r  mod=00 rm=101  disp32      7 bytes

Match those, resolve the target, and the result is every reference in the image
whether or not the containing function has a name.

Use it when the question is "is this ever written", where a false negative from
an incomplete index is the difference between a bug report and a wrong one.

  python slotrefs.py <variable-name> [<variable-name> ...]
  python slotrefs.py --grep <substring>     list variable names that match
"""
import bisect
import struct
import sys

import tkiw

# mov r32/r64, r/m32  with mod=00, rm=101 -> [rip + disp32]
MOV_RM = 0x8B


def scan(pe, names):
    """{name: [(rva_of_instruction, containing_symbol)]} for each name."""
    slots = tkiw.gml_variable_slots(pe)
    want = {rva: n for rva, n in slots.items() if n in names}
    if not want:
        return {}

    syms = tkiw.gml_function_table(pe)
    starts = sorted(syms.values())
    by_rva = {}
    for n, r in syms.items():
        by_rva.setdefault(r, n)

    def owner(rva):
        i = bisect.bisect_right(starts, rva) - 1
        return by_rva.get(starts[i], "?") if i >= 0 else "?"

    text = pe.section(".text")
    data = pe.d[text["raw"]:text["raw"] + text["vsize"]]
    base = text["va"]

    out = {n: [] for n in names}
    for i in range(len(data) - 7):
        off = 1 if 0x40 <= data[i] <= 0x4F else 0      # optional REX
        if data[i + off] != MOV_RM:
            continue
        if (data[i + off + 1] & 0xC7) != 0x05:          # mod=00, rm=101
            continue
        disp = struct.unpack_from("<i", data, i + off + 2)[0]
        target = base + i + off + 6 + disp
        if target in want:
            at = base + i
            out[want[target]].append((at, owner(at)))
    return out


def main(argv):
    pe = tkiw.PE(tkiw.find_image())
    if argv and argv[0] == "--grep":
        needle = argv[1]
        found = sorted({n for n in tkiw.gml_variable_slots(pe).values() if needle in n})
        print("\n".join(found) or "(no match)")
        return 0
    if not argv:
        print(__doc__.strip())
        return 2

    for name, hits in scan(pe, set(argv)).items():
        print("== %s : %d references" % (name, len(hits)))
        if not hits:
            print("     (none -- the variable is declared but never addressed)")
        for at, fn in hits:
            print("     %08x   %s" % (at, fn))
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
