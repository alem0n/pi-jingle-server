// pi-jingle-server
//
// Reference receiver for pi-jingle's `sounds-remote` remote notifications
// (see ../pi-jingle/README.md, section "API Endpoints").
//
// It parses the one fixed endpoint the extension sends to:
//
//   POST /api/jingle/notify
//     Content-Type: application/json
//     {
//       "session_data": { "session_id": "<uuid>|unknown", "persisted": <bool>,
//                          "created_at": "<ISO-8601>|null", "version": <n>|null,
//                          "cwd": "<path>|null", "session_name": "<name>" },
//       "event": "<name>", "timestamp": <epoch ms>
//     }
//
// Any session_data field that is missing or null is shown as "Unknown".
//
// Valid notifications are printed as one readable block per event (event,
// meaning, UTC time, session id, peer) and answered with `200 {"ok":true}`,
// the reply pi-jingle expects. Malformed requests (bad JSON, missing or
// wrongly typed fields, unknown event, non-JSON content type, oversized
// body) are answered with 400/413/415 and logged with the reason, so
// misconfigurations are easy to spot.
//
// Every other request (any method, any path) is still dumped verbatim, so
// this also works as a generic "print every request" debug listener.
//
// Usage:
//   pi-jingle-server [PORT]          (default 8080, or JINGLE_PORT env var)

// The task board window. It has no dependency on this module: the binary
// feeds it with `AgentEvent` values (see src/desktop.rs).
pub mod taskboard;

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, Request, State},
    http::{request::Parts, HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

const DEFAULT_PORT: u16 = 8080;
const MAX_BODY: usize = 1024 * 1024; // 1 MiB cap; larger bodies are truncated
pub const NOTIFY_PATH: &str = "/api/jingle/notify";
pub const EVENTS_PATH: &str = "/api/jingle/events";

// The fixed event set the endpoint can receive, with the README's wording.
// Note: "when_coding" is intentionally absent — the looped song has no
// remote meaning; pi-jingle maps it to agent_start + agent_settled instead.
pub const KNOWN_EVENTS: &[(&str, &str)] = &[
    ("agent_start", "A task starts"),
    (
        "agent_settled",
        "The task fully completes; Pi is idle and waits for your input",
    ),
    ("session_start", "pi starts"),
    ("session_shutdown", "pi closes"),
    ("turn_start", "A user message is received"),
    ("turn_end", "A response is sent"),
    ("tool_call", "A tool is called"),
    ("tool_result", "A tool result is received"),
];

// Shared server state: per-event counters (for the compact "(n=12)" output
// line), the in-memory log of accepted notifications served by
// GET /api/jingle/events, and a broadcast channel a UI can subscribe to for
// live updates. Cloning it shares the same underlying storage.
#[derive(Clone)]
pub struct AppState {
    counts: Arc<Mutex<HashMap<&'static str, u64>>>,
    events: Arc<Mutex<Vec<StoredEvent>>>,
    tx: broadcast::Sender<StoredEvent>,
}

impl Default for AppState {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            counts: Arc::default(),
            events: Arc::default(),
            tx,
        }
    }
}

impl AppState {
    /// Subscribe to live notifications as they are accepted. Each accepted
    /// notification is broadcast to every active subscriber (capacity 256;
    /// a slow subscriber may lag and receive `RecvError::Lagged`).
    pub fn subscribe(&self) -> broadcast::Receiver<StoredEvent> {
        self.tx.subscribe()
    }

    /// Snapshot of every accepted event, oldest first.
    pub fn snapshot(&self) -> Vec<StoredEvent> {
        self.snapshot_filtered(None, None)
    }

    /// Snapshot filtered by event name and/or limited to the most recent
    /// `limit` events. `event = None` keeps all events; `limit = None` keeps
    /// everything matched.
    pub fn snapshot_filtered(&self, event: Option<&str>, limit: Option<usize>) -> Vec<StoredEvent> {
        let events = self.events.lock().unwrap();
        let mut matched: Vec<StoredEvent> = events
            .iter()
            .filter(|e| event.is_none_or(|want| want == e.notification.event))
            .cloned()
            .collect();
        if let Some(n) = limit {
            if matched.len() > n {
                matched.drain(..matched.len() - n);
            }
        }
        matched
    }

