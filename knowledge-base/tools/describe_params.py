#!/usr/bin/env python3
"""
Recover, statically, what each `{&N}` in a unit or trait description is
computed from.

    python describe_params.py               # writes describe_params.json here
    python describe_params.py --spot ash_dragon imp_giant

## How it works

Library descriptions hold `{&1}`-style tokens. `Library_element` binds a shared
default `replace_texts_parameters` which walks `replace_parameters_array` and
invokes each element: **the array holds one bound method per token**, and those
methods are per-unit anonymous functions compiled into `unit_library`.

So the recovery is:

1. one linear disassembly of `unit_library`'s body (and of each
   `___struct___*@unit_library` literal), recording three kinds of event in
   address order: string constants that name a unit or trait (block markers),
   rip references to the `replace_parameters_array` id slot (array sites), and
   `lea` references to `anon@*@unit_library` function addresses (bindings);
2. each array site takes the anon bindings in the window before it, in order —
   those are `{&1}`, `{&2}`, ... — and is attributed to the nearest preceding
   block marker;
3. each bound anon is disassembled on its own and mined for the variable slots
   it reads, the float constants it multiplies or divides by, and the game
   functions it calls. That mix, in order, is the "formula".

## Honesty about quality

This is best-effort reading of optimised code. What it reliably gets right is
**which fields feed a parameter** and the visible constants. It does not
reconstruct exact arithmetic ordering, and a parameter computed through helper
calls shows as the helper's name. Spot-check anything that matters; the anon
name is included so `gmldis.py` can settle a doubt.
"""
import argparse
import json
import os
import struct as pystruct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import strconsts
import tkiw

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "describe_params.json")
LIBRARIES = os.path.join(HERE, "..", "..", "tkiw-momomod-kit", "libraries.json")

FLOAT_OPS = {"mulsd": "*", "divsd": "/", "addsd": "+", "subsd": "-"}


def owners_of_interest():
    """unit system_names and trait system_name_fulls, from libraries.json."""
    with open(LIBRARIES, encoding="utf-8") as fh:
        d = json.load(fh)
    units = set(d["UNITS"].keys())
    traits = set()
    for u in d["UNITS"].values():
        t = u.get("trait")
        if isinstance(t, dict) and t.get("system_name_full"):
            traits.add(t["system_name_full"])
    return units, traits


def function_bounds(pe, funcs):
    ends = dict(tkiw.pdata_functions(pe))
    return {name: (rva, ends.get(rva)) for name, rva in funcs.items()}


def events_in(pe, md, begin, end, anon_addrs, slots, strs, unit_names, trait_names):
    """(addr, kind, value) for every marker/slot/binding in [begin, end).

    Every member-slot reference is an event, not just the array's: a member
    assignment is `slot-id load, method(anon), setter`, all in the same byte
    shape, so the only way to know which member a bound anon belongs to is
    that its nearest preceding slot event names it.

    Strings are resolved through the constant pool (`strconsts`), because GML
    string literals are referenced as RValue descriptors -- a raw c-string
    probe at the target address matches almost nothing.
    """
    out = []
    for insn in tkiw.disasm_function(pe, md, begin, end, max_bytes=end - begin):
        for t in tkiw.rip_targets(pe, insn):
            if t in slots:
                out.append((insn.address, "slot", slots[t]))
            elif t in anon_addrs:
                out.append((insn.address, "bind", anon_addrs[t]))
            else:
                s = strs.get(t)
                if s in unit_names:
                    out.append((insn.address, "unit", s))
                elif s in trait_names:
                    out.append((insn.address, "trait", s))
    return out


