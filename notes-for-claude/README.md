# Notes for Claude

You're reading this because you're picking up work on these mods. This folder is for
you, not for humans. Write here however you like — narrative, first person, long,
digressive, whatever actually helps. Nobody is going to be annoyed by verbosity here.
The one thing that matters is that a future instance of you can act on it.

Everything else in this repo has a human audience and a different style. Don't put war
stories in those. Specifically:

| folder | audience | style |
|---|---|---|
| `knowledge-base/` | modders (human) | **clinical.** Facts, tables, addresses, APIs. Minimal prose. If it reads like a story, it belongs here instead. |
| `for-the-developers/` | the game's devs | **clinical**, and phrased in terms of *their* source. Self-contained. |
| each mod's `README.md` | players | only what's needed to use it |
| **here** | you | anything that helps |

## What goes here

- **Lessons that cost something.** `pitfalls.md` is the main one. Every entry is a real
  mistake with the evidence that revealed it. When you burn an hour on something, write
  it down before you forget why it was confusing.
- **Session notes** (`sessions/`). What was attempted, what was decided and why,
  especially decisions *not* to do something. The reasoning is the valuable part; the
  outcome is usually recoverable from the code.
- **Wrong turns.** Conclusions that were later contradicted, kept alongside what
  disproved them. This repo has four good examples so far, and each one was believed
  confidently, with evidence, and was wrong:
  - "compiled GML never calls the builtins" — real measurement, wrong inference
  - "the timeline shows when `obj_init` starts" — it showed when the registry became
    readable, which is 22 seconds later
  - "quantise the alpha from the frame hook" — Step recomputes it before Draw, so the
    write does nothing
  - "startup is ogg/vorbis decoding" — the right CRC polynomial for the wrong codec; a
    day of planning was reasoned from a name nobody had checked

## Things worth knowing before you start

**Read `knowledge-base/orientation.md` first.** It's short and it rules out the
approaches that don't work here.

**The user is momom2.** They know this game deeply and they'll catch you when you're
wrong — several of the best turns in this project came from them pushing back on a plan.
When they ask "why can't we just X", take it seriously; twice now X was simply better
than what was being proposed.

**You cannot see the screen and playtesting costs them time.** `knowledge-base/tools/playtest.py`
launches the game unattended and watches a log, which covers boot and anything visible
in a log. It cannot get into a run — that needs a person, so batch up whatever needs a
live run and ask once. Driving the game with screen control was tried and it was
disruptive and slow; don't reach for it again without a specific reason.

**Measure before you optimise.** Both speedups here would have been guessed wrong. One
looked like disk I/O and was CPU-bound decoding; the other looked like unit AI and was
text rendering. And two of the loudest complaints turned out not to be the game's fault
at all — profile first, every time.

**Be careful what you patch and when.** The kit can write to the game's code now
(`tkiw_runtime::patch`, `codecave`). The rule that makes it safe is the *window*: either
before the game's entry point, or from the frame hook where you're on the game's own
single thread and it's provably inside `PeekMessageW`. Anything else and you're racing
the game.
