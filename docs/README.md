# Design notes

These are the project's working design records, kept as written. The project began as `civ.game`, a browser game
with a TypeScript/Svelte client, and several notes describe that client or cite product documents that are not
published here (PLATFORM.md, BRAND.md, EXPERIENCE.md, UI-REBUILD.md, DESIGN-SYSTEM.md, SOUND.md, PROCEDURAL.md,
REBUILD.md, ENGINE-BRIEF.md). The Rust crates in this repository are the current code; where a note disagrees with
the code, the code is right.

Engine and protocol:

- [ARCHITECTURE.md](ARCHITECTURE.md): crates, persistence and the session layer (its web-client sections describe
  the removed client).
- [CLIENT-PROTOCOL.md](CLIENT-PROTOCOL.md): the contract between a client and `civ-sim`.
- [AI.md](AI.md): how the AI is measured and improved.
- [PERFORMANCE.md](PERFORMANCE.md): performance work.
- [../crates/civ-core/README.md](../crates/civ-core/README.md): the engine's approach, the differential tests
  against Unciv and the determinism argument.

Content:

- [CONTENT.md](CONTENT.md), [ROSTER.md](ROSTER.md), [BLITZ.md](BLITZ.md): the `base` and `blitz` rulesets.
- [ORIGINALITY.md](ORIGINALITY.md): proposals for replacing content inherited from Civilization V.

Bevy client milestones:

- [CLIENT-BRIEF.md](CLIENT-BRIEF.md), [VISUAL-SPEC.md](VISUAL-SPEC.md), [PLAY-SPEC.md](PLAY-SPEC.md),
  [ROOMS-SPEC.md](ROOMS-SPEC.md), [ACTIONS-SPEC.md](ACTIONS-SPEC.md), [QUALITY-SPEC.md](QUALITY-SPEC.md).

Historical (web client):

- [PLAYABLE.md](PLAYABLE.md), [GAPS.md](GAPS.md).
