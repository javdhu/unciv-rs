# Local private-room host

From the repository root, run `cargo run -p civ-server`. The server listens on
`127.0.0.1:3001` and stores rooms, guest identities, seats, matches, receipts and
clock/controller state in `civ-server.sqlite3`. Only one host may open a database
at a time; its canonical `.host-lock` file remains on disk after shutdown.

Ctrl-C (SIGINT) and SIGTERM both stop the host gracefully: it stops accepting
requests, drains open HTTP requests for at most five seconds (match sockets are
not waited on), stops the match clock, checkpoints the SQLite WAL into the
database file, and exits 0. Every commit is already durable before that; the
checkpoint only leaves one self-contained file behind.

The existing Vite development server proxies `/api` and its WebSocket upgrade.
The exact loopback origins on ports 3001, 5199 (the dev server) and 5304 (the
dev server the web client's smoke run starts) are allowed by default.

Environment options:

| Name | Default | Purpose |
| --- | --- | --- |
| `CIV_BIND` | `127.0.0.1:3001` | Listening address |
| `CIV_DATABASE` | `civ-server.sqlite3` | Durable SQLite file |
| `CIV_ALLOWED_ORIGINS` | Loopback origins above | Comma-separated exact browser origins |
| `CIV_SECURE_COOKIE` | unset | Set `1` when serving through TLS |

A different bind address requires explicit allowed origins. This executable does
not terminate TLS. Mutating HTTP requests and WebSocket upgrades must include an
allowed `Origin`, including when tested outside a browser. Guest cookies are
host-only, HttpOnly and SameSite=Lax; only a SHA-256 hash of their random token is
stored in SQLite. Losing the cookie loses access to that guest's seats.

## HTTP and socket flow

1. `POST /api/guest` creates a guest and sets its cookie, or reuses the authenticated
   guest. Its JSON response contains `guestId`, and `name` once the guest chose one.
   `POST /api/guest/name` with `{ "name": "..." }` sets the display name other members
   see (1 to 32 characters once trimmed, no control characters; 400 otherwise) and
   returns `{ guestId, name }`. A rename reaches running matches with the next update.
2. `POST /api/rooms` accepts optional `width`, `height`, `majors`, `cityStates`, `speed`, `maxTurns`,
   `nation`, and `turnSeconds`. The map seed is generated and retained by the host.
   The owner receives `roomId`, its `invite`, and its assigned `ownSeat`.
3. Another guest posts `{ "invite": "..." }` to `/api/rooms/join`. The host assigns
   a distinct seat. The invite does not authenticate a game connection.
4. Members can `GET /api/rooms/{roomId}`. Only the owner can
   `POST /api/rooms/{roomId}/start`; its response includes `matchId`.
   Starting again returns the existing match. New guests cannot join after start.
5. Connect `/api/matches/{matchId}/ws` using the guest cookie and allowed origin,
   then send `{ "type": "hello", "version": 1 }` within five seconds.
   The welcome contains the shared content hash and that guest's player view.
   `GET /api/rules` supplies public rule tables and the same content hash.
6. Use `ClientMessage`/`ServerMessage` from `civ-sim::protocol` for commands,
   queries and resynchronization. Commands never accept a civilization/actor ID.
   Keep the same command identity and payload when retrying after lost receipts.

The room response contains its dimensions/capacity, occupied-seat count, caller's
seat, owner status, turn duration and match ID. Only the owner sees the invite.
It contains no map seed, credentials or world snapshot.

It also lists `members`, one per occupied seat:
`{ seat, name: string | null, isOwner, left, connected?, ready?, controlled? }`.
Before the start, `connected` says whether the member made a room call (the
lobby reads `GET /api/rooms/{roomId}` every two seconds) in the last six
seconds; this presence is kept in memory only. Once the match runs, `connected`
says the host holds a live socket for the seat, and `ready` and `controlled`
appear: it finished the current turn, and a person (not a bot) plays it. A room
that plays a Blitz slice names it in `slice`.

## Returning players and leaving

`GET /api/rooms` answers `{ rooms: RoomSummary[] }`, every room the guest holds a
seat in, lobbies and matches alike, ordered by room id:
`{ roomId, matchId: string | null, ownSeat, isOwner, occupiedSeats, capacity,
turn?, finished }` (`turn` once the match started). Rooms it left are not listed.

`POST /api/rooms/{roomId}/leave` answers `{ "left": true }`. Before the start, a
member gives up its seat and its nation choice (the next guest to join takes the
lowest free seat), and the owner closes the room, which then answers 404. After
the start, leaving is final: the seat passes to a bot at the next round boundary
(`pendingControl: false`), its socket is fenced, and the guest can no longer
connect (403). The seat shows `left: true` to the other members. Leaving a room
one holds no seat in answers 403.

Inside a match, every `GameUpdate` of a room with more than one human seat
carries `seats: SeatStatusView[]`: `{ seat, ready, controlled, connected?, name?,
left? }` for every human seat. The session records connects and disconnects as
inputs, so the other members receive an update when someone connects, drops, or
finishes the turn. A restart marks every seat disconnected until it reconnects.

## Ordering and recovery

One mutex orders each match's inputs, clock decisions and controller changes.
A command is staged on a copy; its snapshot, host metadata and receipt commit in
one SQLite transaction before the live session is replaced and the receipt sent.
A failed transaction publishes no mutation. Duplicate commands return the saved
receipt after reconnect or restart. Malformed requests are rejected without
being stored as commands. SQLite uses WAL and full synchronous commits.

Every round has one persisted deadline. Commands check expired deadlines under
the same mutex as the periodic clock. `deadlineMs` in every game update is an
epoch-millisecond decimal string, or null after the match finishes.

A new connection increments and persists that seat's connection generation,
fencing the previous socket. A disconnect allows 15 seconds to reconnect.
An expired grace schedules bot control at the next round boundary; returning
players regain control at the next boundary. `controlled` and `pendingControl`
make this explicit. The server pings every ten seconds and disconnects a peer
after thirty seconds without an incoming frame. Restarted connections receive
a fresh grace while existing pending grace/controller decisions remain durable.

Incoming messages are limited to 64 KiB and 128 frames/second per connection.
Each seat has a 16-update queue. Lag and explicit resync return an absolute
snapshot without transient events; older queued revisions cannot replace it.
Outgoing writes have a five-second timeout. Maps are capped at 64×40 with up to
eight major civilizations and eight city-states. Guest/room/history capacities
are finite; automatic archival and cleanup are not implemented.

This is a single-process private-room server. Public matchmaking, ranked play,
account recovery, distributed hosting and production operations remain outside
this slice. No interface changes or deployment are included.

Validate with `cargo test -p civ-server`. Tests include two actual WebSocket
clients, seat isolation, owner/origin checks, replacement sockets, failed-commit
rollback, durable retry after restart, replay verification, exclusive database
ownership, deadline/controller boundary behavior, seat presence and names,
leaving before and after the start, the returning player's room list and the
version 1 to 2 database migration (which adds `guests.name`).
