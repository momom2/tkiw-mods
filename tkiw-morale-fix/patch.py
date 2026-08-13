#!/usr/bin/env python3
"""
Resume-only morale fix for "The King is Watching".

The game keeps morale_current (what King effects actually read) and morale_target.
Loading a save restores morale_target correctly but leaves morale_current at 0, so
it crawls up via approach() at 0.15/frame scaled by TIME_SCALE - which is 0 whenever
gameplay is frozen. This patch assigns morale_current = morale_target exactly once,
when a run is resumed. Fresh runs are unaffected.

Usage:  python patch.py [path to game folder or .exe]
"""
import os
import struct
import sys

IB = 0x140000000

# ---- hook A: run controller "setup", right after the saved gc_stat_mods are applied.
# Arms the latch, but only when PLAYER_CONTINUED_RUN is true - setup() also runs for
# fresh runs, so the flag is what makes this resume-only.
SITE_A_RVA = 0x1610842
SITE_A_ORIG = bytes.fromhex("4c896424304489642438")      # mov [rsp+30],r12 ; mov [rsp+38],r12d
PCR_SLOT_DISP = 0x258                                    # [rbp+0x258] = RValue* PLAYER_CONTINUED_RUN

# ---- hook B: gameplay controller Step, where the approach() result is stored into
# morale_current. When armed we store morale_target (rbx) instead of the approach
# result (rax), so the snap does not depend on approach()/SCALED_DELTA at all.
SITE_B_RVA = 0x1317262
SITE_B_ORIG = bytes.fromhex("488bd0488bcee82386d7fe")    # mov rdx,rax ; mov rcx,rsi ; call 8f890

COPY_RVALUE_RVA = 0x8F890                                # COPY_RValue(dst, src)

# hook sites used by earlier versions of this patch, so unpatch.py can undo them too
LEGACY_SITES = [(0x131721A, bytes.fromhex("488bd7488d4dc0e86a86d7fe"))]

SEC_NAME = b".msnap\0\0"
SEC_CHARS = 0xE0000060      # CODE | INITIALIZED_DATA | EXECUTE | READ | WRITE
CAVE_SIZE = 0x200

EXE_NAME = "The King is Watching.exe"
BACKUP_SUFFIX = ".orig"

# The pristine copy lives here, in the mod's own folder, not next to the game.
# The game folder then shows no trace of this mod, and two mods that both back
# up the exe can't shadow each other's backup. The backup travels with the
# unpatch.py that knows how to use it.
MOD_DIR = os.path.dirname(os.path.abspath(__file__))


def backup_path(exe):
    return os.path.join(MOD_DIR, os.path.basename(exe) + BACKUP_SUFFIX)


def legacy_backup_path(exe):
    """Where versions of this patch before the move left the backup."""
    return exe + BACKUP_SUFFIX


