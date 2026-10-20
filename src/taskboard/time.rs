//! UTC time handling for the task board.
//!
//! Internal representation: [`UtcMillis`], a Unix epoch timestamp in
//! milliseconds, always UTC. Every stored time field uses this type.
//!
//! Two sources are unified here:
//!
//! * `event.timestamp` — a UTC epoch millisecond integer. Values that look
//!   like epoch seconds are converted (see [`UtcMillis::from_epoch_field`]).
//! * `session_data.created_at` — an RFC 3339 string, parsed by
//!   [`UtcMillis::from_rfc3339`] (chrono `parse_from_rfc3339`).
//!
//! Rendering converts UTC to the local time zone with chrono
//! [`chrono::Local`]. When the local zone cannot resolve an instant (DST gap
//! or no zone data) the renderer falls back to UTC text with a `UTC` suffix.

use chrono::{DateTime, Local, LocalResult, TimeZone, Utc};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch milliseconds per second.
pub const MS_PER_SECOND: i64 = 1_000;

/// Values below this limit are treated as epoch *seconds*.
///
/// `100_000_000_000` ms is 1973-03-03T09:46:40Z, so a plausible modern
/// timestamp in seconds (about `1.7e9`) is always detected, while modern
/// millisecond timestamps (about `1.7e12`) are never converted.
pub const SECONDS_AS_MS_LIMIT: i64 = 100_000_000_000;

/// Values below this limit are never treated as epoch seconds.
///
/// `1_000_000_000` seconds is 2001-09-09T01:46:40Z. A smaller number is not a
/// realistic epoch-second value for this program, so it stays milliseconds.
pub const MIN_PLAUSIBLE_EPOCH_SECONDS: i64 = 1_000_000_000;

/// A UTC instant, stored as Unix epoch milliseconds.
///
/// The value is always UTC. Local time exists only in rendered text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct UtcMillis(i64);

impl UtcMillis {
    /// The Unix epoch (1970-01-01T00:00:00Z).
    pub const EPOCH: Self = Self(0);

    /// Create from an exact epoch millisecond value. The value is not changed.
    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    /// Current UTC time from the system clock.
    pub fn now() -> Self {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Self(ms)
    }

    /// Convert an event timestamp field into UTC milliseconds.
    ///
    /// Returns the value plus `true` when a seconds value was converted to
    /// milliseconds. A negative value is not a valid timestamp and yields
    /// `None`.
    pub fn from_epoch_field(raw: i64) -> Option<(Self, bool)> {
        if raw < 0 {
            return None;
        }
        if (MIN_PLAUSIBLE_EPOCH_SECONDS..SECONDS_AS_MS_LIMIT).contains(&raw) {
            return Some((Self(raw.saturating_mul(MS_PER_SECOND)), true));
        }
        Some((Self(raw), false))
    }

    /// Parse an RFC 3339 timestamp (for example `2025-09-12T10:30:00.000Z`)
    /// into UTC milliseconds. Returns `None` when the text is not RFC 3339.
    pub fn from_rfc3339(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        DateTime::parse_from_rfc3339(trimmed)
            .ok()
            .map(|dt| Self(dt.timestamp_millis()))
    }

    /// The raw epoch millisecond value.
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// Difference in milliseconds, clamped at zero so a start time after the
    /// request time never produces a negative duration.
    pub fn saturating_since(self, start: Self) -> i64 {
        self.0.saturating_sub(start.0).max(0)
    }

    /// `true` when this instant is before `other`.
    pub fn is_before(self, other: Self) -> bool {
        self.0 < other.0
    }

    /// The instant as a UTC `DateTime`, or `None` when it is out of range.
    pub fn utc(self) -> Option<DateTime<Utc>> {
        Utc.timestamp_millis_opt(self.0).single()
    }

