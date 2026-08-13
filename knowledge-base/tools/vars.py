#!/usr/bin/env python3
"""
Search the game's variable-name table.

The most reliable index into this game. Variable names survive compilation, and what a
function reads tells you what it is for far more reliably than its call list, which is
mostly unnamed runtime helpers.

  python vars.py training              variables matching a regex
  python vars.py --who "^damage_multi$" ... and which functions touch each
  python vars.py --of gml_Object_obj_unit_parent_Create_0     one function's variables
  python vars.py --object obj_improvement_parent              one object's variables
"""
import re
import sys

sys.path.insert(0, __file__.rsplit("\\", 1)[0] if "\\" in __file__ else ".")
import tkiw
from index import Index


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    if not args:
        print(__doc__)
        return 1

    ix = Index.load()

    if "--of" in flags or "--object" in flags:
        needle = args[0]
        if "--object" in flags:
            fns = [f for f in ix.func_vars if needle in f]
            print(f"{len(fns)} function(s) mentioning {needle}")
        else:
            fns = [f for f in ix.func_vars if needle in f]
            if needle in ix.func_vars:
                fns = [needle]
            elif len(fns) != 1:
                print(f"{len(fns)} matches:")
                for c in sorted(fns)[:30]:
                    print("  ", c)
                return 1
        seen = set()
        for f in fns:
            seen |= set(ix.func_vars.get(f, ()))
        for v in sorted(seen):
            print(f"  {v}")
        print(f"\n{len(seen)} distinct variables")
        return 0

    pe = tkiw.load()
    names = sorted(set(tkiw.gml_variable_slots(pe).values()))
    for pat in args:
        rx = re.compile(pat, re.I)
        hits = [n for n in names if rx.search(n)]
        print(f"===== {pat}  ({len(hits)} variables)")
        for n in hits[:60]:
            if "--who" in flags:
                fs = sorted(ix.var_funcs.get(n, ()))
                short = [x.replace("gml_Object_", "").replace("gml_Script_", "")
                         for x in fs]
                print(f"  {n:44} {len(fs):3} fn  {short[:4]}")
            else:
                print(f"  {n}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