# ---------------------------------------------------------------- locating the game
def steam_libraries():
    roots = []
    if os.name == "nt":
        roots += [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"]
    home = os.path.expanduser("~")
    roots += [
        os.path.join(home, ".steam", "steam"),
        os.path.join(home, ".local", "share", "Steam"),
        os.path.join(home, "Library", "Application Support", "Steam"),
    ]
    libs = []
    for root in roots:
        if not os.path.isdir(root):
            continue
        libs.append(root)
        try:
            with open(os.path.join(root, "steamapps", "libraryfolders.vdf"),
                      encoding="utf-8", errors="replace") as fh:
                for line in fh:
                    parts = line.split('"')
                    if len(parts) >= 5 and parts[1] == "path":
                        libs.append(parts[3].replace("\\\\", "\\"))
        except OSError:
            pass
    return libs


def find_exe(arg=None):
    if arg:
        if os.path.isdir(arg):
            cand = os.path.join(arg, EXE_NAME)
            if os.path.isfile(cand):
                return cand
            sys.exit(f"error: no '{EXE_NAME}' inside {arg}")
        if os.path.isfile(arg):
            return arg
        sys.exit(f"error: no such file or folder: {arg}")
    for lib in steam_libraries():
        cand = os.path.join(lib, "steamapps", "common", "The King is Watching", EXE_NAME)
        if os.path.isfile(cand):
            return cand
    sys.exit("error: could not find the game automatically.\n"
             "       pass the game folder, e.g.:\n"
             '       python patch.py "C:\\Program Files (x86)\\Steam\\steamapps\\'
             'common\\The King is Watching"')


# ---------------------------------------------------------------- tiny PE helpers
class PE:
    def __init__(self, data):
        self.d = data
        self.peo = struct.unpack_from("<I", data, 0x3C)[0]
        if data[self.peo:self.peo + 4] != b"PE\0\0":
            sys.exit("error: not a PE executable")
        self.nsec = struct.unpack_from("<H", data, self.peo + 6)[0]
        self.opt = self.peo + 24
        self.sectab = self.opt + struct.unpack_from("<H", data, self.peo + 20)[0]
        self.file_align = struct.unpack_from("<I", data, self.opt + 36)[0]
        self.sect_align = struct.unpack_from("<I", data, self.opt + 32)[0]
        self.secs = []
        for i in range(self.nsec):
            o = self.sectab + i * 40
            vsize, va, rawsize, raw = struct.unpack_from("<IIII", data, o + 8)
            self.secs.append(dict(name=data[o:o + 8], vsize=vsize, va=va,
                                  rawsize=rawsize, raw=raw, hdr=o))

    def rva2off(self, rva):
        for s in self.secs:
            if s["va"] <= rva < s["va"] + max(s["vsize"], s["rawsize"]):
                d = rva - s["va"]
                if d < s["rawsize"]:
                    return s["raw"] + d
        sys.exit(f"error: rva 0x{rva:x} is not file-backed")

    def section(self, name):
        for s in self.secs:
            if s["name"] == name:
                return s
        return None


def align(v, a):
    return (v + a - 1) // a * a


def rel32(from_rva, insn_len, to_rva):
    return struct.pack("<i", to_rva - (from_rva + insn_len))


# ---------------------------------------------------------------- the patch itself
def build(data):
    pe = PE(data)

    if pe.section(SEC_NAME):
        print("this exe is already patched.\n"
              "run python unpatch.py first, then install again.")
        return None

    for rva, orig, label in ((SITE_A_RVA, SITE_A_ORIG, "hook A"),
                             (SITE_B_RVA, SITE_B_ORIG, "hook B")):
        off = pe.rva2off(rva)
        found = bytes(data[off:off + len(orig)])
        if found != orig:
            sys.exit(f"error: {label} signature mismatch at rva 0x{rva:x} (file 0x{off:x})\n"
                     f"       expected {orig.hex(' ')}\n"
                     f"       found    {found.hex(' ')}\n"
                     "       This exe is not the build this patch was built for\n"
                     "       (a game update moves every address). Nothing was changed.")

    cave_rva = align(max(s["va"] + s["vsize"] for s in pe.secs), pe.sect_align)
    cave_raw = align(len(data), pe.file_align)
    if cave_raw != len(data):
        sys.exit("error: unexpected trailing data in the exe; refusing to patch")

    armed = cave_rva
    stub_a = cave_rva + 0x20
    stub_b = cave_rva + 0x80
    cave = bytearray(CAVE_SIZE)

    # ---- stub A: if (PLAYER_CONTINUED_RUN) armed = 1; then the displaced instructions
    a, p = bytearray(), stub_a
    a += b"\x50"; p += 1                                            # push rax
    a += b"\x48\x8B\x85" + struct.pack("<i", PCR_SLOT_DISP); p += 7  # mov rax,[rbp+0x258]
    a += b"\x48\x8B\x00"; p += 3                                    # mov rax,[rax]
    a += b"\x48\x85\xC0"; p += 3                                    # test rax,rax
    a += b"\x74\x07"; p += 2                                        # je skip
    a += b"\xC6\x05" + rel32(p, 7, armed) + b"\x01"; p += 7         # mov byte[armed],1
    a += b"\x58"; p += 1                                            # skip: pop rax
    a += SITE_A_ORIG; p += len(SITE_A_ORIG)
    a += b"\xE9" + rel32(p, 5, SITE_A_RVA + len(SITE_A_ORIG))
    cave[stub_a - cave_rva:stub_a - cave_rva + len(a)] = a

    # ---- stub B: rdx = armed ? morale_target(rbx) : approach result(rax); then store
    b, p = bytearray(), stub_b
    b += b"\x80\x3D" + rel32(p, 7, armed) + b"\x00"; p += 7         # cmp byte[armed],0
    b += b"\x74\x0C"; p += 2                                        # je normal
    b += b"\xC6\x05" + rel32(p, 7, armed) + b"\x00"; p += 7         # mov byte[armed],0
    b += b"\x48\x8B\xD3"; p += 3                                    # mov rdx,rbx
    b += b"\xEB\x03"; p += 2                                        # jmp have
    b += b"\x48\x8B\xD0"; p += 3                                    # normal: mov rdx,rax
    b += b"\x48\x8B\xCE"; p += 3                                    # have:   mov rcx,rsi
    b += b"\xE8" + rel32(p, 5, COPY_RVALUE_RVA); p += 5             #         call COPY_RValue
    b += b"\xE9" + rel32(p, 5, SITE_B_RVA + len(SITE_B_ORIG))
    cave[stub_b - cave_rva:stub_b - cave_rva + len(b)] = b

    for rva, ln, stub in ((SITE_A_RVA, len(SITE_A_ORIG), stub_a),
                          (SITE_B_RVA, len(SITE_B_ORIG), stub_b)):
        off = pe.rva2off(rva)
        data[off:off + ln] = b"\xE9" + rel32(rva, 5, stub) + b"\x90" * (ln - 5)

    hdr_slot = pe.sectab + pe.nsec * 40
    if hdr_slot + 40 > min(s["raw"] for s in pe.secs if s["raw"]):
        sys.exit("error: no room in the PE header for another section")
    data[hdr_slot:hdr_slot + 40] = (
        SEC_NAME + struct.pack("<IIII", CAVE_SIZE, cave_rva, CAVE_SIZE, cave_raw)
        + struct.pack("<IIHH", 0, 0, 0, 0) + struct.pack("<I", SEC_CHARS))
    struct.pack_into("<H", data, pe.peo + 6, pe.nsec + 1)
    struct.pack_into("<I", data, pe.opt + 56, align(cave_rva + CAVE_SIZE, pe.sect_align))
    data += cave
    return data


def main():
    exe = find_exe(sys.argv[1] if len(sys.argv) > 1 else None)
    print(f"game: {exe}")
    with open(exe, "rb") as fh:
        data = bytearray(fh.read())

    out = build(data)
    if out is None:
        return 1

    backup = backup_path(exe)
    try:
        if not os.path.exists(backup) and not os.path.exists(legacy_backup_path(exe)):
            with open(exe, "rb") as src, open(backup, "wb") as dst:
                dst.write(src.read())
            print(f"backup: {backup}")
        with open(exe, "wb") as fh:
            fh.write(out)
    except PermissionError:
        sys.exit(f"error: cannot write to {os.path.dirname(exe)}\n"
                 "       close the game / Steam and try again, or run elevated.")
    print("patched. resuming a reign now restores morale instantly.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