    /// The instant as a local `DateTime`. `None` when the local zone cannot
    /// resolve the instant.
    pub fn local(self) -> Option<DateTime<Local>> {
        match Local.timestamp_millis_opt(self.0) {
            LocalResult::Single(dt) => Some(dt),
            LocalResult::Ambiguous(dt, _) => Some(dt),
            LocalResult::None => None,
        }
    }

    /// Render for display. `zone` selects local time (default) or UTC.
    pub fn display(self, zone: Zone) -> DisplayTime {
        let Some(utc) = self.utc() else {
            return DisplayTime::missing();
        };

        if zone == Zone::Local {
            if let Some(local) = self.local() {
                return DisplayTime {
                    full: local.format("%Y-%m-%d %H:%M:%S").to_string(),
                    clock: local.format("%H:%M:%S").to_string(),
                    utc_fallback: false,
                };
            }
        }

        DisplayTime {
            full: format!("{} UTC", utc.format("%Y-%m-%d %H:%M:%S%.3f")),
            clock: utc.format("%H:%M:%S").to_string(),
            utc_fallback: zone == Zone::Local,
        }
    }

    /// UTC text with milliseconds, for tooltips and cross-checks.
    pub fn utc_text(self) -> Option<String> {
        self.utc()
            .map(|dt| format!("{} UTC", dt.format("%Y-%m-%d %H:%M:%S%.3f")))
    }
}

/// Time zone used when the board renders times.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    /// The operating system's local time zone.
    #[default]
    Local,
    /// UTC, used when local conversion is not wanted or not possible.
    Utc,
}

/// Rendered form of one instant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayTime {
    /// `YYYY-MM-DD HH:MM:SS`, or `YYYY-MM-DD HH:MM:SS.mmm UTC` on fallback.
    pub full: String,
    /// `HH:MM:SS`.
    pub clock: String,
    /// `true` when the text is UTC because local conversion failed.
    pub utc_fallback: bool,
}

impl DisplayTime {
    /// Empty text, used for an unknown time.
    pub fn missing() -> Self {
        Self::default()
    }

    /// `true` when no text is available.
    pub fn is_missing(&self) -> bool {
        self.full.is_empty()
    }
}

/// Human-readable label of the zone in use, for example `UTC+08:00`.
///
/// Returns `(label, local_failed)`. `local_failed` is `true` when local
/// conversion failed and the label is plain `UTC`.
pub fn zone_label(zone: Zone, at: UtcMillis) -> (String, bool) {
    match zone {
        Zone::Utc => ("UTC".to_string(), false),
        Zone::Local => match at.local() {
            Some(dt) => (format_utc_offset(dt.offset().local_minus_utc()), false),
            None => ("UTC".to_string(), true),
        },
    }
}

/// Format an offset in seconds as `UTC+08:00` / `UTC-03:30`.
pub fn format_utc_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let abs = seconds.unsigned_abs();
    let hours = abs / 3600;
    let minutes = (abs % 3600) / 60;
    format!("UTC{sign}{hours:02}:{minutes:02}")
}

