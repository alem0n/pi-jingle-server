//! Input types for the task board.
//!
//! The UI consumes the same JSON body that `POST /api/jingle/notify` receives
//! (see the crate root). Parsing here is deliberately lenient: a missing or
//! mistyped optional field becomes `None` instead of an error, so a broken
//! event can never panic the window.
//!
//! No raw JSON is kept after parsing. [`AgentEvent`] holds only the fields the
//! board needs, and the board stores only derived values.

use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;
use std::fmt;

/// One notification, already parsed and normalized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEvent {
    /// Event name from the `event` field.
    pub kind: EventKind,
    /// Raw `timestamp` field (UTC epoch milliseconds, normally). The board
    /// converts it with [`UtcMillis::from_epoch_field`], which detects epoch
    /// seconds. `None` when the field is missing or not a number.
    pub timestamp: Option<i64>,
    /// Fields of `session_data`.
    pub session: SessionInput,
}

impl AgentEvent {
    /// Parse the JSON body of one notification.
    pub fn from_json(text: &str) -> Result<Self, EventError> {
        Self::from_json_bytes(text.as_bytes())
    }

    /// Parse the JSON body of one notification from raw bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, EventError> {
        let raw: RawEvent = serde_json::from_slice(bytes).map_err(|e| EventError {
            message: e.to_string(),
        })?;
        Ok(raw.into_agent_event())
    }
}

/// The three events the board acts on. Every other name is kept as [`Self::Other`]
/// so the caller can log and ignore it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// `session_start` — pi starts.
    SessionStart,
    /// `agent_start` — a task starts.
    AgentStart,
    /// `agent_settled` — a task finished; one card is added.
    AgentSettled,
    /// Any other value, including an empty or missing `event` field.
    Other(String),
}

impl EventKind {
    /// Classify a raw `event` value. Surrounding whitespace is ignored.
    pub fn from_raw(raw: &str) -> Self {
        match raw.trim() {
            "session_start" => Self::SessionStart,
            "agent_start" => Self::AgentStart,
            "agent_settled" => Self::AgentSettled,
            other => Self::Other(other.to_string()),
        }
    }

    /// The raw name of this event.
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "session_start",
            Self::AgentStart => "agent_start",
            Self::AgentSettled => "agent_settled",
            Self::Other(raw) => raw,
        }
    }

    /// `true` for [`Self::Other`].
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Other(_))
    }
}

/// Fields of `session_data`. Every field is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInput {
    /// Session identifier. Empty after trimming means "not usable".
    pub session_id: Option<String>,
    /// RFC 3339 creation time, as sent.
    pub created_at: Option<String>,
    /// Working directory candidate for the card title.
    pub cwd: Option<String>,
    /// Session name, the highest-priority title candidate.
    pub session_name: Option<String>,
    /// `persisted` flag, kept for optional display.
    pub persisted: Option<bool>,
    /// Session version, kept for optional display.
    pub version: Option<u64>,
}

impl SessionInput {
    /// Build from the placeholder-typed fields of the HTTP receiver, where
    /// missing values are the literal text `Unknown`.
    pub fn from_legacy_placeholders(
        session_id: &str,
        created_at: &str,
        cwd: &str,
        session_name: &str,
        persisted: &str,
        version: &str,
    ) -> Self {
        Self {
            session_id: placeholder_to_option(session_id),
            created_at: placeholder_to_option(created_at),
            cwd: placeholder_to_option(cwd),
            session_name: placeholder_to_option(session_name),
            persisted: parse_bool(persisted),
            version: version.trim().parse().ok(),
        }
    }

    /// Trimmed `session_id`, or `None` when it is missing or blank.
    pub fn session_id_value(&self) -> Option<&str> {
        usable(self.session_id.as_deref())
    }

    /// Trimmed `session_name`, or `None` when it is missing or blank.
    pub fn session_name_value(&self) -> Option<&str> {
        usable(self.session_name.as_deref())
    }

    /// Trimmed `cwd`, or `None` when it is missing or blank.
    pub fn cwd_value(&self) -> Option<&str> {
        usable(self.cwd.as_deref())
    }
}

