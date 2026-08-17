#!/usr/bin/env python3
"""
The momomod mod manager.  python manage-mods.py

Two windows, two jobs. This one is about *having* a mod at all: it lists the
mods momomod supports, downloads the ones you want into the `mods/` folder, and
removes the ones you don't. The other window -- `configure.py` -- is about
*settings*: enabling, disabling and tuning the mods you have installed.

A mod is "installed" when its DLL is in `mods/` beside this script; momomod
loads whatever it finds there the next time the game starts. So installing is a
download, and uninstalling is a delete.

## Where mods come from

The catalogue and the mod DLLs are fetched from the momomod release on GitHub.
Set REPO below to the repository that hosts them. Until it is set, the manager
runs in developer mode and installs from the locally built DLLs in
`../target/release`, so the flow can be exercised without a release.
"""
import json
import os
import shutil
import sys
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
MODS_DIR = os.path.join(HERE, "mods")
CONFIG_DIR = os.path.join(HERE, "config")

# What "install and enable" means for a mod: which switches to turn on in the
# mod's own default config.
#
# **Values only, never prose.** The config document itself -- every description,
# every default -- is rendered by the mod and shipped with the release as
# `<mod>.default.ini`; this only says which lines "and enable" flips. Keeping a
# copy of the document here is what made a reworded description silently fail to
# reach anybody, so it is not kept here.
#
# `(section, key): value`. A mod absent from this map installs dormant. A
# self-configuring mod (the auto-picker, whose file is built from the live game,
# and whose default -- 0 rerolls, everything blacklisted -- is already its
# enabled-but-passive state) is deliberately absent.
ENABLE_OVERRIDES = {
    "bugfixes": {
        ("mod", "enabled"): "true",
        # Defaults off, since it changes the rules of a save already running;
        # asking for it enabled is what "and enable" means.
        ("feature.fortifications_cap", "enabled"): "true",
    },
}

# Per mod, where its master on/off switch lives in its config: (section, key).
# The framework mods use `[mod] enabled`; the self-configuring auto-picker keeps
# its own in `[global] enabled`. The mod manager reads and flips this without
# needing to understand the rest of the file.
#
# The picker's switch is `enabled` (does the mod run) rather than `act` (press
# buttons, or only log what it would press). `act` is the in-game Ctrl+Alt+P
# toggle and stays a setting in its file; a checkbox labelled "enabled" in two
# windows must mean the same thing in both, and that thing is the master switch.
ENABLE_KEY = {
    "bugfixes": ("mod", "enabled"),
    "reward-picker": ("global", "enabled"),
}

# ---- where mods are published -------------------------------------------------
# The GitHub repository that hosts the momomod release. Its mods and the
# catalogue are release assets, so they download without any credentials as long
# as the release is public.
REPO = "momom2/tkiw-mods"

# The download base. Overridable for testing against a local server or a
# specific release, e.g.  MOMOMOD_MODS_BASE=http://localhost:8000
BASE = os.environ.get("MOMOMOD_MODS_BASE", "https://github.com/%s/releases/latest/download" % REPO)
CATALOG_URL = BASE + "/catalog.json"


# ---- the catalogue ------------------------------------------------------------
def load_catalog():
    """The supported mods: [{name, title, blurb, ...}]. From the release, or the
    bundled copy if that cannot be reached."""
    try:
        with urllib.request.urlopen(CATALOG_URL, timeout=10) as r:
            return json.loads(r.read().decode("utf-8"))["mods"], None
    except (urllib.error.URLError, ValueError, KeyError) as e:
        note = "could not fetch the catalogue (%s); showing the bundled one" % e
    local = os.path.join(HERE, "catalog.json")
    if os.path.isfile(local):
        with open(local, encoding="utf-8") as fh:
            return json.load(fh)["mods"], note
    return [], "no catalogue found (looked at %s and %s)" % (CATALOG_URL, local)


def is_installed(name):
    return os.path.isfile(os.path.join(MODS_DIR, name + ".dll"))


# Mod name -> the crate's built DLL, for the developer fallback below. This map
# exists only in a source checkout; a player never reaches it.
_DEV_BUILT_DLL = {
    "reward-picker": "tkiw_reward_picker_plugin.dll",
    "bugfixes": "tkiw_bugfixes_plugin.dll",
}