    /// Copy of the per-event counters.
    pub fn counts(&self) -> HashMap<&'static str, u64> {
        self.counts.lock().unwrap().clone()
    }
}

// The fixed request body. Extra fields are ignored so future extension
// versions do not break this receiver.
#[derive(Clone, Deserialize, Serialize)]
pub struct Notification {
    #[serde(default)]
    pub session_data: SessionData,
    pub event: String,
    pub timestamp: u64,
}

// One accepted notification as served by GET /api/jingle/events: the parsed
// body plus the receive time and the client address.
#[derive(Clone, Serialize)]
pub struct StoredEvent {
    pub received_at: u64,
    pub from: String,
    #[serde(flatten)]
    pub notification: Notification,
}

// Deserialization helpers: a field may be absent or null, in which case it
// becomes the literal string "Unknown" instead of failing the request.
pub mod de {
    use serde::{Deserialize, Deserializer};
    use std::fmt::Display;

    pub fn unknown_string() -> String {
        "Unknown".to_string()
    }
    pub fn unknown_bool() -> String {
        "Unknown".to_string()
    }
    pub fn unknown_u64() -> String {
        "Unknown".to_string()
    }

    fn from_option<'de, T, D>(d: D) -> Result<String, D::Error>
    where
        T: Deserialize<'de> + Display,
        D: Deserializer<'de>,
    {
        Ok(Option::<T>::deserialize(d)?
            .map(|v| v.to_string())
            .unwrap_or_else(|| "Unknown".to_string()))
    }

    pub fn string_or_unknown<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        from_option::<String, D>(d)
    }
    pub fn bool_or_unknown<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        from_option::<bool, D>(d)
    }
    pub fn u64_or_unknown<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        from_option::<u64, D>(d)
    }
}

// Session metadata as sent by pi-jingle (see
// ../pi-jingle/docs/remote-notifications.md, section 6.2). Every field is
// nullable: a missing or null value deserializes to the string "Unknown"
// (persisted/version keep their typed shape as strings, e.g. "true"/"3").
#[derive(Clone, Deserialize, Serialize)]
pub struct SessionData {
    #[serde(
        default = "de::unknown_string",
        deserialize_with = "de::string_or_unknown"
    )]
    pub session_id: String,
    #[serde(default = "de::unknown_bool", deserialize_with = "de::bool_or_unknown")]
    pub persisted: String,
    #[serde(
        default = "de::unknown_string",
        deserialize_with = "de::string_or_unknown"
    )]
    pub created_at: String,
    #[serde(default = "de::unknown_u64", deserialize_with = "de::u64_or_unknown")]
    pub version: String,
    #[serde(
        default = "de::unknown_string",
        deserialize_with = "de::string_or_unknown"
    )]
    pub cwd: String,
    #[serde(
        default = "de::unknown_string",
        deserialize_with = "de::string_or_unknown"
    )]
    pub session_name: String,
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            session_id: de::unknown_string(),
            persisted: de::unknown_bool(),
            created_at: de::unknown_string(),
            version: de::unknown_u64(),
            cwd: de::unknown_string(),
            session_name: de::unknown_string(),
        }
    }
}

/// Run the HTTP server on `0.0.0.0:port` until the future is dropped or the
/// process exits. The caller keeps (a clone of) `state` to query or subscribe
/// to the event log from a UI.
pub async fn serve(state: AppState, port: u16) -> std::io::Result<()> {
    let addr = format!("0.0.0.0:{port}");

    let listener = TcpListener::bind(&addr).await?;

    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

/// Build the axum router for the given state: POST NOTIFY_PATH parses and
/// records notifications, GET EVENTS_PATH serves the log, everything else is
/// dumped verbatim.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(NOTIFY_PATH, post(handle_notify))
        .route(EVENTS_PATH, get(handle_events))
        .fallback(handle_request)
        .with_state(state)
}

