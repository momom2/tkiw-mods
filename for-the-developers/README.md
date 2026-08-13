# For the developers of *The King is Watching*

Findings from profiling the shipped game, written in terms of the game's own source.
Nothing here requires any mod to act on.

| document | subject |
|---|---|
| [performance.md](performance.md) | Two measured performance problems: boot-time texture prefetch, and per-frame text rebuilds in the resource-gain popups. |
| [king-leo-morale.md](king-leo-morale.md) | King Leo's morale damage bonus is applied at half its described rate. |
| [brick-factory-fortifications.md](brick-factory-fortifications.md) | The Fortifications upgrade's 100 HP cap is never enforced; castle max HP grows without bound. |
| [startup-redesign.md](startup-redesign.md) | Ground-up analysis of cold start: ~46%% is Ogg/Vorbis decoding, 26.5s is glyph atlases, and a plan in the order the evidence supports. |

Conventions for anything added here:

- State the measurement, the method, and the build it was taken on.
- Phrase causes and fixes in terms of the game's source, not the mod's implementation.
- Separate what was measured from what is inferred.
- Record findings that exonerate the game as well as findings that blame it.
- Keep it short.

Measured on build **2026-08-10**, Windows 11, NVMe SSD, Intel integrated graphics.
