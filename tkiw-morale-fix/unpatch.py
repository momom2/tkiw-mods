#!/usr/bin/env python3
"""
Reverse the resume-only morale fix for "The King is Watching".

Restores the two hooked instruction sequences, removes the appended .msnap
section and its header, and truncates the file - producing a byte-for-byte
copy of the original exe. Does not need the .orig backup to work.

Usage:  python unpatch.py [--purge] [path to game folder or .exe]

  --purge   also delete the .orig backup kept in this folder
"""
import os
import struct
import sys

from patch import (PE, SEC_NAME, SITE_A_RVA, SITE_A_ORIG, SITE_B_RVA, SITE_B_ORIG,
                   LEGACY_SITES, align, find_exe, backup_path, legacy_backup_path)

# every site any version of this patch has ever hooked
KNOWN_SITES = [(SITE_A_RVA, SITE_A_ORIG), (SITE_B_RVA, SITE_B_ORIG)] + LEGACY_SITES


def revert(data):
    pe = PE(data)
    sec = pe.section(SEC_NAME)
    if sec is None:
        print("not patched - nothing to do.")
        return None

    # 1. put back whichever hooked instructions are actually detoured
    restored = 0
    for rva, orig in KNOWN_SITES:
        off = pe.rva2off(rva)
        if data[off] == 0xE9 and bytes(data[off:off + len(orig)]) != orig:
            data[off:off + len(orig)] = orig
            restored += 1
    if not restored:
        sys.exit("error: the patch section is present but no known hook site is "
                 "detoured; refusing to guess. Restore from the .orig backup.")
    print(f"restored {restored} hook site(s)")

    # 2. drop the section header and fix the header counts
    data[sec["hdr"]:sec["hdr"] + 40] = b"\0" * 40
    struct.pack_into("<H", data, pe.peo + 6, pe.nsec - 1)
    remaining = [s for s in pe.secs if s["name"] != SEC_NAME]
    struct.pack_into("<I", data, pe.opt + 56,
                     align(max(s["va"] + s["vsize"] for s in remaining), pe.sect_align))

    # 3. truncate the appended data
    end = sec["raw"] + sec["rawsize"]
    if len(data) != end:
        sys.exit(f"error: file is 0x{len(data):x} bytes but the patch section ends at "
                 f"0x{end:x}; something else appended data. Restore from the .orig backup.")
    del data[sec["raw"]:]
    return data


def main():
    args = [a for a in sys.argv[1:] if a != "--purge"]
    purge = "--purge" in sys.argv[1:]

    exe = find_exe(args[0] if args else None)
    print(f"game: {exe}")
    with open(exe, "rb") as fh:
        data = bytearray(fh.read())

    out = revert(data)
    # this mod's folder first; the second is where older versions left it, next
    # to the exe, and is still honoured so an old install can be undone
    backups = [p for p in (backup_path(exe), legacy_backup_path(exe))
               if os.path.exists(p)]

    if out is not None:
        try:
            with open(exe, "wb") as fh:
                fh.write(out)
        except PermissionError:
            sys.exit(f"error: cannot write {exe}\n"
                     "       close the game / Steam and try again, or run elevated.")
        print("unpatched - original executable restored.")

        for backup in backups:
            with open(backup, "rb") as fh:
                if fh.read() == bytes(out):
                    print(f"verified: identical to {backup}")
                else:
                    print(f"note: result differs from {backup} - keeping it just in case.")
                    purge = False

    if purge:
        for backup in backups:
            os.remove(backup)
            print(f"removed: {backup}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