def _local_build(name):
    """The crate's built DLL under ../target/release, if this is a source
    checkout. `None` in a player's install, where no such folder exists."""
    built = _DEV_BUILT_DLL.get(name)
    if not built:
        return None
    path = os.path.join(HERE, "..", "target", "release", built)
    return path if os.path.isfile(path) else None


def install(name, enable=False):
    """Put <name>.dll into mods/, downloaded from the release.

    With `enable`, also write the mod's enabled-default config, so it comes up on
    rather than installed-but-off -- unless it configures itself, or already has
    a config the player has tuned, in which case that is left alone.

    Returns a short description of what happened, or raises on failure. In a
    source checkout, a failed download falls back to the locally built DLL, so
    the manager is testable before a release exists; a player's install has no
    build to fall back to and simply reports the download error.
    """
    os.makedirs(MODS_DIR, exist_ok=True)
    dst = os.path.join(MODS_DIR, name + ".dll")
    tmp = dst + ".part"
    url = "%s/%s.dll" % (BASE, name)
    try:
        with urllib.request.urlopen(url, timeout=30) as r, open(tmp, "wb") as out:
            shutil.copyfileobj(r, out)
        source = "downloaded"
    except urllib.error.URLError as download_err:
        built = _local_build(name)
        if built is None:
            raise RuntimeError("download failed (%s):\n    %s" % (download_err, url))
        shutil.copyfile(built, tmp)
        source = "installed from the local build (the release was not reachable)"
    os.replace(tmp, dst)  # atomic: a half-written DLL is never left in mods/

    if enable and name in ENABLE_OVERRIDES:
        cfg = os.path.join(CONFIG_DIR, name + ".ini")
        if os.path.isfile(cfg):
            source += "; kept your existing settings"
        else:
            source += "; " + _write_enabled_config(name, cfg)
    return source


