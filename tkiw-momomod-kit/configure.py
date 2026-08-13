#!/usr/bin/env python3
"""
Config window for TKIW's momomod Kit.  python configure.py

One tab per file in config/, one row per setting, Apply. The kit re-reads its
config while running, so applying takes effect without a restart.

No schema: the widget comes from the value already in the file (true/false ->
checkbox, digits -> number box, else text), so a new feature appears here with no
change to this file. Writes replace only the value after `=`, so comments survive.

Descriptions come from the ini comments the kit writes: the comment above a
section is the feature's, the comment above a key is that option's. Shown, never
restated here -- one place to correct when a setting's meaning changes.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CONFIG_DIR = os.path.join(HERE, "config")
KIT_FILE = "momomod.ini"

# Settings whose values are a fixed set rather than free text. Inferring a
# dropdown is not possible from one value, and getting `restore_on` wrong is a
# config error rather than a typo, so these few are worth naming.
CHOICES = {
    "restore_on": ["main_menu", "never"],
    "cap": ["per_factory", "total"],
}

# What a choice reads as in the window. The ini keeps the plain value, which is what
# the kit parses; only the label differs, so a hand-edited file and a file written
# from here are the same file.
LABELS = {
    ("cap", "per_factory"): "Cap +100 per Factory",
    ("cap", "total"): "Cap +100 total",
}


def label_of(key, value):
    """What the window shows for a value. The raw value if nobody named it."""
    return LABELS.get((key, value.strip()), value)


def value_of(key, label):
    """The value behind a label. The label itself if it is not one of ours."""
    for (k, v), shown in LABELS.items():
        if k == key and shown == label:
            return v
    return label

TRUE = ("true", "yes", "on", "1")
FALSE = ("false", "no", "off", "0")


# --------------------------------------------------------------------------
# reading and writing ini files, preserving everything we do not change
# --------------------------------------------------------------------------

class Setting:
    """One `key = value` line, with the comment block above it as its help."""

    def __init__(self, section, key, value, help_text, line_no):
        self.section = section
        self.key = key
        self.value = value
        self.help = help_text
        self.line_no = line_no

    @property
    def kind(self):
        v = self.value.strip().lower()
        if self.key in CHOICES:
            return "choice"
        # Numbers are tested before booleans, and that order matters: "1" and "0"
        # are legal spellings of true and false, so bool-first turned the
        # profiler's `interval_ms = 1` into a checkbox. A boolean written as 1
        # becoming a number box is the harmless direction to be wrong in, since
        # the kit accepts 1 and 0 for booleans anyway.
        if re.fullmatch(r"-?\d+", v):
            return "int"
        if v in TRUE or v in FALSE:
            return "bool"
        return "text"

    @property
    def truth(self):
        return self.value.strip().lower() in TRUE


def foreign(lines):
    """Whether this file is in some dialect other than the kit's `key = value`.

    A mod may write its own config in its own shape -- the reward picker's is 600
    lines of ordered preference lists, which this parser drops on the floor. Showing
    a form built from the 98 lines it *did* understand would be worse than showing
    nothing: it looks complete, and Apply would rewrite a file it never understood.
    """
    bare = 0
    for raw in lines:
        line = raw.strip()
        if not line or line.startswith(("#", ";", "[")):
            continue
        if "=" not in line.split("#")[0]:
            bare += 1
    return bare > 10


def parse(path):
    """[(section, [Setting])] in file order, plus the raw lines."""
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().splitlines()

    sections, current, comment = [], None, []
    for n, raw in enumerate(lines):
        line = raw.strip()
        if not line:
            comment = []
            continue
        if line.startswith("#") or line.startswith(";"):
            comment.append(line.lstrip("#; ").rstrip())
            continue
        if line.startswith("[") and line.endswith("]"):
            current = line[1:-1].strip()
            sections.append((current, " ".join(comment), []))
            comment = []
            continue
        if "=" in line and current is not None:
            key, value = line.split("=", 1)
            # a trailing comment is part of neither the key nor the value
            value = value.split("#")[0].split(";")[0].strip()
            sections[-1][2].append(
                Setting(current, key.strip(), value, " ".join(comment), n)
            )
        comment = []
    return sections, lines


def write_values(path, changes):
    """Replace the value on specific lines. Everything else is untouched.

    `changes` is `{line_no: new_value}`. Rewriting by line number rather than by
    searching for the key means a key that appears in two sections cannot be
    confused, and a file with an unusual layout survives a round trip.
    """
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    for n, value in changes.items():
        if n >= len(lines):
            continue
        key = lines[n].split("=", 1)[0]
        lines[n] = "%s= %s" % (key, value)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(lines) + "\n")


# --------------------------------------------------------------------------
# the window
# --------------------------------------------------------------------------

def build(tk, ttk):
    class Scrollable(ttk.Frame):
        """A frame that scrolls, because a mod may have more settings than fit.

        The inner frame is the one to put widgets in; `body` is it.
        """

        def __init__(self, parent):
            super().__init__(parent)
            canvas = tk.Canvas(self, borderwidth=0, highlightthickness=0)
            bar = ttk.Scrollbar(self, orient="vertical", command=canvas.yview)
            self.body = ttk.Frame(canvas)
            window = canvas.create_window((0, 0), window=self.body, anchor="nw")

            canvas.configure(yscrollcommand=bar.set)
            canvas.pack(side="left", fill="both", expand=True)
            bar.pack(side="right", fill="y")

            def resize(_event=None):
                canvas.configure(scrollregion=canvas.bbox("all"))
                canvas.itemconfigure(window, width=canvas.winfo_width())

            self.body.bind("<Configure>", resize)
            canvas.bind("<Configure>", resize)

            # Wheel scrolling only while the pointer is over this canvas -- a
            # global binding would scroll whichever tab happened to be built last.
            def wheel(event):
                canvas.yview_scroll(-1 * (event.delta // 120), "units")

            canvas.bind("<Enter>", lambda _e: canvas.bind_all("<MouseWheel>", wheel))
            canvas.bind("<Leave>", lambda _e: canvas.unbind_all("<MouseWheel>"))

    class App(ttk.Frame):
        def __init__(self, root):
            super().__init__(root, padding=8)
            self.root = root
            self.pack(fill="both", expand=True)
            # {path: {line_no: tk variable}}
            self.vars = {}
            self.settings = {}

            self.tabs = ttk.Notebook(self)
            self.tabs.pack(fill="both", expand=True)

            row = ttk.Frame(self)
            row.pack(fill="x", pady=(6, 0))
            self.status = ttk.Label(row, text="", foreground="#777")
            self.status.pack(side="left")
            ttk.Button(row, text="Apply", command=self.apply).pack(side="right")

            self.reload()

        # -- building ------------------------------------------------------

        def files(self):
            """The kit's file first, then the mods in the order the kit lists them.

            The order comes from `[mods]` in the kit's own file rather than from a
            list kept here, so the tabs match the kit and there is no second
            ordering to forget to update.
            """
            if not os.path.isdir(CONFIG_DIR):
                return []
            names = sorted(
                f for f in os.listdir(CONFIG_DIR)
                if f.endswith(".ini") and not f.endswith(".reference.ini")
            )
            order = []
            if KIT_FILE in names:
                names.remove(KIT_FILE)
                order.append(KIT_FILE)
                for section, _blurb, settings in parse(os.path.join(CONFIG_DIR, KIT_FILE))[0]:
                    if section.lower() != "mods":
                        continue
                    for s in settings:
                        wanted = s.key + ".ini"
                        if wanted in names:
                            names.remove(wanted)
                            order.append(wanted)
            # anything the kit did not mention still gets a tab, alphabetically
            return [os.path.join(CONFIG_DIR, n) for n in order + names]

        def reload(self):
            for tab in self.tabs.tabs():
                self.tabs.forget(tab)
            self.vars.clear()
            self.settings.clear()

            paths = self.files()
            if not paths:
                frame = ttk.Frame(self.tabs, padding=16)
                ttk.Label(frame, text="No config/ yet — launch the game once.").pack(anchor="w")
                self.tabs.add(frame, text="—")
                self.status.config(text=CONFIG_DIR)
                return

            for path in paths:
                self.add_tab(path)
            self.status.config(text="")

        def add_tab(self, path):
            name = os.path.basename(path)
            title = "Kit" if name == KIT_FILE else os.path.splitext(name)[0]
            sections, lines = parse(path)
            self.settings[path] = sections
            self.vars[path] = {}

            page = Scrollable(self.tabs)
            self.tabs.add(page, text=title)
            body = page.body

            if foreign(lines):
                ttk.Label(
                    body,
                    text="This mod writes its own config, in its own format.",
                    wraplength=460,
                ).pack(anchor="w", padx=8, pady=(12, 4))
                ttk.Label(body, text=path, foreground="#777", wraplength=460).pack(
                    anchor="w", padx=8
                )
                ttk.Button(
                    body, text="Open", command=lambda p=path: open_in_editor(p)
                ).pack(anchor="w", padx=8, pady=8)
                self.vars.pop(path, None)
                return

            for section, blurb, settings in sections:
                # The mirror is a copy of the other tabs; editing it here would be
                # editing the same setting in two places, so it is not shown.
                if ".feature." in section:
                    continue
                if not settings:
                    continue
                ttk.Label(body, text=pretty(section), font=("", 9, "bold")).pack(
                    anchor="w", pady=(8, 0)
                )
                # The comment above the section is the feature's own description, as
                # the kit wrote it. Shown, not restated: one place to correct.
                if blurb:
                    ttk.Label(
                        body, text=blurb, wraplength=460, foreground="#666"
                    ).pack(anchor="w", pady=(0, 3))
                for s in settings:
                    self.add_row(body, path, s)

            if not body.winfo_children():
                ttk.Label(body, text="nothing to set").pack(anchor="w", padx=8, pady=8)

        def add_row(self, parent, path, setting):
            row = ttk.Frame(parent)
            row.pack(fill="x", padx=12)

            if setting.kind == "bool":
                var = tk.BooleanVar(value=setting.truth)
                ttk.Checkbutton(row, text=setting.key, variable=var).pack(side="left")
            else:
                # A dropdown says what it is; the key beside it would be the same thing
                # twice. Every other kind still needs naming.
                if setting.kind != "choice":
                    ttk.Label(row, text=setting.key, width=18).pack(side="left")
                var = tk.StringVar(value=setting.value)
                if setting.kind == "choice":
                    # The variable holds the label, not the value; `apply` turns it
                    # back. A key with no labels shows its raw values, unchanged.
                    options = [label_of(setting.key, v) for v in CHOICES[setting.key]]
                    var.set(label_of(setting.key, setting.value))
                    ttk.Combobox(
                        row,
                        textvariable=var,
                        values=options,
                        state="readonly",
                        # Wide enough for the longest option rather than a fixed
                        # guess, so adding a longer one later does not clip it.
                        width=max(len(o) for o in options) + 1,
                    ).pack(side="left")
                elif setting.kind == "int":
                    ttk.Spinbox(row, textvariable=var, from_=0, to=10_000_000, width=10).pack(
                        side="left"
                    )
                else:
                    ttk.Entry(row, textvariable=var, width=22).pack(side="left")

            # The key's own comment, on the same line. It is a short phrase by
            # convention -- ten words or so -- so it needs no wrapping and no room
            # of its own. A long one wraps rather than widening the window.
            if setting.help:
                ttk.Label(
                    row, text=setting.help, wraplength=300, foreground="#777"
                ).pack(side="left", padx=(8, 0))

            self.vars[path][setting.line_no] = (var, setting)

        # -- applying ------------------------------------------------------

        def apply(self):
            written = 0
            for path, entries in self.vars.items():
                changes = {}
                for line_no, (var, setting) in entries.items():
                    new = str(var.get())
                    if setting.kind == "bool":
                        new = "true" if var.get() else "false"
                    elif setting.kind == "choice":
                        # The combobox holds a label; the file keeps the value.
                        new = value_of(setting.key, new)
                    if new.strip() != setting.value.strip():
                        changes[line_no] = new.strip()
                if changes:
                    write_values(path, changes)
                    written += len(changes)
            self.status.config(text="applied %d" % written if written else "")
            if written:
                self.reload()

    return App


def open_in_editor(path):
    """Hand the file to whatever the system opens .ini with."""
    try:
        os.startfile(path)  # Windows only, which is where the game is
    except (AttributeError, OSError):
        pass


def pretty(section):
    """`feature.fast_boot` -> `Fast boot`; `mods` -> `Mods`."""
    name = section.split(".")[-1]
    return name.replace("_", " ").capitalize()


def main():
    try:
        import tkinter as tk
        from tkinter import ttk
    except ImportError:
        print("tkinter is not available in this Python.", file=sys.stderr)
        print("The config files are plain text; edit them in any editor:", file=sys.stderr)
        print("   ", CONFIG_DIR, file=sys.stderr)
        return 1

    root = tk.Tk()
    root.title("TKIW's momomod Kit")
    root.minsize(560, 420)
    try:
        root.call("ttk::style", "theme", "use", "vista")
    except tk.TclError:
        pass

    App = build(tk, ttk)
    App(root)
    root.mainloop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