/// Text that is not empty after trimming.
fn usable(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

/// The receiver uses `Unknown` for absent fields. Treat it as absent.
fn placeholder_to_option(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Parse a boolean from a typed or textual field.
fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Failure to parse an event body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventError {
    /// Parser message, for logs.
    pub message: String,
}

impl fmt::Display for EventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid task board event: {}", self.message)
    }
}

impl std::error::Error for EventError {}

// ---------------------------------------------------------------------------
// Serde layer
// ---------------------------------------------------------------------------

/// The JSON body of one notification.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawEvent {
    /// `session_data`, which may be missing or `null`.
    #[serde(default)]
    pub session_data: Option<RawSessionData>,
    /// `event`, which may be missing.
    #[serde(default, deserialize_with = "opt_string")]
    pub event: Option<String>,
    /// `timestamp` in UTC epoch milliseconds.
    #[serde(default, deserialize_with = "opt_millis")]
    pub timestamp: Option<i64>,
}

/// `session_data` as sent. Unknown keys (for example a misspelled `create_at`)
/// are ignored.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawSessionData {
    /// `session_id`.
    #[serde(default, deserialize_with = "opt_string")]
    pub session_id: Option<String>,
    /// `persisted`.
    #[serde(default, deserialize_with = "opt_bool")]
    pub persisted: Option<bool>,
    /// `created_at`, an RFC 3339 string.
    #[serde(default, deserialize_with = "opt_string")]
    pub created_at: Option<String>,
    /// `version`.
    #[serde(default, deserialize_with = "opt_u64")]
    pub version: Option<u64>,
    /// `cwd`.
    #[serde(default, deserialize_with = "opt_string")]
    pub cwd: Option<String>,
    /// `session_name`.
    #[serde(default, deserialize_with = "opt_string")]
    pub session_name: Option<String>,
}

impl RawEvent {
    /// Convert the parsed body into a normalized event.
    pub fn into_agent_event(self) -> AgentEvent {
        let kind = self
            .event
            .as_deref()
            .map(EventKind::from_raw)
            .unwrap_or_else(|| EventKind::Other(String::new()));
        let session = self
            .session_data
            .map(SessionInput::from)
            .unwrap_or_default();
        AgentEvent {
            kind,
            timestamp: self.timestamp,
            session,
        }
    }
}

impl From<RawSessionData> for SessionInput {
    fn from(raw: RawSessionData) -> Self {
        Self {
            session_id: raw.session_id,
            created_at: raw.created_at,
            cwd: raw.cwd,
            session_name: raw.session_name,
            persisted: raw.persisted,
            version: raw.version,
        }
    }
}

/// Deserialize a string field. Numbers and booleans are converted to text so a
/// mistyped sender cannot break the whole event.
fn opt_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptString;

    impl<'de> Visitor<'de> for OptString {
        type Value = Option<String>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string, a number, or null")
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(OptString)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
    }

    deserializer.deserialize_option(OptString)
}

/// Deserialize an optional boolean from `true`/`false`, `1`/`0`, or text.
fn opt_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptBool;

    impl<'de> Visitor<'de> for OptBool {
        type Value = Option<bool>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a boolean, 0, 1, or null")
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(OptBool)
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v != 0))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v != 0))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(parse_bool(v))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(parse_bool(&v))
        }
    }

    deserializer.deserialize_option(OptBool)
}

/// Deserialize an optional unsigned integer from a number or numeric text.
fn opt_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptU64;

    impl<'de> Visitor<'de> for OptU64 {
        type Value = Option<u64>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an unsigned integer or null")
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(OptU64)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(u64::try_from(v).ok())
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.trim().parse().ok())
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(v.trim().parse().ok())
        }
    }

    deserializer.deserialize_option(OptU64)
}

