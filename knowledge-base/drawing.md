# Drawing on screen

A mod can draw over the game — shapes in GUI space, on top of the game's own
UI — through `tkiw_runtime::overlay`. It hides the two hard parts: getting onto
a Draw event (the frame hook runs after the frame is presented, so it cannot
draw), and calling the game's draw builtins with the ambient state preserved.

## The mechanism, in one paragraph

GameMaker draws from Draw events. `overlay` installs a
[trampoline](../tkiw-momomod-kit/src) — a register-preserving detour — on one
Draw event's prologue, and from inside it calls the game's own
`draw_rectangle`, `draw_line`, and so on. Because the call happens mid-frame on
the game's thread, what a mod draws is indistinguishable from what the game
draws. See `tkiw_runtime::trampoline` for the detour itself and
`tkiw_runtime::overlay` for the drawing layer.

## Using it

Two steps. **Once**, the host mod names the Draw event to draw into — a
build-specific address, found with the `draw_probe` diagnostic and passed with
the bytes currently there, so a game update disables drawing rather than
corrupting it:

```rust
// obj_display_manager's Draw GUI End: runs last in the GUI phase, so an overlay
// sits on top of the game's UI. The kit does this at startup.
overlay::set_host(rt.base + 0x1257860, &[0x48, 0x8b, 0xc4, 0x48, 0x89, 0x58, 0x10]);
```

Then **any feature** draws by registering a painter and holding the handle:

```rust
use tkiw_runtime::overlay::{self, Colour};

let handle = overlay::paint(rt, |c| {
    let (w, h) = c.gui_size();
    c.rectangle(w / 2.0 - 20.0, h / 2.0 - 20.0, w / 2.0 + 20.0, h / 2.0 + 20.0, Colour::BLACK);
    c.frame(10.0, 10.0, 100.0, 60.0, Colour::rgb(255, 200, 0));
})?;
```

The closure runs once per frame with a `Canvas`. Drawing stops the moment the
handle drops — so a feature stores it in `activate` and drops it in
`deactivate`, and the overlay draws exactly while the feature is on.

## Guarantees

- **Nothing is patched until something draws.** The detour installs on the
  first painter and reverts when the last handle drops. A kit with no overlay
  feature active changes nothing about the game.
- **One bad painter cannot take down the others, or the game.** Each runs inside
  a panic boundary; one that panics is dropped with a line in the log.
- **The game's own drawing is unaffected.** Colour and alpha are saved before the
  painters run and restored after.
- **Register/unregister on the game's thread.** `paint` installs the detour, so
  call it from `activate` (on the game thread) or before the game's entry point,
  never from another thread.

## The `Canvas`

Coordinates are GUI pixels, origin top-left. Colours are `Colour` — a BGR
integer, but use `Colour::rgb(r, g, b)` and the constants (`BLACK`, `WHITE`,
`RED`, `GREEN`, `BLUE`) rather than packing bytes.

| method | draws |
|---|---|
| `gui_size() -> (f64, f64)` | the GUI surface size, for laying out relative to the screen |
| `rectangle(x1, y1, x2, y2, colour)` | a filled rectangle |
| `frame(x1, y1, x2, y2, colour)` | a rectangle outline |
| `line(x1, y1, x2, y2, colour)` | a one-pixel line |
| `line_width(x1, y1, x2, y2, w, colour)` | a line of a given width |
| `circle(x, y, r, colour, filled)` | a circle |

## What is not here yet

**Text.** `draw_text` takes a GML string, and the builtin bridge only passes
numbers — handing text to the game means constructing a string RValue it will
accept, which is a separate, riskier piece deferred until it can be proved on
screen. The `Canvas` API is additive, so text lands as a new method without
disturbing shape-drawing callers.

## Finding a host on a new build

`set_host` needs the address of a Draw event and the exact bytes of its
prologue. Run the `draw_probe` diagnostic (in the kit); its report lists every
object with a GUI-layer or begin/end Draw event, whether it is alive in each
phase, and its prologue bytes. A good host is alive in every phase you want to
draw in and opens with position-independent instructions (a `mov rax, rsp` and
register stores, not a rip-relative load or a relative branch). `obj_cursor`'s
Draw GUI and `obj_display_manager`'s Draw GUI End are both good on the current
build.
