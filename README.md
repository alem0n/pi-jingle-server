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

## Build

```bash
cargo build           # debug build (target/debug/)
cargo build --release # release build, optimized (target/release/)
```

## Run

```bash
cargo run            # listens on 0.0.0.0:8080
cargo run -- 3000    # custom port via arg
JINGLE_PORT=3000 cargo run   # or via env var
```

## Test

Point a `sounds-remote` entry at this host, e.g. `192.168.1.50:8080`, or test
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

Unit tests for parsing, the event set, and epoch-ms→UTC conversion:

```bash
cargo test
```
