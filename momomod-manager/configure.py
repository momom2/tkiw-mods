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

# Where a mod keeps its whole-mod switch when it is not the usual `[mod] enabled`.
#
# A self-configuring mod writes its own file in its own shape, so the switch is
# wherever that mod put it. The auto-picker's `[global] enabled` is whether the
# mod runs at all -- not `act`, which is the separate "press buttons or just log
# what I would press" toggle that Ctrl+Alt+P flips mid-game.
FOREIGN_MASTER = {"reward-picker": ("global", "enabled")}

# A comment line starting with this marks the rest of its section as settings for
# working on the mod rather than using it: kept in the file, kept working, but not
# shown in this window. The generator writes the marker, so which keys are advanced
# is decided in one place (the Rust that renders the config), not here.
ADVANCED_MARK = "advanced:"

# The key whose value names a diagnostic's output file, relative to the mod folder.
OUTPUT_KEY = "file"

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

    def __init__(self, section, key, value, help_text, line_no, advanced=False):
        self.section = section
        self.key = key
        self.value = value
        self.help = help_text
        self.line_no = line_no
        # Set for keys below an `# advanced:` marker in their section. They stay in
        # the file and the mod still reads them; this window just does not show
        # them, because they are for working on the diagnostic rather than using it.
        self.advanced = advanced

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
    advanced = False
    for n, raw in enumerate(lines):
        line = raw.strip()
        if not line:
            comment = []
            continue
        if line.startswith("#") or line.startswith(";"):
            text = line.lstrip("#; ").rstrip()
            # Everything after this marker, to the end of the section, is for
            # working on the mod rather than using it. See Setting.advanced.
            if text.lower().startswith(ADVANCED_MARK):
                advanced = True
            comment.append(text)
            continue
        if line.startswith("[") and line.endswith("]"):
            current = line[1:-1].strip()
            sections.append((current, " ".join(comment), []))
            comment, advanced = [], False
            continue
        if "=" in line and current is not None:
            key, value = line.split("=", 1)
            # a trailing comment is part of neither the key nor the value
            value = value.split("#")[0].split(";")[0].strip()
            sections[-1][2].append(
                Setting(current, key.strip(), value, " ".join(comment), n, advanced)
            )
        comment = []
    return sections, lines


def file_blurb(lines):
    """The mod's own description: the second paragraph of the file's header.

    The generator writes `# <title>`, then the mod's description, then boilerplate,
    so the description is readable here rather than kept a second time in Python."""
    paragraphs, current = [], []
    for raw in lines:
        line = raw.strip()
        if line.startswith("[") or (line and not line.startswith(("#", ";"))):
            break
        text = line.lstrip("#; ").rstrip()
        if text:
            current.append(text)
        elif current:
            paragraphs.append(" ".join(current))
            current = []
    if current:
        paragraphs.append(" ".join(current))
    return paragraphs[1] if len(paragraphs) > 1 else ""


