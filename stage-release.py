#!/usr/bin/env python3
"""
Stage everything a momomod release ships, and optionally serve it for an
end-to-end test of the mod manager.

    python stage-release.py            build and stage into dist/release/
    python stage-release.py --serve    also serve it over HTTP, so you can test
                                       the whole download flow before publishing

A release's assets, all uploaded to the **same** GitHub release:

    momomod-<version>.zip         the manager, for players
    momomod-<version>-modder.zip  the same, with the diagnostics mod included, for
                                  people making mods (it is not in the catalogue)
    catalog.json                  the list of mods the manager offers
    reward-picker.dll             \  the mods themselves, named <mod>.dll, which
    bugfixes.dll                  /  manage-mods.py downloads into the player's mods/
    bugfixes.default.ini          each mod's own default config, so "install and
                                  enable" needs no hand-written copy

**One GitHub release, not two.** The manager fetches the catalogue and every mod
from `releases/latest/download/`, and only one release can be "latest": splitting
the two audiences into two releases would leave whichever is older unable to
download any mod at all. Two zips on one release keeps both working.

The manager fetches catalog.json and each <mod>.dll from the release's
`latest/download/` URL, so all four live on the one release.

## Testing before you publish

`--serve` stages the assets and serves them at http://localhost:<port>. Point
the mod manager at that instead of GitHub and the whole flow runs locally:

    # terminal 1
    python stage-release.py --serve

    # terminal 2 (Windows: set the variable first, then run)
    MOMOMOD_MODS_BASE=http://localhost:<port> python momomod-manager/manage-mods.py

Install a mod in the window, launch the game, and confirm it loads -- exactly
what a player will do, but from your own machine.
"""
import http.server
import os
import shutil
import socketserver
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.abspath(__file__))
MANAGER = os.path.join(ROOT, "momomod-manager")
BUILT = os.path.join(ROOT, "target", "release")
RELEASE = os.path.join(ROOT, "dist", "release")

# Mod name (as the player installs it) -> the crate's built DLL. A release
# renames each to <mod>.dll; this map is the one place that correspondence lives.
MOD_DLL = {
    "reward-picker": "tkiw_reward_picker_plugin.dll",
    "bugfixes": "tkiw_bugfixes_plugin.dll",
}

# Mods whose default config the release also ships, as <mod>.default.ini.
#
# "Install and enable" has to write a config before the mod has ever run (a mod
# writes its own on first launch, which is too late). The manager copies this
# rendering rather than keeping a hand-written copy, so the descriptions live in
# exactly one place -- the Rust that renders them. Value is the crate providing
# the `dump-default-config` bin.
#
# A self-configuring mod (the auto-picker, which builds its file from the live
# game) is deliberately absent.
#
# `(crate, bin)`. The bin is named after the mod rather than a shared
# `dump-default-config`: two crates with a bin of the same name compile to the
# same `target/release/dump-default-config.exe` and clobber each other, which
# silently staged the diagnostics' config as the bugfixes one.
MOD_DEFAULT_CONFIG = {
    "bugfixes": ("tkiw_bugfixes_plugin", "dump-bugfixes-config"),
}


def run(cmd, **kw):
    print("+", " ".join(cmd))
    subprocess.run(cmd, check=True, **kw)


