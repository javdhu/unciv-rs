# Rooms spec (M5)

Status: R1-R7 decided by Javed on 2026-10-13; built the same day (How M5 is built, at the end).

| | Choice | Pick |
|---|---|---|
| R1 | App flow | In-app states: Home, Rooms, Lobby, Match; a match tears down cleanly on leaving |
| R2 | Serving the browser client | civ-server serves a static directory (`CIV_STATIC_DIR`) with COOP/COEP |
| R3 | Native identity (D4) | Imitate a browser: guest token from `Set-Cookie`, stored per server (0600), sent as `Cookie` with the server's own `Origin` |
| R4 | Outbox across a restart | Memory only; reconnects resend |
| R5 | Rooms HTTP types | Moved to civ-sim, used by server and client |
| R6 | Network crates | Native `tungstenite` and `ureq` on the Link thread; browser `web-sys` WebSocket and fetch |
| R7 | Trade view | As Unciv (gold, gold per turn, resource counts, cities), only for civs where you hold an embassy |

The options not picked stay below for the record.

## Problem

Play a match with other people through civ-server, from the native client and from the browser, with everything
above Link (ViewState, Outbox, play, screens, presentation) unchanged. M5 also carries the M4 follow-up Javed
decided: civ-sim shows what each met civ can trade.

## What the server already does

Read from the code on 2026-10-13 (`crates/civ-server/src/http.rs`, `host.rs`, `store.rs`).

- Identity is a guest cookie, `civ_guest`, a 64-hex token set by `POST /api/guest`: `HttpOnly; SameSite=Lax;
  Path=/`, 30 days. The server keeps only its SHA-256. There are no accounts or passwords.
- Every POST and the WebSocket upgrade must carry an `Origin` from `CIV_ALLOWED_ORIGINS` (default: localhost and
  127.0.0.1 on ports 3001, 5199 and 5304); a missing or other Origin gets 403. There is no CORS layer and no
  static file serving: in production civ.game served the old Svelte client and a Cloudflare Worker there
  proxied `/api` to Fly (`fly.toml`), so page and API shared one origin.