/// Format a duration in milliseconds as readable text.
///
/// Examples: `0s`, `3.250s`, `2m 05s`, `1h 02m 05s`.
pub fn format_duration(ms: i64) -> String {
    let ms = ms.max(0);
    let total_seconds = ms / MS_PER_SECOND;
    let millis = ms % MS_PER_SECOND;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else if total_seconds > 0 {
        format!("{seconds}.{millis:03}s")
    } else if millis > 0 {
        format!("0.{millis:03}s")
    } else {
        "0s".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_sample_rfc3339_time() {
        let parsed = UtcMillis::from_rfc3339("2025-09-12T10:30:00.000Z").unwrap();
        assert_eq!(parsed.as_millis(), 1_757_673_000_000);
        assert_eq!(parsed.utc_text().unwrap(), "2025-09-12 10:30:00.000 UTC");
    }

    #[test]
    fn parses_rfc3339_offsets_and_fractions() {
        let a = UtcMillis::from_rfc3339("2025-09-12T18:30:00+08:00").unwrap();
        let b = UtcMillis::from_rfc3339("2025-09-12T10:30:00Z").unwrap();
        assert_eq!(a, b);
        let c = UtcMillis::from_rfc3339("2025-09-12T10:30:00.250Z").unwrap();
        assert_eq!(c.as_millis(), b.as_millis() + 250);
    }

    #[test]
    fn rejects_invalid_rfc3339() {
        assert_eq!(UtcMillis::from_rfc3339(""), None);
        assert_eq!(UtcMillis::from_rfc3339("   "), None);
        assert_eq!(UtcMillis::from_rfc3339("2025-09-12 10:30:00"), None);
        assert_eq!(UtcMillis::from_rfc3339("Unknown"), None);
    }

    #[test]
    fn epoch_field_detects_seconds() {
        let (ms, corrected) = UtcMillis::from_epoch_field(1_757_626_496_789).unwrap();
        assert_eq!(ms.as_millis(), 1_757_626_496_789);
        assert!(!corrected);

        let (ms, corrected) = UtcMillis::from_epoch_field(1_757_626_496).unwrap();
        assert_eq!(ms.as_millis(), 1_757_626_496_000);
        assert!(corrected);

        // Small values are not plausible epoch seconds; keep them as ms.
        let (ms, corrected) = UtcMillis::from_epoch_field(1_000).unwrap();
        assert_eq!(ms.as_millis(), 1_000);
        assert!(!corrected);

        assert_eq!(UtcMillis::from_epoch_field(-1), None);
    }

    #[test]
    fn timestamp_and_created_at_use_the_same_internal_type() {
        let from_ms = UtcMillis::from_epoch_field(1_757_673_000_000).unwrap().0;
        let from_text = UtcMillis::from_rfc3339("2025-09-12T10:30:00.000Z").unwrap();
        assert_eq!(from_ms, from_text);
        assert_eq!(from_ms.saturating_since(from_text), 0);
    }

    #[test]
    fn duration_is_never_negative() {
        let start = UtcMillis::from_millis(1_000);
        let request = UtcMillis::from_millis(5_500);
        assert_eq!(request.saturating_since(start), 4_500);
        assert_eq!(start.saturating_since(request), 0);
    }

    #[test]
    fn duration_format_is_readable() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(-5), "0s");
        assert_eq!(format_duration(250), "0.250s");
        assert_eq!(format_duration(3_250), "3.250s");
        assert_eq!(format_duration(125_000), "2m 05s");
        assert_eq!(format_duration(3_725_000), "1h 02m 05s");
    }

    #[test]
    fn utc_display_is_exact() {
        let t = UtcMillis::from_millis(1_757_626_496_789);
        let shown = t.display(Zone::Utc);
        assert_eq!(shown.full, "2025-09-11 21:34:56.789 UTC");
        assert_eq!(shown.clock, "21:34:56");
        assert!(!shown.utc_fallback);
    }

    #[test]
    fn local_display_matches_the_utc_instant() {
        let t = UtcMillis::from_millis(1_757_626_496_789);
        let utc = t.utc().unwrap();
        let local = t.local();
        let shown = t.display(Zone::Local);
        match local {
            Some(local) => {
                assert_eq!(shown.full, local.format("%Y-%m-%d %H:%M:%S").to_string());
                assert_eq!(
                    local.timestamp(),
                    utc.timestamp(),
                    "local rendering must describe the same instant"
                );
                assert!(!shown.utc_fallback);
            }
            None => {
                assert!(shown.full.ends_with("UTC"));
                assert!(shown.utc_fallback);
            }
        }
    }

    #[test]
    fn zone_label_is_readable() {
        assert_eq!(zone_label(Zone::Utc, UtcMillis::EPOCH).0, "UTC");
        assert_eq!(format_utc_offset(28_800), "UTC+08:00");
        assert_eq!(format_utc_offset(-9_000), "UTC-02:30");
        assert_eq!(format_utc_offset(0), "UTC+00:00");
    }
}