pub fn port_from_args_or_env() -> u16 {
    std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .or_else(|| {
            std::env::var("JINGLE_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(DEFAULT_PORT)
}

// POST /api/jingle/notify — parse, validate, print, ack.
pub async fn handle_notify(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response<Body> {
    let (parts, body) = req.into_parts();

    let body: Bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return reject(
                peer,
                &parts,
                &Bytes::new(),
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("body exceeds the {MAX_BODY} byte cap"),
            );
        }
    };

    // The extension always sends application/json; anything else is a client bug.
    if let Some(false) = json_content_type(&parts.headers) {
        return reject(
            peer,
            &parts,
            &body,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        );
    }

    let note: Notification = match serde_json::from_slice(&body) {
        Ok(n) => n,
        Err(e) => {
            return reject(
                peer,
                &parts,
                &body,
                StatusCode::BAD_REQUEST,
                &format!("invalid JSON body: {e}"),
            );
        }
    };

    // Validate against the fixed event set from the README.
    let Some((event, what)) = KNOWN_EVENTS
        .iter()
        .find(|(name, _)| *name == note.event.as_str())
    else {
        let known: Vec<&str> = KNOWN_EVENTS.iter().map(|(n, _)| *n).collect();
        return reject(
            peer,
            &parts,
            &body,
            StatusCode::BAD_REQUEST,
            &format!(
                "unknown event: {} (expected one of: {})",
                note.event,
                known.join(", ")
            ),
        );
    };

    if note.session_data.session_id.trim().is_empty() {
        return reject(
            peer,
            &parts,
            &body,
            StatusCode::BAD_REQUEST,
            "session_data.session_id must not be empty",
        );
    }
    let n = {
        let mut counts = state.counts.lock().unwrap();
        let c = counts.entry(*event).or_insert(0);
        *c += 1;
        *c
    };

    let mut out = String::new();
    out.push_str(&"-".repeat(60));
    out.push('\n');
    out.push_str(&format!("[notify]  {event} (n={n})\n"));
    out.push_str(&format!(
        "[time]    {} UTC  ({} epoch ms)\n",
        format_utc_ms(note.timestamp),
        note.timestamp
    ));
    out.push_str(&format!("[what]    {what}\n"));
    out.push_str(&format!("[session] {}\n", note.session_data.session_id));
    out.push_str(&format!("[name]    {}\n", note.session_data.session_name));
    out.push_str(&format!("[cwd]     {}\n", note.session_data.cwd));
    out.push_str(&format!(
        "[meta]    persisted={} created_at={} version={}\n",
        note.session_data.persisted, note.session_data.created_at, note.session_data.version
    ));
    out.push_str(&format!("[from]    {peer}\n"));
    println!("{out}");

    // Record the accepted notification for GET /api/jingle/events and
    // broadcast it to live UI subscribers.
    let stored = StoredEvent {
        received_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        from: peer.to_string(),
        notification: note,
    };
    state.events.lock().unwrap().push(stored.clone());
    let _ = state.tx.send(stored);

    // Play the done jingle when the agent settles (non-blocking).
    if *event == "agent_settled" {
        play_sound("sounds/done.mp3");
    }

    ok_response()
}

// GET /api/jingle/events — serve the accepted notifications as JSON.
// Optional query params: `event=<name>` filters by event type, `limit=<n>`
// returns only the most recent n events (default: all).
pub async fn handle_events(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Response<Body> {
    let event = params.get("event").map(|s| s.as_str());
    let limit = params.get("limit").and_then(|s| s.parse::<usize>().ok());
    let selected = state.snapshot_filtered(event, limit);
    let body = serde_json::to_string(&selected).unwrap_or_else(|_| "[]".to_string());
    json_response(
        StatusCode::OK,
        &format!("{{\"count\":{},\"events\":{body}}}", selected.len()),
    )
}

// Play an audio file on a background thread via rodio, which is fully
// cross-platform (WASAPI on Windows, CoreAudio on macOS, ALSA/PulseAudio on
// Linux). Errors are only logged — playback failure must not affect the ack.
pub fn play_sound(path: &'static str) {
    std::thread::spawn(move || {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            // The stream must outlive the sink, hence the binding.
            let (_stream, handle) = rodio::OutputStream::try_default()?;
            let sink = rodio::Sink::try_new(&handle)?;
            // Resolve relative paths against the executable's directory, not
            // the current working directory, so the exe works when launched
            // from PATH with sounds/ next to it. build.rs copies sounds/ next
            // to the binary for both debug and release builds.
            let resolved = {
                let p = std::path::Path::new(path);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.parent().map(|d| d.join(p)))
                        .unwrap_or_else(|| p.to_path_buf())
                }
            };
            let file = std::fs::File::open(resolved)?;
            let source = rodio::Decoder::new(std::io::BufReader::new(file))?;
            sink.append(source);
            sink.sleep_until_end();
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("[sound]    failed to play {path}: {e}");
        }
    });
}

