#!/usr/bin/env python3
"""
Launch the game, watch a mod's log, and shut it down. Unattended.

Written because debugging was bottlenecked on a human being available to click
Play. Most questions about a mod -- does the game still start, how long does boot
take, which phase does it hang in -- are answered by launching, reading the log and
killing the process, none of which needs a person.

    python playtest.py --log ../../tkiw-momomod-kit/momomod.log \\
                       --until "+ obj_main_menu" --timeout 180

    python playtest.py --log ... --until "+ obj_gameplay_controller" --hold 60

Exits 0 if every `--until` pattern appeared, 1 on timeout, 2 if the game died on its
own, 3 if it never started.

## Launching through Steam, and why

Running the executable directly does not work: `steam_api64` sees no app context,
calls `SteamAPI_RestartAppIfNecessary`, and **exits with code 0** while Steam starts
a fresh process. A launcher that watches the process it spawned therefore sees a
clean exit after nine seconds and misses the game entirely.

So this launches `steam://rungameid/<appid>` and then finds the game **by process
name**, which is correct whichever way Steam decides to start it.

## What it does about a hang

A hang is the interesting case, so it is handled rather than merely tolerated. On
timeout the game is killed and the log tail printed, along with **whether the window
was still responding** -- "not responding" is Windows saying the main window has not
serviced its message queue, which is exactly the difference between a game that is
busy and a game that is wedged.

## Cautions

* **Never run this while someone is playing.** It refuses if the game is running.
* It force-kills. Fine for boot tests, which never reach a run. Every launch
  snapshots saves anyway.
* A kill before the mod's crash-loop guard stands down (60s) leaves the guard's
  breadcrumb behind, which makes the *next* launch passive -- and with Steam
  relaunching, that next launch can be seconds later and part of the same test. The
  breadcrumb is therefore cleared both before launching and again once the game
  process is up, because an automated kill is not the crash the guard exists for.
"""
import argparse
import csv
import os
import subprocess
import sys
import time

EXE_NAME = "The King is Watching.exe"
GAME_DIR_DEFAULT = r"C:\Program Files (x86)\Steam\steamapps\common\The King is Watching"
APPID = "2753900"
BREADCRUMB = "probe.incomplete"


def game_pids():
    """PIDs of every running game process, by image name."""
    out = subprocess.run(
        ["tasklist", "/FI", f"IMAGENAME eq {EXE_NAME}", "/FO", "CSV", "/NH"],
        capture_output=True, text=True).stdout
    pids = []
    for row in csv.reader(out.splitlines()):
        if len(row) >= 2 and row[0].lower() == EXE_NAME.lower():
            try:
                pids.append(int(row[1]))
            except ValueError:
                pass
    return pids


def responding(pid):
    out = subprocess.run(
        ["tasklist", "/FI", f"PID eq {pid}", "/FI", "STATUS eq NOT RESPONDING",
         "/FO", "CSV", "/NH"],
        capture_output=True, text=True).stdout
    return str(pid) not in out


def kill(pid):
    subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)


def read_log(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return ""


def clear_breadcrumb(mod_dir, why):
    crumb = os.path.join(mod_dir, BREADCRUMB)
    if os.path.isfile(crumb):
        try:
            os.remove(crumb)
            print(f"note: removed {BREADCRUMB} ({why})")
        except OSError:
            pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", required=True, help="the mod log to watch")
    ap.add_argument("--until", action="append", default=[],
                    help="substring to wait for; repeatable, all must appear")
    ap.add_argument("--timeout", type=float, default=180.0)
    ap.add_argument("--hold", type=float, default=0.0,
                    help="seconds to keep running after the last pattern appears")
    ap.add_argument("--start-timeout", type=float, default=60.0,
                    help="how long to wait for Steam to get the game running")
    ap.add_argument("--appid", default=APPID)
    ap.add_argument("--tail", type=int, default=30)
    ap.add_argument("--keep-log", dest="fresh", action="store_false", default=True)
    args = ap.parse_args()

    log = os.path.abspath(args.log)
    mod_dir = os.path.dirname(log)

    if game_pids():
        sys.exit(f"error: {EXE_NAME} is already running. Refusing to interfere.")

    clear_breadcrumb(mod_dir, "left by a previous run")
    if args.fresh and os.path.isfile(log):
        aside = log + ".prev-session"
        os.replace(log, aside)
        print(f"note: previous log moved to {os.path.basename(aside)}")

    url = f"steam://rungameid/{args.appid}"
    print(f"launching {url}")
    t0 = time.time()
    os.startfile(url)  # noqa: S606 -- a steam: URL, not a shell command

    # Steam may start the game directly, or the game may restart itself through
    # Steam. Either way it is found by name, not by the pid of what we spawned.
    pid = None
    while time.time() - t0 < args.start_timeout:
        pids = game_pids()
        if pids:
            pid = pids[-1]
            break
        time.sleep(0.5)
    if pid is None:
        print(f"the game did not start within {args.start_timeout:.0f}s")
        return 3
    print(f"  [{time.time() - t0:6.1f}s] running as pid {pid}")
    # A relaunch through Steam means the process that wrote the breadcrumb is not the
    # one now running, and the new one would go passive on finding it.
    clear_breadcrumb(mod_dir, "so the running session is not held back")

    wanted, status = list(args.until), "timeout"
    while True:
        elapsed = time.time() - t0
        live = game_pids()
        if pid not in live:
            # A restart-through-Steam handover looks like this; follow it.
            if live:
                pid = live[-1]
                print(f"  [{elapsed:6.1f}s] followed a handover to pid {pid}")
            else:
                status = "exited"
                print(f"the game exited by itself after {elapsed:.1f}s")
                break
        text = read_log(log)
        for pat in list(wanted):
            if pat in text:
                wanted.remove(pat)
                print(f"  [{elapsed:6.1f}s] saw {pat!r}")
        if not wanted:
            status = "ok"
            if args.hold:
                print(f"  holding {args.hold:.0f}s")
                time.sleep(args.hold)
            break
        if elapsed > args.timeout:
            alive = responding(pid)
            print(f"TIMED OUT after {elapsed:.1f}s waiting for {wanted!r}")
            print("  the window was "
                  + ("responding (busy or idle, but pumping messages)" if alive
                     else "NOT RESPONDING -- wedged, not pumping messages"))
            break
        time.sleep(0.5)

    for p in game_pids():
        print(f"killing pid {p}")
        kill(p)

    # Our kill is not a crash, but the mod cannot tell the difference: it left a
    # breadcrumb before probing and never got to clear it. Left behind, that makes the
    # *next* launch passive -- and if the next launch is a human sitting down to play,
    # they get an unmodded game with no indication why. That happened: a boot test
    # silently disabled the kit for a real session, and the measurement it was meant to
    # produce was lost.
    #
    # We know the kill was deliberate, so we clear it. The guard is still doing its job
    # for launches this script did not end.
    clear_breadcrumb(mod_dir, "the kill above was ours, not a crash")

    print(f"\n---- last {args.tail} lines of {os.path.basename(log)} ----")
    for line in read_log(log).splitlines()[-args.tail:]:
        print("  " + line)

    if status == "ok":
        print("\nresult: reached every marker")
        return 0
    return 1 if status == "timeout" else 2


if __name__ == "__main__":
    sys.exit(main())