def _write_enabled_config(name, cfg):
    """Write the mod's own default config with its enable-switches turned on.

    The document comes from the mod (shipped as `<mod>.default.ini`); only the
    values in ENABLE_OVERRIDES are changed. Returns a short account of what
    happened, including when it could not be done -- a mod that quietly installs
    dormant after you asked for it enabled is the confusing case worth naming.
    """
    text = _default_config(name)
    if text is None:
        return ("could not fetch its settings file, so it is installed but OFF"
                " -- switch it on here after launching the game once")
    text, applied = _with_overrides(text, ENABLE_OVERRIDES[name])
    os.makedirs(CONFIG_DIR, exist_ok=True)
    with open(cfg, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
    missing = len(ENABLE_OVERRIDES[name]) - applied
    if missing:
        # The settings file no longer has a line we expected to switch on: say so
        # rather than report "enabled" for a mod that is partly off.
        return ("enabled, but %d setting(s) it expected were not in the file"
                " -- check them here" % missing)
    return "enabled"


def _default_config(name):
    """The mod's default config: downloaded from the release, or -- in a source
    checkout -- read from the staged release. `None` if neither is available."""
    try:
        with urllib.request.urlopen("%s/%s.default.ini" % (BASE, name), timeout=30) as r:
            return r.read().decode("utf-8")
    except urllib.error.URLError:
        staged = os.path.join(HERE, "..", "dist", "release", name + ".default.ini")
        if os.path.isfile(staged):
            with open(staged, encoding="utf-8") as fh:
                return fh.read()
        return None


def _with_overrides(text, overrides):
    """`text` with the given `(section, key): value` settings applied.

    Line-surgical, like every other write here: only what follows `=` changes, so
    the document stays exactly the one the mod rendered. Returns the new text and
    how many overrides actually landed."""
    lines = text.splitlines()
    section, applied = None, 0
    for i, line in enumerate(lines):
        bare = line.split("#")[0].split(";")[0].strip()
        if bare.startswith("[") and bare.endswith("]"):
            section = bare[1:-1].strip().lower()
            continue
        if "=" not in bare:
            continue
        key = bare.split("=", 1)[0].strip().lower()
        want = overrides.get((section, key))
        if want is not None:
            lines[i] = "%s= %s" % (line.split("=", 1)[0], want)
            applied += 1
    return "\n".join(lines) + "\n", applied


def uninstall(name):
    path = os.path.join(MODS_DIR, name + ".dll")
    if os.path.isfile(path):
        os.remove(path)


# ---- per-mod enable switch, read and flipped in the mod's own config ----------
_TRUE = ("true", "yes", "on", "1")


def config_path(name):
    return os.path.join(CONFIG_DIR, name + ".ini")


def read_enabled(name):
    """The mod's master switch: True, False, or None if there is no config yet
    (the mod writes it the first time the game runs) or no known switch."""
    key = ENABLE_KEY.get(name)
    path = config_path(name)
    if not key or not os.path.isfile(path):
        return None
    section, want = key
    in_section = False
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            s = line.split("#")[0].split(";")[0].strip()
            if s.startswith("[") and s.endswith("]"):
                in_section = s[1:-1].strip().lower() == section
            elif in_section and "=" in s:
                k, v = s.split("=", 1)
                if k.strip().lower() == want:
                    return v.strip().lower() in _TRUE
    return None


def set_enabled(name, value):
    """Flip the mod's master switch in its config, line-surgically -- comments,
    layout and every other setting are left exactly as they were. Returns whether
    the switch was found and written."""
    key = ENABLE_KEY.get(name)
    path = config_path(name)
    if not key or not os.path.isfile(path):
        return False
    section, want = key
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    in_section = False
    for i, line in enumerate(lines):
        s = line.split("#")[0].split(";")[0].strip()
        if s.startswith("[") and s.endswith("]"):
            in_section = s[1:-1].strip().lower() == section
        elif in_section and "=" in line and line.split("=", 1)[0].strip().lower() == want:
            head, rest = line.split("=", 1)
            comment = ""
            for c in ("#", ";"):
                if c in rest:
                    comment = "  " + rest[rest.index(c):].rstrip()
                    break
            lines[i] = "%s= %s%s" % (head, "true" if value else "false", comment)
            with open(path, "w", encoding="utf-8", newline="\n") as fh:
                fh.write("\n".join(lines) + "\n")
            return True
    return False


def open_config(name):
    """Open the mod's config file in whatever the system uses for .ini."""
    path = config_path(name)
    if not os.path.isfile(path):
        return False
    try:
        os.startfile(path)  # Windows, which is where the game runs
        return True
    except (AttributeError, OSError):
        return False


def open_configurer(name):
    """Open the settings window (configure.py) on this mod's tab.

    For a mod the kit can render as a form -- everything except the
    self-configuring auto-picker, whose file is generated from the live game and
    has no form. Returns whether the window was launched; the caller falls back
    to the raw file if not."""
    import subprocess

    script = os.path.join(HERE, "configure.py")
    if not os.path.isfile(script):
        return False
    try:
        # Non-blocking: the settings window is its own program, so this window
        # stays usable while it is open.
        subprocess.Popen([sys.executable, script, name])
        return True
    except OSError:
        return False


# ---- the window ---------------------------------------------------------------
def build(tk, ttk):
    class App(ttk.Frame):
        def __init__(self, root):
            super().__init__(root, padding=10)
            self.root = root
            self.pack(fill="both", expand=True)
            # tkinter garbage-collects Variables with no surviving reference, so
            # the checkbox variables are kept here for as long as the window lives.
            self.enable_vars = {}

            ttk.Label(self, text="momom2's mod manager", font=("", 13, "bold")).pack(
                anchor="w", pady=(0, 8)
            )

            self.body = ttk.Frame(self)
            self.body.pack(fill="both", expand=True)

            self.status = ttk.Label(self, text="", foreground="#777", wraplength=520)
            self.status.pack(anchor="w", pady=(8, 0))

            self.reload()

        def reload(self):
            for w in self.body.winfo_children():
                w.destroy()
            mods, note = load_catalog()
            if note:
                self.status.config(text=note)
            if not mods:
                ttk.Label(self.body, text="No mods to show.").pack(anchor="w")
                return
            for mod in mods:
                self.row(mod)

        def row(self, mod):
            name = mod["name"]
            frame = ttk.Frame(self.body, padding=(0, 6))
            frame.pack(fill="x")

            left = ttk.Frame(frame)
            left.pack(side="left", fill="x", expand=True)
            ttk.Label(left, text=mod.get("title", name), font=("", 10, "bold")).pack(anchor="w")
            if mod.get("blurb"):
                ttk.Label(left, text=mod["blurb"], foreground="#666", wraplength=420).pack(anchor="w")

            if is_installed(name):
                # Installed: a master on/off switch, a button to its config, and
                # the option to remove it. The switch and the config are the same
                # file -- the mod's own -- so they never disagree.
                self_cfg = mod.get("self_configuring", False)
                ttk.Button(
                    frame, text="Uninstall", width=10, command=lambda: self.remove(name)
                ).pack(side="right", padx=(4, 0))
                ttk.Button(
                    frame, text="Configure", width=11,
                    command=lambda n=name, sc=self_cfg: self.open_cfg(n, sc),
                ).pack(side="right", padx=(4, 0))

                enabled = read_enabled(name)
                if enabled is None:
                    ttk.Label(
                        frame, text="launch the game once", foreground="#999"
                    ).pack(side="right", padx=8)
                else:
                    var = tk.BooleanVar(value=enabled)
                    self.enable_vars[name] = var
                    ttk.Checkbutton(
                        frame, text="enabled", variable=var,
                        command=lambda n=name, v=var: self.toggle(n, v),
                    ).pack(side="right", padx=8)
            else:
                ttk.Label(frame, text="not installed", foreground="#999", width=12).pack(
                    side="right", padx=6
                )
                # Two ways in: install it dormant, or install it already switched
                # on with sensible defaults.
                ttk.Button(
                    frame, text="Install & enable", width=15,
                    command=lambda: self.add(name, enable=True),
                ).pack(side="right")
                ttk.Button(
                    frame, text="Install", width=10,
                    command=lambda: self.add(name, enable=False),
                ).pack(side="right", padx=(0, 4))

            ttk.Separator(self.body, orient="horizontal").pack(fill="x")

        def add(self, name, enable=False):
            try:
                source = install(name, enable=enable)
                extra = "" if source == "downloaded" else " (%s)" % source
                self.status.config(
                    text="installed %s%s — it loads next time the game starts" % (name, extra),
                    foreground="#2a7",
                )
            except (RuntimeError, OSError) as e:
                self.status.config(text=str(e), foreground="#c33")
            self.reload()

        def remove(self, name):
            try:
                uninstall(name)
                self.status.config(text="removed %s" % name, foreground="#777")
            except OSError as e:
                self.status.config(text="could not remove %s: %s" % (name, e), foreground="#c33")
            self.reload()

        def toggle(self, name, var):
            if set_enabled(name, var.get()):
                self.status.config(
                    text="%s %s — takes effect next launch" % (name, "enabled" if var.get() else "disabled"),
                    foreground="#2a7" if var.get() else "#777",
                )
            else:
                self.status.config(text="could not change %s's switch" % name, foreground="#c33")
                cur = read_enabled(name)
                var.set(bool(cur))

        def open_cfg(self, name, self_configuring):
            # A mod the kit can render opens in the settings window; the
            # self-configuring auto-picker has no form, so open its file instead.
            if not self_configuring and open_configurer(name):
                self.status.config(text="opened %s in the settings window" % name, foreground="#777")
                return
            if open_config(name):
                self.status.config(text="opened %s's config file" % name, foreground="#777")
            else:
                self.status.config(
                    text="no config for %s yet — launch the game once, then it appears" % name,
                    foreground="#c33",
                )

    return App


def main():
    try:
        import tkinter as tk
        from tkinter import ttk
    except ImportError:
        print("tkinter is not available in this Python.", file=sys.stderr)
        print("Install mods by hand: put each mod's <name>.dll into", file=sys.stderr)
        print("   ", MODS_DIR, file=sys.stderr)
        return 1

    root = tk.Tk()
    root.title("momomod — Mods")
    root.minsize(560, 320)
    build(tk, ttk)(root)
    root.mainloop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