// Every other method and path: dump verbatim (the original MVP behavior).
pub async fn handle_request(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response<Body> {
    let (parts, body) = req.into_parts();

    // Collect the full body (capped) before printing so the dump is complete.
    let body: Bytes = axum::body::to_bytes(body, MAX_BODY)
        .await
        .unwrap_or_default();

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let mut out = String::new();
    out.push_str(&"-".repeat(60));
    out.push('\n');
    out.push_str(&format!("[time]    {ts} (epoch ms)\n"));
    out.push_str(&format!("[from]    {peer}\n"));
    out.push_str(&format!("[request] {} {}\n", parts.method, parts.uri));
    out.push_str("[headers]\n");
    for (name, value) in parts.headers.iter() {
        out.push_str(&format!(
            "  {}: {}\n",
            name,
            value.to_str().unwrap_or("<binary>")
        ));
    }
    if body.is_empty() {
        out.push_str("[body]    (empty)\n");
    } else {
        out.push_str(&format!("[body]    {}\n", String::from_utf8_lossy(&body)));
    }
    println!("{out}");

    // Minimal ack so clients (including pi-jingle) see a clean 200.
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from("{\"ok\":true}"))
        .unwrap()
}

/// Log a rejected request and answer with a JSON error body.
pub fn reject(
    peer: SocketAddr,
    parts: &Parts,
    body: &Bytes,
    status: StatusCode,
    reason: &str,
) -> Response<Body> {
    let mut out = String::new();
    out.push_str(&"-".repeat(60));
    out.push('\n');
    out.push_str(&format!(
        "[reject]  {} {} -> {status}\n",
        parts.method, parts.uri
    ));
    out.push_str(&format!("[from]    {peer}\n"));
    out.push_str(&format!("[reason]  {reason}\n"));
    if body.is_empty() {
        out.push_str("[body]    (empty)\n");
    } else {
        out.push_str(&format!("[body]    {}\n", String::from_utf8_lossy(body)));
    }
    println!("{out}");

    json_response(
        status,
        &serde_json::json!({ "ok": false, "error": reason }).to_string(),
    )
}

/// The reply pi-jingle expects: `200 {"ok":true}`.
pub fn ok_response() -> Response<Body> {
    json_response(StatusCode::OK, "{\"ok\":true}")
}

pub fn json_response(status: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

/// `None` when the header is absent; `Some(true)` when it is `application/json`
/// (parameters such as `; charset=utf-8` are ignored).
pub fn json_content_type(headers: &HeaderMap) -> Option<bool> {
    let value = headers.get("content-type")?.to_str().ok()?;
    let essence = value.split(';').next().unwrap_or("").trim();
    Some(essence.eq_ignore_ascii_case("application/json"))
}

/// Format epoch milliseconds as UTC `YYYY-MM-DD HH:MM:SS.mmm`.
pub fn format_utc_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let millis = ms % 1000;
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03}")
}