# `hidden` -- a third state beside true and false, which hid a mod's tab as well
# as switching it off -- has been retired. It suppressed the display of something
# that might still be running (it never governed the plugins the manager loads),
# so a mod could act while this window insisted it did not exist. A mod nobody
# should meet yet is now simply absent from the catalogue and the release. Any
# `hidden` left in a config still reads as off; it just no longer hides anything.


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
            # Whole-mod master switches: (var, file, line_no, original value). Kept
            # apart from `vars` because a switch may live in a different file than
            # the tab it appears on (an internal mod's is in the manager's file).
            self.masters = []

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
            """One tab per mod, in the order the manager's file lists them.

            The manager's own file (`momomod.ini`) is **not** shown: it holds only
            developer settings (tracing, and which internal tools to load), nothing
            a player configures. It is still read here, just to order the mod tabs
            the way the manager lists them, so there is no second ordering to forget
            to update.

            **Every mod with a config file gets a tab.** Nothing is suppressed: a
            mod that is installed and possibly running must be visible, or the
            window becomes a thing that lies about what the game is doing.
            """
            if not os.path.isdir(CONFIG_DIR):
                return []
            names = sorted(
                f for f in os.listdir(CONFIG_DIR)
                if f.endswith(".ini") and not f.endswith(".reference.ini")
            )
            order = []
            if KIT_FILE in names:
                # Read but not shown: take its [mods] order, give it no tab.
                names.remove(KIT_FILE)
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
            self.masters.clear()

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

        def select_tab(self, name):
            """Bring the tab whose title matches `name` to the front, if there is
            one -- so the mod manager can open this window straight on a mod."""
            for tab_id in self.tabs.tabs():
                if self.tabs.tab(tab_id, "text").lower() == name.lower():
                    self.tabs.select(tab_id)
                    return True
            return False

        def add_tab(self, path):
            modname = os.path.splitext(os.path.basename(path))[0]
            sections, lines = parse(path)
            self.settings[path] = sections
            self.vars[path] = {}

            page = Scrollable(self.tabs)
            self.tabs.add(page, text=modname)
            body = page.body

            if foreign(lines):
                # No form for this one -- but it still gets the same whole-mod
                # switch every other tab has, since "is this mod on" is a question
                # the window can answer whatever dialect the rest of the file is in.
                blurb = catalog_blurb(modname)
                if blurb:
                    ttk.Label(body, text=blurb, wraplength=460, foreground="#666").pack(
                        anchor="w", padx=8, pady=(12, 0)
                    )
                master_var, target, master_setting = self.master_of(path, modname, sections)
                if master_var is not None:
                    self.masters.append((master_var, target[0], target[1], master_setting.value))
                    ttk.Checkbutton(
                        body, text="Enable this mod", variable=master_var
                    ).pack(anchor="w", padx=8, pady=(10, 2))
                    ttk.Separator(body, orient="horizontal").pack(fill="x", pady=(2, 6))

                ttk.Label(
                    body,
                    text="Its other settings live in its own file, in its own format.",
                    wraplength=460,
                ).pack(anchor="w", padx=8, pady=(4, 4))
                ttk.Label(body, text=path, foreground="#777", wraplength=460).pack(
                    anchor="w", padx=8
                )
                ttk.Button(
                    body, text="Open", command=lambda p=path: open_in_editor(p)
                ).pack(anchor="w", padx=8, pady=8)
                self.vars.pop(path, None)
                return

            # What this mod is, in one line: from the catalogue for a published mod,
            # otherwise from its own file's header. Either way it is written once,
            # where the mod is defined, and only shown here.
            intro = catalog_blurb(modname) or file_blurb(lines)
            if intro:
                ttk.Label(body, text=intro, wraplength=460, foreground="#666").pack(
                    anchor="w", padx=8, pady=(10, 0)
                )

            # The whole-mod switch goes first; every feature row below greys out
            # while it is off. A published mod keeps the switch in its own
            # [mod] enabled; an internal one keeps it in the manager's [mods].
            master_var, target, master_setting = self.master_of(path, modname, sections)
            feature_widgets = []
            if master_var is not None:
                self.masters.append((master_var, target[0], target[1], master_setting.value))
                ttk.Checkbutton(
                    body, text="Enable this mod", variable=master_var,
                    command=lambda: self.set_features_state(feature_widgets, master_var.get()),
                ).pack(anchor="w", padx=8, pady=(10, 2))
                ttk.Separator(body, orient="horizontal").pack(fill="x", pady=(2, 6))

            for section, blurb, settings in sections:
                # The mirror sections (kit file) and the [mod]/[mods] master -- shown
                # as the switch above -- are not repeated as rows here.
                if ".feature." in section or section.lower() in ("mod", "mods"):
                    continue
                if not settings:
                    continue
                head = ttk.Label(body, text=pretty(section), font=("", 9, "bold"))
                head.pack(anchor="w", pady=(8, 0))
                feature_widgets.append(head)
                # The comment above the section is the feature's own description, as
                # the mod wrote it. Shown, not restated: one place to correct.
                if blurb:
                    bl = ttk.Label(body, text=blurb, wraplength=460, foreground="#666")
                    bl.pack(anchor="w", pady=(0, 3))
                    feature_widgets.append(bl)
                for s in settings:
                    if s.advanced:
                        continue
                    feature_widgets.extend(self.add_row(body, path, s))
                # A feature with settings this window does not show still needs a way
                # to reach them, rather than pretending the file holds nothing more.
                if any(s.advanced for s in settings):
                    more = ttk.Frame(body)
                    more.pack(fill="x", padx=12, pady=(2, 0))
                    btn = ttk.Button(
                        more, text="Open config", width=12,
                        command=lambda p=path: open_in_editor(p),
                    )
                    btn.pack(side="left")
                    note = ttk.Label(
                        more, text="Further settings in the mod's config file.",
                        foreground="#777", wraplength=300,
                    )
                    note.pack(side="left", padx=(8, 0))
                    feature_widgets.extend([btn, note])

            if not body.winfo_children():
                ttk.Label(body, text="nothing to set").pack(anchor="w", padx=8, pady=8)
            elif master_var is not None:
                # Reflect the switch's starting state on the rows.
                self.set_features_state(feature_widgets, master_var.get())

        def master_of(self, path, modname, sections):
            """The whole-mod switch for this tab: (BooleanVar, (file, line_no),
            setting), or (None, None, None) if there is none to show.

            A published mod keeps it in its own `[mod] enabled`; a self-configuring
            one wherever FOREIGN_MASTER says; an internal mod keeps it in the
            manager file's `[mods]`, under the mod's name."""
            want_section, want_key = FOREIGN_MASTER.get(modname.lower(), ("mod", "enabled"))
            for section, _blurb, settings in sections:
                if section.lower() == want_section:
                    for s in settings:
                        if s.key.lower() == want_key:
                            return tk.BooleanVar(value=s.truth), (path, s.line_no), s
            kit_path = os.path.join(CONFIG_DIR, KIT_FILE)
            if os.path.isfile(kit_path):
                for section, _blurb, settings in parse(kit_path)[0]:
                    if section.lower() != "mods":
                        continue
                    for s in settings:
                        if s.key.lower() == modname.lower():
                            return tk.BooleanVar(value=s.truth), (kit_path, s.line_no), s
            return None, None, None

        def set_features_state(self, widgets, enabled):
            """Grey out (and lock) the feature widgets when the mod is off."""
            for w in widgets:
                if w.winfo_class() == "TLabel":
                    if not hasattr(w, "_fg0"):
                        w._fg0 = str(w.cget("foreground"))
                    w.configure(foreground=(w._fg0 or "") if enabled else "#b0b0b0")
                else:
                    # `!disabled` keeps a combobox's `readonly` flag; it only clears
                    # the disabled one, so re-enabling does not make it editable.
                    try:
                        w.state(["!disabled"] if enabled else ["disabled"])
                    except tk.TclError:
                        pass

        def add_row(self, parent, path, setting):
            """Build one setting's row. Returns the widgets in it, so the tab can
            grey them out when the mod's master switch is off."""
            row = ttk.Frame(parent)
            row.pack(fill="x", padx=12)
            made = []

            if setting.kind == "bool":
                var = tk.BooleanVar(value=setting.truth)
                w = ttk.Checkbutton(row, text=setting.key, variable=var)
                w.pack(side="left")
                made.append(w)
            else:
                # A dropdown says what it is; the key beside it would be the same thing
                # twice. Every other kind still needs naming.
                if setting.kind != "choice":
                    lbl = ttk.Label(row, text=setting.key, width=18)
                    lbl.pack(side="left")
                    made.append(lbl)
                var = tk.StringVar(value=setting.value)
                if setting.kind == "choice":
                    # The variable holds the label, not the value; `apply` turns it
                    # back. A key with no labels shows its raw values, unchanged.
                    options = [label_of(setting.key, v) for v in CHOICES[setting.key]]
                    var.set(label_of(setting.key, setting.value))
                    w = ttk.Combobox(
                        row,
                        textvariable=var,
                        values=options,
                        state="readonly",
                        # Wide enough for the longest option rather than a fixed
                        # guess, so adding a longer one later does not clip it.
                        width=max(len(o) for o in options) + 1,
                    )
                elif setting.kind == "int":
                    w = ttk.Spinbox(row, textvariable=var, from_=0, to=10_000_000, width=10)
                else:
                    w = ttk.Entry(row, textvariable=var, width=22)
                w.pack(side="left")
                made.append(w)

            # The key's own comment, on the same line. It is a short phrase by
            # convention -- ten words or so -- so it needs no wrapping and no room
            # of its own. A long one wraps rather than widening the window.
            if setting.help:
                h = ttk.Label(row, text=setting.help, wraplength=300, foreground="#777")
                h.pack(side="left", padx=(8, 0))
                made.append(h)

            # A diagnostic's whole point is its output file, so offer to open it
            # from here. Greyed until the file exists, since a diagnostic that has
            # never run has nothing to show and a dead button would say otherwise.
            if setting.key.lower() == OUTPUT_KEY:
                out = os.path.join(HERE, setting.value.strip())
                exists = os.path.isfile(out)
                row2 = ttk.Frame(parent)
                row2.pack(fill="x", padx=12, pady=(1, 0))
                btn = ttk.Button(
                    row2, text="Open output", width=12,
                    command=lambda p=out: open_in_editor(p),
                )
                if not exists:
                    btn.state(["disabled"])
                btn.pack(side="left")
                note = ttk.Label(
                    row2,
                    text="See the results." if exists else "Run this diagnostic first.",
                    foreground="#777", wraplength=300,
                )
                note.pack(side="left", padx=(8, 0))
                made.append(note)
                # The button is deliberately **not** greyed with the rest: it opens a
                # report the diagnostic already produced, and reading last run's
                # results is exactly what you do while the thing is switched off. Its
                # own state says the only thing that matters -- whether a file exists.

            self.vars[path][setting.line_no] = (var, setting)
            return made

        # -- applying ------------------------------------------------------

        def apply(self):
            written = 0
            # The master switches first, gathered per file: an internal mod's lives
            # in the manager's file, not in the tab's own.
            master_changes = {}
            for var, mpath, line_no, was in self.masters:
                new = "true" if var.get() else "false"
                if new != was.strip().lower():
                    master_changes.setdefault(mpath, {})[line_no] = new
            for mpath, changes in master_changes.items():
                write_values(mpath, changes)
                written += len(changes)

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