def stage():
    run(["cargo", "build", "--release"], cwd=ROOT)

    # The manager zip and a fresh catalogue, both from the manager's own scripts.
    run([sys.executable, "package.py"], cwd=MANAGER)
    run([sys.executable, "-c",
         "import subprocess,sys;"
         "subprocess.run(['cargo','run','-q','--bin','dump-catalog','--','--into','.'],check=True)"],
        cwd=MANAGER)

    os.makedirs(RELEASE, exist_ok=True)
    # start clean so a removed mod does not linger in the staged release
    for f in os.listdir(RELEASE):
        os.remove(os.path.join(RELEASE, f))

    staged = []

    # the manager zip
    dist = os.path.join(MANAGER, "dist")
    zips = [f for f in os.listdir(dist) if f.startswith("momomod") and f.endswith(".zip")]
    if not zips:
        sys.exit("error: package.py did not produce a manager zip in %s" % dist)
    newest = max(zips, key=lambda f: os.path.getmtime(os.path.join(dist, f)))
    shutil.copy(os.path.join(dist, newest), os.path.join(RELEASE, newest))
    staged.append(newest)

    # The modder variant: the same manager, with the diagnostics mod already in
    # `mods/`. Those tools are deliberately absent from the published catalogue --
    # a player has no use for a profiler that stops the game's thread a thousand
    # times a second -- so this zip is how someone working on a mod gets them.
    #
    # It arrives switched off, like any mod: the plugin writes its own config on
    # first launch with everything off, and the settings window shows it from then
    # on. (It used to be a config line flipping `hidden` to `false`, back when the
    # diagnostics were compiled into the manager and shipped to everyone. Now they
    # are simply not in a player's install, which is why `hidden` could go.)
    modder = newest[: -len(".zip")] + "-modder.zip"
    shutil.copy(os.path.join(RELEASE, newest), os.path.join(RELEASE, modder))
    diagnostics = os.path.join(BUILT, "tkiw_diagnostics_plugin.dll")
    if not os.path.isfile(diagnostics):
        sys.exit("error: the diagnostics plugin is not built (looked for %s)" % diagnostics)
    with zipfile.ZipFile(os.path.join(RELEASE, modder), "a", zipfile.ZIP_DEFLATED) as z:
        z.write(diagnostics, "mods/diagnostics.dll")
    staged.append(modder)

    # the catalogue
    shutil.copy(os.path.join(MANAGER, "catalog.json"), os.path.join(RELEASE, "catalog.json"))
    staged.append("catalog.json")

    # the mods, renamed to <mod>.dll
    for mod, built in MOD_DLL.items():
        src = os.path.join(BUILT, built)
        if not os.path.isfile(src):
            sys.exit("error: %s not built (looked for %s)" % (mod, src))
        shutil.copy(src, os.path.join(RELEASE, mod + ".dll"))
        staged.append(mod + ".dll")

    # each mod's default config, rendered by the mod itself
    for mod, (crate, binary) in MOD_DEFAULT_CONFIG.items():
        out = os.path.join(RELEASE, mod + ".default.ini")
        run(["cargo", "run", "-q", "--release", "-p", crate,
             "--bin", binary, "--", out], cwd=ROOT)
        if not os.path.isfile(out):
            sys.exit("error: %s did not render a default config" % mod)
        # It is this mod's document, not another's: a bin-name collision once
        # staged the wrong one, and the file looks perfectly valid either way.
        head = open(out, encoding="utf-8").readline().strip()
        if head != "# " + mod:
            sys.exit("error: %s.default.ini begins %r, not '# %s' -- the wrong mod's\n"
                     "       config was rendered." % (mod, head, mod))
        staged.append(mod + ".default.ini")

    print("\nstaged into %s:" % RELEASE)
    for name in staged:
        size = os.path.getsize(os.path.join(RELEASE, name))
        print("  %-28s %8.1f KB" % (name, size / 1024))
    print("\nupload all of these to one GitHub release on momom2/tkiw-mods.")
    return staged


def serve():
    handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=RELEASE, **k)
    srv = socketserver.TCPServer(("127.0.0.1", 0), handler)
    port = srv.server_address[1]
    print("\nserving the staged release at http://localhost:%d" % port)
    print("test the manager against it, in another terminal:")
    print("    set MOMOMOD_MODS_BASE=http://localhost:%d   (Windows)" % port)
    print("    python momomod-manager/manage-mods.py")
    print("\nCtrl+C to stop.")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print("\nstopped.")


def main():
    stage()
    if "--serve" in sys.argv:
        serve()
    return 0


if __name__ == "__main__":
    sys.exit(main())
