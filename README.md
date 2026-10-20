# pi-jingle-server

Receiver server for [pi-jingle](../pi-jingle) remote event notifications.

It parses the one fixed endpoint the extension sends to (see
[../pi-jingle/README.md — API Endpoints](../pi-jingle/README.md#api-endpoints)):

- `POST /api/jingle/notify`
- `Content-Type: application/json`
- Body:
  ```json
  {
    "session_data": {
      "session_id": "<uuid>|unknown",
      "persisted": true,
      "created_at": "2025-09-12T10:30:00.000Z|null",
      "version": 3,
      "cwd": "/home/user/project|null",
      "session_name": "Refactor auth module"
    },
    "event": "<name>",
    "timestamp": <epoch ms>
  }
  ```

Only `event` and `timestamp` are strictly required. Any `session_data` field
that is missing or `null` (or `session_data` itself) is tolerated and shown
as `Unknown` instead of being rejected.

Valid notifications are printed as one readable block per event — event name,
its meaning, UTC time, session id, peer — and answered with `200 {"ok":true}`,
the reply pi-jingle expects. Malformed requests are answered with a JSON error
and logged with the reason:

| Problem | Status |
|---|---|
| Bad JSON, missing/mistyped fields (e.g. non-integer timestamp) | `400` |
| `event` not in the fixed event set (`agent_start`, `agent_settled`, `session_start`, `session_shutdown`, `turn_start`, `turn_end`, `tool_call`, `tool_result`) | `400` |
| Empty `session_data.session_id` (empty string; missing/null becomes `Unknown`) | `400` |
| Body larger than 1 MiB | `413` |
| `Content-Type` other than `application/json` | `415` |
| Non-`POST` on the notify path | `405` |

Any other request (any method, any path) is still dumped verbatim — timestamp,
peer, method, path, headers, body — so this also works as a generic
"print every request" debug listener.

## Agent task board window

The program also opens a Tauri window. The window shows one card for each
`agent_settled` event. The window groups the cards by `session_id`, and each
group keeps its cards newest first. The window keeps all data in memory. It does
not write files.

The window has these features:

- One new card for each `agent_settled` event. Equal titles do not merge cards.
- One title per card: `session_name`, then `cwd`, then `session_id`, then `—`.
- Start time, request time and duration. The board stores times in UTC and shows
  them in local time.
- One delete button per card.
- A park function. A drag to the top edge moves the whole window above the
  screen; only an 8 px band of its bottom edge stays visible. A hover or a click
  moves the window back into the screen. A pointer leave moves it out again. A
  drag keeps the window open. The window is never resized for this.
- A slide. A park and an expand move the window in 160 ms with an ease-out
  curve. A drag stops the slide at once.
- A status light in the band. When a card arrives while the window is parked,
  the light blinks every 450 ms. It stops when the user opens the window.
- A close button in the top-left corner of the header. It exits the program.
- A card detail panel. A hover shows it after 280 ms; a click shows it at once
  and pins it. The panel is a second window, so it opens outside the task board
  window: left or right of it, at the height of the row. When both sides have
  room, it opens on the left side. It shows the complete card: title source,
  second value, session, persisted, version, times in local time, UTC and epoch
  ms, the start basis, the duration and an anomaly note.

See [docs/taskboard-ui.md](docs/taskboard-ui.md) for the data model, the public
API, the geometry constants, the design decisions and the manual test checklist.

## Build

```bash
cargo build                        # debug build with the window (target/debug/)
cargo build --release              # optimized build with the window
cargo build --no-default-features  # receiver only, no Tauri dependency
```

The first build compiles Tauri and the WebView2 bindings. It needs the WebView2
runtime on Windows. Windows 10 and Windows 11 include this runtime.

## Run

The program starts the receiver and opens the task board window.

```bash
cargo run            # receiver on 0.0.0.0:8080 and the window
cargo run -- 3000    # custom port via arg
JINGLE_PORT=3000 cargo run   # or via env var
```

Use the headless build when no window is necessary:

```bash
cargo run --no-default-features
```

On Windows the program does not open a console window. The window is the only
visible part. Logs go to the terminal that started the program. Use a
redirection to keep the log when you start the program from a script:

```powershell
Start-Process .\target\debug\pi-jingle-server.exe `
  -RedirectStandardOutput target\app.log -RedirectStandardError target\app.err
```

## Test

Point a `sounds-remote` entry at this host, e.g. `<your-lan-ip>:8080`, or test
manually:

```bash
curl -X POST http://127.0.0.1:8080/api/jingle/notify \
  -H 'Content-Type: application/json' \
  -d '{"session_data":{"session_id":"abc-123"},"event":"agent_settled","timestamp":1757626496789}'
# 200 {"ok":true}

curl -X POST http://127.0.0.1:8080/api/jingle/notify \
  -H 'Content-Type: application/json' \
  -d '{"session_data":{"session_id":"abc-123"},"event":"agent_finished"}'
# 400 {"ok":false,"error":"unknown event: agent_finished (...)"}
```

The unit tests cover parsing, the event set and epoch-ms to UTC conversion. The
task board tests cover the map, the title order, the start time order, the
duration, the delete command, the snap geometry, the slide steps, the
unseen-card tracker and the detail panel placement:

```bash
cargo test
cargo clippy --all-targets
```