/// Deserialize the `timestamp` field. Accepts an integer, a float, or numeric
/// text. Anything else becomes `None`; the board then shows `—` for that time.
fn opt_millis<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptMillis;

    impl<'de> Visitor<'de> for OptMillis {
        type Value = Option<i64>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an epoch millisecond integer or null")
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(OptMillis)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(i64::try_from(v).ok())
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            if v.is_finite() {
                Ok(Some(v.round() as i64))
            } else {
                Ok(None)
            }
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.trim().parse().ok())
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(v.trim().parse().ok())
        }
    }

    deserializer.deserialize_option(OptMillis)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "session_data": {
            "session_id": "7f3c9a2e-1b4d-4c8a-9e2f-3a1b2c3d4e5f",
            "persisted": true,
            "created_at": "2025-09-12T10:30:00.000Z",
            "version": 3,
            "cwd": "/home/user/project",
            "session_name": "Refactor auth module"
        },
        "event": "agent_settled",
        "timestamp": 1757626496789
    }"#;

    #[test]
    fn parses_the_sample_body() {
        let event = AgentEvent::from_json(SAMPLE).unwrap();
        assert_eq!(event.kind, EventKind::AgentSettled);
        assert_eq!(event.timestamp, Some(1_757_626_496_789));
        assert_eq!(
            event.session.session_id.as_deref(),
            Some("7f3c9a2e-1b4d-4c8a-9e2f-3a1b2c3d4e5f")
        );
        assert_eq!(
            event.session.session_name.as_deref(),
            Some("Refactor auth module")
        );
        assert_eq!(event.session.cwd.as_deref(), Some("/home/user/project"));
        assert_eq!(
            event.session.created_at.as_deref(),
            Some("2025-09-12T10:30:00.000Z")
        );
        assert_eq!(event.session.persisted, Some(true));
        assert_eq!(event.session.version, Some(3));
    }

    #[test]
    fn tolerates_a_missing_session_data_and_event() {
        let event = AgentEvent::from_json("{}").unwrap();
        assert!(event.kind.is_unknown());
        assert_eq!(event.timestamp, None);
        assert_eq!(event.session, SessionInput::default());
    }

    #[test]
    fn tolerates_null_fields() {
        let event = AgentEvent::from_json(
            r#"{"session_data":{"session_id":null,"created_at":null,"cwd":null,"session_name":null,"persisted":null,"version":null},"event":"agent_settled","timestamp":null}"#,
        )
        .unwrap();
        assert_eq!(event.session, SessionInput::default());
        assert_eq!(event.timestamp, None);
    }

    #[test]
    fn ignores_a_misspelled_field_name() {
        let event = AgentEvent::from_json(
            r#"{"session_data":{"session_id":"s1","create_at":"2025-09-12T10:30:00Z"},"event":"agent_settled","timestamp":1}"#,
        )
        .unwrap();
        assert_eq!(event.session.session_id.as_deref(), Some("s1"));
        assert_eq!(event.session.created_at, None);
    }

    #[test]
    fn accepts_text_numbers_and_booleans() {
        let event = AgentEvent::from_json(
            r#"{"session_data":{"session_id":"s1","persisted":"false","version":"7"},"event":"agent_settled","timestamp":"1757626496789"}"#,
        )
        .unwrap();
        assert_eq!(event.session.persisted, Some(false));
        assert_eq!(event.session.version, Some(7));
        assert_eq!(event.timestamp, Some(1_757_626_496_789));
    }

    #[test]
    fn rejects_non_object_json() {
        assert!(AgentEvent::from_json("not json").is_err());
        assert!(AgentEvent::from_json("[1,2,3]").is_err());
    }

    #[test]
    fn unknown_event_names_are_kept() {
        let kind = EventKind::from_raw("agent_finished");
        assert_eq!(kind, EventKind::Other("agent_finished".to_string()));
        assert!(kind.is_unknown());
        assert_eq!(
            EventKind::from_raw(" agent_settled "),
            EventKind::AgentSettled
        );
    }

    #[test]
    fn placeholder_fields_become_absent() {
        let session = SessionInput::from_legacy_placeholders(
            "Unknown", "Unknown", "Unknown", "Unknown", "Unknown", "Unknown",
        );
        assert_eq!(session, SessionInput::default());

        let session = SessionInput::from_legacy_placeholders(
            "abc",
            "2025-09-12T10:30:00.000Z",
            "/tmp/x",
            "Name",
            "true",
            "3",
        );
        assert_eq!(session.session_id.as_deref(), Some("abc"));
        assert_eq!(session.persisted, Some(true));
        assert_eq!(session.version, Some(3));
    }
}