- Rooms: `POST /api/rooms` (settings, `turnSeconds` 5-3600, default 120), `GET /api/rooms` (the caller's rooms),
  `POST /api/rooms/join {invite}`, `GET /api/rooms/{id}` (members only; polling it every 2 s is also how
  presence is kept, 6 s), `POST /api/rooms/{id}/seat {nation, leader}`, `/start` (owner), `/leave`. Seat number
  is civ id; majors nobody holds are bots. No push channel for lobbies, no public room list.
- Match socket `GET /api/matches/{matchId}/ws`: the client sends `hello` (protocol 9) within 5 s and gets a full
  `Welcome`. Every connect starts from a full Welcome; there is no resume from a cursor. Then Command/Receipt,
  Query/QueryReply, Resync/Update and Deltas as in-process. The server pings every 10 s and drops a peer silent
  for 30 s. 128 frames/s, 64 KiB per message.
- Turns are simultaneous. The round closes when every human seat has finished, or at the deadline
  (`deadlineMs`, epoch milliseconds as a string, server clock). Once a seat has finished, every command from it,
  Undo included, is refused with `turnFinished` until the next turn.
- Disconnects: after 15 s a bot is scheduled for the seat and takes it at the next round boundary; reconnecting
  schedules the person back. `GameUpdate.controlled`, `pendingControl` and `seats` (ready, controlled,
  connected, name, left) say so. Leaving a started match is permanent.
- Retried commands with the same `commandId` and payload get the stored receipt, across reconnects and server
  restarts (SQLite). Matches, seats and deadlines survive a restart; sockets and lobby presence do not.
- Close reasons arrive as `Error{code}` before the close: `connectionReplaced` (a newer connection took the
  seat; terminal), `rateLimited` and `unavailable` (retry), `protocolVersion`, `invalidRequest`.

## Non-goals

Deploying anything or touching civ.game; accounts, passwords and matchmaking; spectating other people's rooms;
chat; lobby push; server changes beyond the ones R2, R3 and R5 pick; solo saves (M6).

## Architecture

```
 civ-client
 ┌ Rooms ─────────────────────────────────────────────┐
 │ Home: solo · watch · rooms                         │
 │ Rooms: yours, join by invite, create (settings)    │      HTTP (cookie or R3)
 │ Lobby: seats, nation and leader, who is here,      │ ───── /api/guest, /api/rooms…  ─────┐
 │        invite to copy, start (owner), leave        │                                     │
 └───────────────┬────────────────────────────────────┘                                     ▼
                 │ matchId                                                            ┌───────────┐
 ┌ Match ────────▼─────────────────────────────────────────────────────┐            │ civ-server│
 │ Link::remote(server, matchId)  ── WebSocket /api/matches/{id}/ws ──────────────▶ │  Host     │
 │ Link::play / Link::watch       ── in-process thread (as now)        │            └───────────┘
 │   Down::Message(ServerMessage) | Down::Status(Live | Retrying(n) | Stopped(reason))           │
 │ ViewState · Outbox (resends unresolved ids after a Welcome) · play · screens · presentation    │
 │ + seats strip, deadline, "waiting for N", connection line, bot-control notice                  │
 └───────────────────────────────────────────────────────────────────────────────────────────────┘
```

## Components and contracts

1. Api. `guest()`, `rename(name)`, `rooms()`, `create(settings)`, `join(invite)`, `room(id)`, `seat(id, nation,
   leader)`, `start(id)`, `leave(id)`, `rules()`. Every call returns at once with a request id; replies arrive
   as messages, so the browser's main thread never waits. Errors are the server's codes (401, 403, 404, 409,
   429, 503) mapped to one enum. The request and response types: R5.
2. Link::remote. The same `Up`/`Down` as in-process plus `Down::Status`. Connect, send `hello`, expect
   `Welcome`. On a dropped socket or a retryable code: retry at 250 ms doubling to 10 s with jitter, up to 8
   tries, the count reset after 30 s connected. Terminal: `connectionReplaced`, 403 on the upgrade (left or not
   a member), `protocolVersion`. `Up::Advance` is ignored remotely; the server closes rounds.
3. ViewState and presentation on reconnect. A Welcome replaces the view (it already does); presentation drops
   its queue, since replay batches from the missed rounds are gone. The client never applies a delta whose base
   is not its cursor (unchanged).
4. Outbox. After every Welcome it resends unresolved commands in order with their ids and payloads; a command
   the server already applied returns its stored receipt. Whether the Outbox also survives a client restart:
   R4.
5. Rules. If the Welcome's `contentHash` differs from the client's compiled tables, fetch `/api/rules` and use
   those tables (the brief's rules cache, component 8). The in-process Session keeps the compiled content.
6. Match UI additions.
   - Top bar: the deadline as a countdown (server clock, offset from the HTTP `Date` header to within a second),
     red under 15 s.
   - Seats strip under the top bar: each human seat's name and colour with finished, connected, bot-controlled
     or left.
   - After End turn: "Waiting for 2 players", orders disabled (the server refuses them). End turn asks for a
     second click in rooms, since it cannot be undone there.
   - Connection line: "Reconnecting (3)" while retrying; when stopped, the reason and Back to rooms.
   - `pendingControl`: "A bot plays your seat from next turn" or "You take your seat back next turn".
7. Rooms screens: in Bevy UI with the M3/M4 panels' look, the same on native and wasm. Lobby state is polled
   every 2 s while the lobby is open (the server's presence rule).
8. Trade view (M4 follow-up). `RelationView` gains `tradable` for met majors not at war, computed with the
   engine's own rules (`trade::can_give`, `resources::available_stock`, `available_gold_per_turn`): gold, gold
   per turn, luxuries, strategic resources, cities, open borders, embassy. How much it reveals: R7. The trade
   window's "they give" lists from it.

## Data flow and state ownership

The server owns rooms, seats, matches, deadlines and receipts. The client owns the guest token (native, R3),
the screen it is on, the Outbox, and nothing else that outlives a connection. A reconnect is: socket drops,
status Retrying, socket opens, `hello`, `Welcome` replaces the view, presentation clears, the Outbox resends,
status Live.

## Choices (decided; see the table at the top)

R1. App flow. (a) In-app states: Home, Rooms, Lobby and Match as Bevy states; leaving a match tears down its
entities and resets ViewState, Outbox, Selection and Screens, and the next match starts clean in the same
process. (b) One match per run: the rooms screens hand a match id to a fresh start of the client (native
relaunches itself with the id; the browser reloads with `?match=`). Teardown code is not needed, at the cost of a
restart between matches and a launcher step on native.

R2. Serving the browser client. The page must share an origin with `/api` for the cookie, and needs COOP/COEP
for wasm threads. (a) civ-server serves a static directory (`CIV_STATIC_DIR`) with COOP/COEP headers: one
process, one origin, and production could later drop the Worker proxy. (b) A dev proxy in front of both
(`serve.py` grows WebSocket proxying, or Caddy as a new tool); the server stays as it is, and production would
need the same proxy rules as civ.game's Worker.

R3. Native identity (D4). (a) Native imitates a browser: `POST /api/guest`, keep the token from `Set-Cookie`,
store it per server in the user's config directory (mode 0600), send it as `Cookie`, and send an `Origin` the
server allows (the server's own URL). No server change; native sends a browser header by convention. (b) Bearer
token: the server also accepts `Authorization: Bearer <token>` for the same guest, and requests carrying it skip
the Origin check, since no ambient credential is involved. A small server change with tests; stored as in (a).
(c) Rooms in the browser only for M5; native stays solo and watch, D4 stays deferred.

R4. Outbox across a client restart. (a) Written before sending (native: a file per match; browser:
localStorage per match), resent on start, so an order the player gave is either applied once or still pending.
(b) In memory only: reconnects resend, a client crash forgets orders not yet acknowledged; those that reached
the server are applied once and show in the restarted view, the rest the player gives again.

R5. Rooms HTTP types (now private in civ-server). (a) Move them to civ-sim (a `rooms` module of serde types),
used by server and client: one definition. (b) The client keeps its own copies, checked by a test against a
real civ-server in the client's test suite. (c) civ-server exports them behind a feature that leaves out axum,
tokio and SQLite, and the client depends on it.

R6. Network crates. (a) Native: `tungstenite` (already in Cargo.lock, 0.29) and `ureq` on the Link thread,
synchronous, no async runtime; browser: `web-sys` WebSocket and fetch with callbacks into the lock-free queues.
Two small transports. (b) `ewebsock` and `ehttp`: one API for both targets (native uses tungstenite and ureq
underneath); fewer cfg branches, two more third-party crates that follow egui's release cadence. (c) Native on
tokio with `tokio-tungstenite` and `reqwest` (what the server's tests use): an async runtime in the client and
longer builds; the browser side is (a)'s anyway.

R7. What the trade view reveals. Unciv's trade screen shows the other civ's gold, gold per turn, resources with
counts and cities, with no embassy needed (`TradeLogic.getAvailableOffers`, both sides). The port chose
stricter: `agreements.rs:21` says a proposal must not reveal the recipient's treasury, resource balance or
income, and `ai_offers.rs` keeps the other side's holdings inside the engine. (a) As Unciv: exact amounts for
every met major at peace; the port's rule is relaxed for the view. (b) As Unciv, but only with an embassy in
their capital. (c) Kinds without amounts: which luxuries, strategic resources and cities they could give, not
how many or how much gold; asking for more than they have is refused as today.

## Risks and unknowns

- Browser cookie rules: `SameSite=Lax` cookies ride same-origin requests only, so any split of page and API
  across origins breaks identity (R2 exists for this).
- Every reconnect costs a full Welcome, the whole view; its size on a 64x40 map over a poor link is unmeasured. A cursor-based resume would be a protocol change, out of M5.
- Deadline display depends on clock offset; the server is authoritative, so a wrong countdown misleads but
  never decides.
- A slow client with the 16-update queue full gets a forced full Update; ViewState handles it like a Welcome.
- Two humans on one machine need two guests: two browser profiles, or two native config directories.

## How it will be verified

- Client tests against a real civ-server started in-process on a free port (as civ-server's own network tests
  do): create a room, join from a second guest, start, play a round with both seats, drop one socket and see
  Retrying then Live with the Outbox resent, a stale command refused with `staleTurn` after the deadline,
  `connectionReplaced` stopping the older client.
- A trade view test in civ-sim: what `tradable` shows equals what `can_give` accepts, for each R7 rule.
- A scripted two-client recording: native and browser in one room, both finishing turns, one reconnecting.
- Chrome by hand for the lobby and the match, both flavours.

## Rollback

Link::remote and the rooms screens are additive; solo and watch keep their paths. Server changes from R2 or R3
are additive routes or headers behind configuration. The trade view field is optional in the protocol, so an
older client ignores it.

## How M5 is built

- civ-sim: `rooms` holds every `/api` request and reply type, moved from civ-server with the JSON unchanged
  (civ-server's own tests pass unmodified). `RelationView.tradable` (R7) is present for a met, living major at
  peace in whose capital you hold an embassy (`our_embassy`): their gold, gold a turn up to the engine's cap
  (`available_gold_per_turn`), luxuries and strategic resources from `available_stock`, cities `can_give` accepts
  (not the capital, only cities you know), open borders and embassy. `embassy` is always false there, since the
  gate already means you have one. Two engine paths disagree at the edges: `can_give` reads resource stock
  cached at turn start and refuses any gold a turn while their treasury is negative, while accepting a trade
  reads fresh stock and does not check gold a turn that way; the view follows the acceptance path.
- civ-server: `site.rs` serves `CIV_STATIC_DIR` at every GET and HEAD outside `/api` and `/health`, with
  COOP/COEP/CORP, `nosniff` and `no-cache`, and a `.br` twin when the request accepts it. Each path segment is
  decoded and must be a plain name (no dot prefix, `..`, slashes, backslashes, colons or NUL); the file must
  stay inside the canonical root and be a regular file; anything else is 404. Files are read whole per request.
- Client, states (R1, `app.rs`): `Boot`, `Home`, `Rooms`, `Lobby`, `Match`. A match's systems run in the
  `InMatch` set, its setup on `OnEnter(Match)`; leaving sweeps every top-level entity with a `Transform` or
  `Node` not marked `Persist`, and each plugin resets its own resources on `OnExit(Match)`. Dropping the `Link`
  stops its host thread. Solo and watched matches have a Home button (solo asks twice: the match is lost).
- Client, network (R3, R6, `net.rs`, `link.rs`): natively, HTTP runs on a ureq thread and the match socket on a
  tungstenite thread with a 20 ms read timeout so it can send between reads; the guest token comes from
  `Set-Cookie` and lives in `<config>/guests.json` (0600) keyed by server origin and `CIV_PROFILE`, sent as
  `Cookie` with the server's own `Origin`. In the browser, fetch and WebSocket run on the page's thread, the
  socket driven once a frame. Lost sockets retry at 250 ms doubling to 10 s with jitter, 8 tries, the count
  reset after 30 s up; `connectionReplaced` and `protocolVersion` stop for good. What the client sends while
  the socket is down is dropped: after the welcome the Outbox resends its in-flight command with the same id
  and the screens ask again. Rules come over the socket (`Query::Rules`), asked again after a welcome whose
  content hash differs from the rules held; `/api/rules` only names the lobby's nations.
- Client, screens (`rooms.rs`, `roomui.rs`, `api.rs`): home; rooms (your name, your rooms with Play, Open or
  Leave, join by invite, create with map size, civilizations, city-states and turn time); lobby (settings, the
  invite as read-only text to copy, members with nation and presence, your nation, Start for the owner, Leave).
  Text typed in a field survives the screen's redraws. `?invite=` / `CIV_INVITE` opens on the rooms and joins;
  the browser keeps `?match=` in its address while a match is open. In a match: a panel under the top bar with
  the countdown (red under 15 s, server clock from the HTTP `Date`), the connection, the bot-control notice,
  each human seat (colour, name, playing, finished, away, bot or left) and Back to rooms; End turn takes a
  second click and then reads "Waiting for N players". The trade window lists what they can give from
  `tradable`, says when an embassy is needed, and offers cities on both sides.
- Verified: 1150 engine and server tests; 20 client tests, among them `tests/rooms.rs`, which runs civ-server
  in-process behind a relay it can cut (two guests create, join and start a room and finish a round; an order
  given while one connection is down is applied once after it returns; after the deadline an old turn token is
  refused `staleTurn`; a second connection stops the first with `connectionReplaced`), passing five runs in a
  row. `CIV_REJOIN=3`: the entity count at home is 465 before any match, 471 after the first and steady after
  the next two. End to end on this Mac: civ-server serving the web build, the native client hosting through
  `CIV_SCENE=room`, headless Chrome (WebGPU) joining from the invite link; the lobby showed the browser player
  as here, and in the match its seat stayed connected while deadlines closed the rounds. Both wasm builds check.
- Not verified: the browser's match as drawn (headless, not captured), its reconnects, and a refused browser
  socket (browsers hide the upgrade's status, so it retries eight times and stops with "could not be
  reached"); typing and clicking the rooms screens by hand; Safari; TLS (`https`, `wss`) natively; leaving a
  started match from the client (the server's tests cover it).
- Not built: the lobby's nation choice is one button per nation (about sixty); `build.sh` writes no `.br`
  twins, so the page loads 74 MB of wasm uncompressed from civ-server; no chat or public room list (non-goals).

