# pi-jingle-server

Receiver server for [pi-jingle](../pi-jingle) remote event notifications.

Current stage (MVP): an HTTP server that **receives and prints every request** to the command line — timestamp (epoch ms), peer address, method, path + query, headers, and body.

It is the prototype counterpart of pi-jingle's planned `sounds-remote` block, which POSTs a fixed JSON payload to a fixed path:

```json
{ "session_id": "...", "event": "agent_settled", "timestamp": 1757626496789 }
```

See [../pi-jingle/docs/remote-notifications.md](../pi-jingle/docs/remote-notifications.md) for the full design.

## Run

```bash
cargo run            # listens on 0.0.0.0:8080
cargo run -- 3000    # custom port via arg
JINGLE_PORT=3000 cargo run   # or via env var
```

Then point a `sounds-remote` entry (once implemented) at this host, e.g. `127.0.0.1:8080`, or test manually:

```bash
curl -X POST http://127.0.0.1:8080/api/jingle/notify \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"abc-123","event":"agent_settled","timestamp":1757626496789}'
```

Every request prints a full dump to stdout and gets `200 {"ok":true}` back.
