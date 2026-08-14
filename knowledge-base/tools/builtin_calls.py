#!/usr/bin/env python3
"""
Which builtins does a given GML function call, and how many times?

The compiled GML never calls a builtin directly. It calls the dispatcher at
`call_builtin_by_index` (0x1aa46c0) with an integer index, and the dispatcher looks up
a 24-byte entry: `base + index*24`, where `base` is a pointer filled in at runtime by
`Function_Add`. So a profile can see the dispatcher and the builtin's internals but not
the builtin's name, and a disassembly shows an integer.

The index is recoverable offline because it is the **registration order**: `Function_Add`
appends, so the Nth call site registers index N. Walking those call sites in address
order reconstructs the whole table.

  python builtin_calls.py obj_init_Create_0
  python builtin_calls.py obj_init_Create_0 --top 30

Cross-checked on every run: a handful of indices are resolved back to their function
address and compared against the name table `builtins.py` recovers independently. If
those disagree the registration order is not the index and the output is refused.
"""
import argparse
import bisect
import os
import sys

import tkiw

HERE = os.path.dirname(os.path.abspath(__file__))
DISPATCHER = 0x1AA46C0
FUNCTION_ADD = 0x1B6BCF0


def call_sites(pe, target):
    """rvas of every `call rel32` to `target`, in address order."""
    import struct
    sec = pe.section(".text")
    data = pe.d[sec["raw"]:sec["raw"] + sec["vsize"]]
    base = sec["va"]
    out = []
    for i in range(len(data) - 5):
        if data[i] != 0xE8:
            continue
        rel = struct.unpack_from("<i", data, i + 1)[0]
        if base + i + 5 + rel == target:
            out.append(base + i)
    return out


def builtin_name_at_slot(pe, slot_rva):
    """The builtin a dispatcher-index slot stands for, from the pointer 8 bytes below.

    The same shape as a GML variable slot: `<char* name>` then, eight bytes later, the
    integer the runtime fills in. Reading the name is exact and needs no index at all.
    """
    import struct
    b = pe.bytes_at(slot_rva - 8, 8)
    if not b:
        return None
    ptr = struct.unpack("<Q", b)[0]
    if not (pe.image_base < ptr < pe.image_base + 0x10000000):
        return None
    return pe.is_printable_cstr(ptr - pe.image_base, 80)


def registration_order(pe):
    """[(name, fn_rva)] in the order Function_Add was called: index N is entry N.

    Deliberately the *same* walk `builtins.py` does -- same state, same resets, same
    printable-string requirement -- differing only in appending to a list instead of
    filling a dict. A first attempt paraphrased it instead and agreed with it on only
    77% of entries, which is exactly the kind of near-miss that produces a table of
    plausible wrong names.
    """
    md = tkiw.make_disassembler()
    sites = call_sites(pe, FUNCTION_ADD)
    site_set = set(sites)
    fidx = tkiw.FunctionIndex(pe, symbols={})
    funcs = sorted(set(f for f in (fidx.enclosing(r) for r in sites) if f))

    table = []
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
                    argc = int(ops.split("+")[1].strip(" ]"), 0) if "+" in ops else 0
            elif m == "mov" and ops.startswith("r8d,"):
                try:
                    argc = int(ops.split(",")[1].strip(), 0)
                except ValueError:
                    pass
            elif m == "xor" and ops.startswith("r8d, r8d"):
                argc = 0
            elif m == "call" and rva in site_set:
                nm = pe.is_printable_cstr(name_rva, 80) if name_rva else None
                table.append((nm, fn_rva))
                name_rva = fn_rva = argc = None
    return table


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("function")
    ap.add_argument("--top", type=int, default=20)
    args = ap.parse_args(argv)

    pe = tkiw.PE(tkiw.find_image())
    syms = tkiw.gml_function_table(pe)
    matches = [n for n in syms if args.function in n]
    if not matches:
        print("no such GML function: %s" % args.function)
        return 2
    name = min(matches, key=len)
    start = syms[name]
    end = dict(tkiw.pdata_functions(pe)).get(start)
    if not end:
        print("%s has no .pdata entry" % name)
        return 2

    table = registration_order(pe)
    print("recovered %d builtin registrations" % len(table))

    # Cross-check: the independently recovered name table must agree.
    import pickle
    with open(os.path.join(HERE, "builtins.pickle"), "rb") as fh:
        known = dict(pickle.load(fh))
    # Keyed by *name*, not by address. Several builtins share one implementation --
    # `draw_vertex_texture_colour` and `draw_vertex_texture_color` are the same
    # function -- so a reverse map by address keeps one name and reports the other as a
    # disagreement. That put the first cross-check at 77% and refused a table that was
    # in fact fine.
    def rva_of(v):
        return v[0] if isinstance(v, (tuple, list)) else v

    agree = sum(1 for nm, fn in table if nm in known and rva_of(known[nm]) == fn)
    named = sum(1 for nm, _ in table if nm in known)
    print("cross-check: %d/%d entries agree with builtins.py" % (agree, named))
    if named and agree < named * 0.9:
        print("REFUSING: registration order does not look like the index")
        return 1

    # Now read the dispatcher call sites inside the target function.
    #
    # The index is *not* an immediate. It is loaded rip-relative from a slot the
    # runtime fills in at startup -- `mov eax, [rip+N]` then `mov [rsp+0x20], eax` --
    # exactly like a GML variable id. And like a variable id, the slot is preceded by a
    # pointer to its name, eight bytes below. So the call site names itself without
    # needing the index at all.
    md = tkiw.make_disassembler()
    counts = {}
    pending_name = None
    for insn in tkiw.disasm_function(pe, md, start, end):
        ops = insn.op_str
        if insn.mnemonic == "mov" and ops.startswith("eax, dword ptr [rip"):
            pending_name = None
            for t in tkiw.rip_targets(pe, insn):
                pending_name = builtin_name_at_slot(pe, t)
        elif insn.mnemonic == "call":
            tgt = insn.op_str
            if tgt.startswith("0x"):
                addr = int(tgt, 16) - pe.image_base
                if addr == DISPATCHER:
                    who = pending_name or "(index slot not resolved)"
                    counts[who] = counts.get(who, 0) + 1
            pending_name = None

    if not counts:
        print("\nno dispatcher calls found with a readable index in %s" % name)
        return 0

    print("\n%s calls %d distinct builtin(s):" % (name, len(counts)))
    for who, n in sorted(counts.items(), key=lambda kv: -kv[1])[:args.top]:
        print("  %5d call site(s)  %s" % (n, who))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