def catalog_blurb(modname):
    """A published mod's one-line description, from `catalog.json`.

    The same file the mod manager's list is built from, so a mod is described in
    one place and the two windows cannot disagree. Missing or unreadable
    catalogue just means no description here -- never an error."""
    try:
        import json

        with open(os.path.join(HERE, "catalog.json"), encoding="utf-8") as fh:
            for mod in json.load(fh).get("mods", []):
                if str(mod.get("name", "")).lower() == modname.lower():
                    return mod.get("blurb") or ""
    except (OSError, ValueError):
        pass
    return ""


def open_in_editor(path):
    """Hand the file to whatever the system opens .ini with."""
    try:
        os.startfile(path)  # Windows only, which is where the game is
    except (AttributeError, OSError):
        pass


def pretty(section):
    """`feature.popup_stutter_fix` -> `Popup stutter fix`; `mods` -> `Mods`."""
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
    root.title("momom2's mod manager")
    root.minsize(560, 420)
    try:
        root.call("ttk::style", "theme", "use", "vista")
    except tk.TclError:
        pass

    App = build(tk, ttk)
    app = App(root)
    # Optional: a mod name to open on, passed by the mod manager's Configure button.
    if len(sys.argv) > 1:
        app.select_tab(sys.argv[1])
    root.mainloop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