def mine_anon(pe, md, funcs_bounds, index, name):
    """Best-effort description of what one parameter method computes."""
    begin, end = funcs_bounds[name]
    if end is None:
        return "?"
    parts = []
    for insn in tkiw.disasm_function(pe, md, begin, end, max_bytes=end - begin):
        m = insn.mnemonic
        for t in tkiw.rip_targets(pe, insn):
            if t in mine_anon.slots:
                var = mine_anon.slots[t]
                if var not in ("undefined",) and (not parts or parts[-1] != var):
                    parts.append(var)
            elif m in FLOAT_OPS and pe.in_section(t, ".rdata"):
                raw = pe.u64(t)
                if raw is not None:
                    v = pystruct.unpack("<d", pystruct.pack("<Q", raw))[0]
                    if v == v and abs(v) < 1e12 and v != 0.0:
                        vs = str(int(v)) if v == int(v) else f"{v:g}"
                        parts.append(f"{FLOAT_OPS[m]} {vs}")
        if m == "call":
            target = insn.op_str
            try:
                rva = pe.va2rva(int(target, 16))
                fname = index.name_of(rva)
                if fname and fname.startswith("gml_Script_") and "@" not in fname:
                    parts.append(fname.removeprefix("gml_Script_") + "()")
            except ValueError:
                pass
    return " ".join(parts) if parts else "(no fields read - likely a constant or helper)"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--spot", nargs="*", default=[])
    args = ap.parse_args()

    pe = tkiw.load()
    md = tkiw.make_disassembler()
    funcs = tkiw.gml_function_table(pe)
    bounds = function_bounds(pe, funcs)
    index = tkiw.FunctionIndex(pe, funcs)

    slots = tkiw.gml_variable_slots(pe)  # {slot_rva: name}
    mine_anon.slots = slots
    rpa_slot = next((rva for rva, n in slots.items() if n == "replace_parameters_array"), None)
    if rpa_slot is None:
        sys.exit("no slot for replace_parameters_array")

    strs = strconsts.load(pe)
    unit_names, trait_names = owners_of_interest()
    anon_addrs = {
        rva: n for n, rva in funcs.items() if n.startswith("gml_Script_anon@")
    }

    # Regions to walk. unit_library and its struct literals carry per-unit
    # arrays, with the owner read from name-string markers. The `Unit_trait_*`
    # family constructors carry one *shared* array per trait class -- the
    # per-unit numbers arrive as constructor arguments -- so the whole region
    # is one fixed owner, recorded as `template:<class>`.
    regions = [("gml_Script_unit_library", None)]
    regions += [
        (n, None)
        for n in funcs
        if "___struct___" in n and n.endswith("@unit_library@unit_library")
    ]
    regions += [
        (n, "template:" + n.removeprefix("gml_Script_"))
        for n in funcs
        if "@" not in n and n.startswith("gml_Script_Unit_trait")
    ]

    result = {}  # owner -> [formula per token, in order]
    anons_named = {}  # owner -> [anon names], for gmldis follow-up
    for region, fixed_owner in regions:
        begin, end = bounds[region]
        if end is None:
            continue
        events = events_in(pe, md, begin, end, anon_addrs, slots, strs, unit_names, trait_names)
        # One array push is: read of replace_parameters_array's id slot, then
        # method(anon), then the push. A description with three tokens is
        # three pushes. Any other member's slot event in between closes the
        # window -- that is what keeps on_activate and friends out.
        owner = fixed_owner
        open_site = None  # address of an array-slot event awaiting its anon
        for addr, kind, value in events:
            if kind in ("unit", "trait"):
                if fixed_owner is None:
                    owner = value
                open_site = None
            elif kind == "slot":
                open_site = addr if value == "replace_parameters_array" else None
            elif kind == "bind":
                if open_site is not None and addr - open_site < 0x300 and owner:
                    anons_named.setdefault(owner, []).append(value)
                open_site = None
    for owner, names in anons_named.items():
        result[owner] = [mine_anon(pe, md, bounds, index, n) for n in names]
    write(args.out, result, anons_named)

    print(f"{len(result)} owners with parameters -> {args.out}")
    for want in args.spot:
        hits = {k: v for k, v in result.items() if want in k}
        for k, v in hits.items():
            print(f"  {k}: {v}")


def funco(bounds, names):
    return {n: bounds[n] for n in names if n in bounds}


def write(path, result, anons):
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(
            {k: {"params": v, "anons": anons.get(k, [])} for k, v in sorted(result.items())},
            fh,
            indent=1,
        )


if __name__ == "__main__":
    main()