/// Days since 1970-01-01 to (year, month, day), Howard Hinnant's algorithm.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_notification() {
        let json = br#"{"session_data":{"session_id":"7f3c9a2e-1b4d-4c8a-9e2f-3a1b2c3d4e5f","persisted":true,"created_at":"2025-09-12T10:30:00.000Z","version":3,"cwd":"/home/user/project","session_name":"Refactor auth module"},"event":"agent_settled","timestamp":1757626496789}"#;
        let n: Notification = serde_json::from_slice(json).unwrap();
        assert_eq!(
            n.session_data.session_id,
            "7f3c9a2e-1b4d-4c8a-9e2f-3a1b2c3d4e5f"
        );
        assert_eq!(n.session_data.persisted, "true");
        assert_eq!(n.session_data.created_at, "2025-09-12T10:30:00.000Z");
        assert_eq!(n.session_data.version, "3");
        assert_eq!(n.session_data.cwd, "/home/user/project");
        assert_eq!(n.session_data.session_name, "Refactor auth module");
        assert_eq!(n.event, "agent_settled");
        assert_eq!(n.timestamp, 1757626496789);
    }

    #[test]
    fn null_or_missing_session_fields_become_unknown() {
        // Explicit nulls become "Unknown".
        let json = br#"{"session_data":{"session_id":"unknown","session_name":"x","persisted":null,"created_at":null,"version":null,"cwd":null},"event":"tool_call","timestamp":1}"#;
        let n: Notification = serde_json::from_slice(json).unwrap();
        assert_eq!(n.session_data.persisted, "Unknown");
        assert_eq!(n.session_data.created_at, "Unknown");
        assert_eq!(n.session_data.version, "Unknown");
        assert_eq!(n.session_data.cwd, "Unknown");

        // Missing session_data (or missing fields) becomes "Unknown" too.
        let n: Notification =
            serde_json::from_slice(br#"{"event":"tool_call","timestamp":1}"#).unwrap();
        assert_eq!(n.session_data.session_id, "Unknown");
        assert_eq!(n.session_data.session_name, "Unknown");
    }

    #[test]
    fn rejects_missing_or_mistyped_fields() {
        // timestamp + event missing
        assert!(serde_json::from_slice::<Notification>(br#"{}"#).is_err());
        // timestamp is a string, not epoch ms
        assert!(serde_json::from_slice::<Notification>(
            br#"{"session_data":{"session_id":"x","session_name":"y"},"event":"turn_end","timestamp":"now"}"#
        )
        .is_err());
        // negative timestamp is not a valid epoch ms
        assert!(serde_json::from_slice::<Notification>(
            br#"{"session_data":{"session_id":"x","session_name":"y"},"event":"turn_end","timestamp":-1}"#
        )
        .is_err());
        // a mistyped persisted (array) is rejected, not coerced
        assert!(serde_json::from_slice::<Notification>(
            br#"{"session_data":{"persisted":[1]},"event":"turn_end","timestamp":1}"#
        )
        .is_err());
    }

    #[test]
    fn ignores_unknown_fields() {
        let json = br#"{"session_data":{"session_id":"unknown","session_name":"x"},"event":"tool_call","timestamp":1,"extra":true}"#;
        assert!(serde_json::from_slice::<Notification>(json).is_ok());
    }

    #[test]
    fn known_events_cover_the_readme_set() {
        let names: Vec<&str> = KNOWN_EVENTS.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec![
                "agent_start",
                "agent_settled",
                "session_start",
                "session_shutdown",
                "turn_start",
                "turn_end",
                "tool_call",
                "tool_result",
            ]
        );
    }

    #[test]
    fn epoch_ms_formats_as_utc() {
        // The README example timestamp.
        assert_eq!(format_utc_ms(1757626496789), "2025-09-11 21:34:56.789");
        assert_eq!(format_utc_ms(0), "1970-01-01 00:00:00.000");
        // Leap day: 2024-02-29 12:00:00 UTC.
        assert_eq!(format_utc_ms(1709208000000), "2024-02-29 12:00:00.000");
    }

    #[test]
    fn content_type_match_ignores_parameters() {
        let mut headers = HeaderMap::new();
        assert_eq!(json_content_type(&headers), None);
        headers.insert("content-type", "application/json".parse().unwrap());
        assert_eq!(json_content_type(&headers), Some(true));
        headers.insert(
            "content-type",
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert_eq!(json_content_type(&headers), Some(true));
        headers.insert("content-type", "text/plain".parse().unwrap());
        assert_eq!(json_content_type(&headers), Some(false));
    }
}
