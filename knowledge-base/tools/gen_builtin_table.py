#!/usr/bin/env python3
"""
Bake the runtime's builtin names into a Rust table the profiler can use.

`builtins.py` recovers ~2,700 named runtime builtins by walking `Function_Add`'s call
sites. That name table exists only offline, so a profile of the running game reports
`sub_1b1db20` where it could report `audio_group_load` -- and an engine table full of
addresses is the thing that made the first profiler useless.

The names cannot be recovered cheaply at runtime (the walk needs the whole image), so
they are generated here and compiled in.

  python gen_builtin_table.py            write tkiw-runtime/src/builtins_table.rs

Re-run it when the game updates: every RVA here is build-specific, and a wrong name is
worse than no name. `Symbolizer` only trusts an entry that lands on a real `.pdata`
boundary, so a stale table degrades to anonymous rather than to fiction.
"""
import os
import sys

import builtins as _py_builtins  # noqa: F401  (guard against shadowing confusion)
import tkiw

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.normpath(os.path.join(HERE, "..", "..", "tkiw-runtime", "src", "builtins_table.rs"))


def main():
    sys.path.insert(0, HERE)
    import builtins as _  # noqa: F401
    from builtins import __name__ as _n  # noqa: F401
    # the analysis module, not python's
    import importlib.util
    spec = importlib.util.spec_from_file_location("tkiw_builtins", os.path.join(HERE, "builtins.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)

    pe = tkiw.PE(tkiw.find_image())
    table = mod.load(pe) if hasattr(mod, "load") else None
    if table is None:
        import pickle
        with open(os.path.join(HERE, "builtins.pickle"), "rb") as fh:
            table = pickle.load(fh)

    items = table.items() if hasattr(table, "items") else table
    rows = []
    for name, value in items:
        rva = value[0] if isinstance(value, (tuple, list)) else value
        if not isinstance(rva, int) or rva <= 0 or rva > 0xFFFFFFFF:
            continue
        if not name or any(c in name for c in '"\\'):
            continue
        rows.append((rva, name))
    rows.sort()

    with open(OUT, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("//! Names for the runtime's builtins, generated -- do not edit.\n")
        fh.write("//!\n")
        fh.write("//! Produced by `knowledge-base/tools/gen_builtin_table.py` from the\n")
        fh.write("//! `Function_Add` walk in `builtins.py`. Regenerate when the game updates:\n")
        fh.write("//! every RVA is build-specific.\n")
        fh.write("//!\n")
        fh.write("//! A profile that says `audio_group_load` instead of `sub_1b1db20` is the\n")
        fh.write("//! difference between a finding and an afternoon of disassembly.\n\n")
        fh.write("/// `(rva, name)`, sorted by rva.\n")
        fh.write("pub const BUILTINS: &[(u32, &str)] = &[\n")
        for rva, name in rows:
            fh.write('    (0x%x, "%s"),\n' % (rva, name))
        fh.write("];\n")
    print("wrote %s with %d entries" % (OUT, len(rows)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
