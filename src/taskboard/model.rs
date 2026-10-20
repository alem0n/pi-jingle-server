//! In-memory model of the task board.
//!
//! Storage is one map: `HashMap<session key, Group>`. A group holds the
//! session metadata, the current start anchor and a `Vec<Card>` where index 0
//! is the most recent card. Nothing is written to disk and no raw message is
//! kept.
//!
//! All time fields are [`UtcMillis`]. Rendering creates the text form; the
//! stored value never changes when the clock or the time zone changes.

use serde::Serialize;
use std::collections::HashMap;

use super::event::{AgentEvent, EventKind, SessionInput};
use super::time::{format_duration, zone_label, UtcMillis, Zone};

/// Text shown when a value is missing or not usable.
pub const PLACEHOLDER: &str = "—";

/// Group key for cards whose `session_id` is missing or blank.
pub const RESERVED_SESSION_KEY: &str = "unknown";

/// Stable identifier of one card. Card ids start at 1 and never repeat.
pub type CardId = u64;

/// Which session field supplied the card title.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldSource {
    /// `session_data.session_name`.
    SessionName,
    /// `session_data.cwd`.
    Cwd,
    /// `session_data.session_id`.
    SessionId,
    /// No usable field; the title is [`PLACEHOLDER`].
    None,
}

impl FieldSource {
    /// Short label for the UI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionName => "session_name",
            Self::Cwd => "cwd",
            Self::SessionId => "session_id",
            Self::None => "none",
        }
    }
}

/// Where a card's start time came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartSource {
    /// Latest `agent_start` event timestamp.
    AgentStart,
    /// Latest `session_start` event timestamp.
    SessionStart,
    /// `session_data.created_at`, parsed from RFC 3339.
    CreatedAt,
    /// UTC time recorded when the previous card of this session was inserted.
    Recorded,
    /// The `agent_settled` timestamp itself; used when no earlier time exists,
    /// so the duration is 0.
    RequestTimestamp,
}

impl StartSource {
    /// Short label for the UI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentStart => "agent_start",
            Self::SessionStart => "session_start",
            Self::CreatedAt => "created_at",
            Self::Recorded => "recorded",
            Self::RequestTimestamp => "request_timestamp",
        }
    }
}

/// One label and value row of the card detail popover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DetailLine {
    /// Row label, for example `Start (UTC)`.
    pub label: String,
    /// Value text, already formatted for display.
    pub value: String,
    /// `true` when the value needs attention, for example a clamped duration.
    pub warn: bool,
}

/// Complete information about one card, for the detail popover.
///
/// The popover gets this on demand: the render snapshot stays small, and the
/// card table remains the only stored data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CardDetail {
    /// Card id.
    pub id: CardId,
    /// Single main display value.
    pub title: String,
    /// Group heading of the session.
    pub session: String,
    /// Label and value rows, in display order.
    pub lines: Vec<DetailLine>,
}

impl DetailLine {
    /// A normal row.
    fn new(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.to_string(),
            value: value.into(),
            warn: false,
        }
    }

    /// A row that needs attention.
    fn warn(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.to_string(),
            value: value.into(),
            warn: true,
        }
    }
}

/// One resolved start time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartInstant {
    /// UTC instant.
    pub at: UtcMillis,
    /// Which input supplied the instant.
    pub source: StartSource,
}

/// Per-session metadata, kept for display only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionMeta {
    /// `session_id`, when usable.
    pub session_id: Option<String>,
    /// `session_name`, when usable.
    pub session_name: Option<String>,
    /// `cwd`, when usable.
    pub cwd: Option<String>,
    /// `created_at`, parsed to UTC milliseconds.
    pub created_at: Option<UtcMillis>,
    /// `persisted`, passed through.
    pub persisted: Option<bool>,
    /// `version`, passed through.
    pub version: Option<u64>,
}

/// One agent task card. Created only by an `agent_settled` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Stable id, used by the delete button.
    pub id: CardId,
    /// The single main display value.
    pub title: String,
    /// Which field supplied `title`.
    pub title_source: FieldSource,
    /// Optional second value. It is never equal to `title`.
    pub secondary: Option<(String, FieldSource)>,
    /// Resolved start time.
    pub start: Option<StartInstant>,
    /// Time of the `agent_settled` event.
    pub request: Option<UtcMillis>,
    /// `request - start` in milliseconds, clamped at 0.
    pub duration_ms: i64,
    /// `true` when the start time is later than the request time.
    pub duration_anomaly: bool,
}

/// Anchor used to resolve the start time of the next card of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    at: UtcMillis,
    source: StartSource,
}

/// One session and its cards. `cards[0]` is the newest card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Session metadata, merged from the events seen so far.
    pub meta: SessionMeta,
    /// First-seen sequence number; it fixes the group order in the list.
    pub order: u64,
    /// Start anchor for the next card.
    anchor: Option<Anchor>,
    /// Cards, newest first.
    pub cards: Vec<Card>,
}

/// What one ingest call did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestOutcome {
    /// A card was inserted at index 0 of its session.
    CardInserted {
        /// New card id.
        card_id: CardId,
        /// Group key that received the card.
        session_key: String,
        /// Which source supplied the card start time, if any.
        start_source: Option<StartSource>,
        /// Computed duration in milliseconds.
        duration_ms: i64,
        /// `true` when a seconds timestamp was converted to milliseconds.
        seconds_corrected: bool,
    },
    /// A start event updated the session anchor.
    StartRecorded {
        /// Group key that was updated.
        session_key: String,
        /// Event kind that set the anchor.
        source: StartSource,
        /// New anchor value, `None` when the event had no usable timestamp.
        at: Option<UtcMillis>,
        /// `true` when a seconds timestamp was converted to milliseconds.
        seconds_corrected: bool,
    },
    /// An event name the board does not act on.
    IgnoredUnknownEvent {
        /// The raw event name.
        raw: String,
    },
}

/// The board state. Use [`super::TaskBoard`] from outside this module.
#[derive(Debug)]
pub struct Board {
    groups: HashMap<String, Group>,
    next_card_id: CardId,
    next_order: u64,
    revision: u64,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            groups: HashMap::new(),
            next_card_id: 1,
            next_order: 0,
            revision: 0,
        }
    }
}

impl Board {
    /// Create an empty board.
    pub fn new() -> Self {
        Self::default()
    }

    /// Change counter. It increases on every insert and delete.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Total number of cards.
    pub fn card_count(&self) -> usize {
        self.groups.values().map(|g| g.cards.len()).sum()
    }

    /// Number of groups that hold at least one card.
    pub fn session_count(&self) -> usize {
        self.groups.values().filter(|g| !g.cards.is_empty()).count()
    }

    /// All groups in first-seen order, including groups that have no card.
    pub fn groups(&self) -> impl Iterator<Item = (&String, &Group)> {
        self.groups.iter()
    }

    /// Apply one event. `now` is used for the start anchor recorded after an
    /// insert (see section 5 of the specification).
    pub fn ingest(&mut self, event: AgentEvent, now: UtcMillis) -> IngestOutcome {
        match &event.kind {
            EventKind::Other(raw) => {
                if !raw.is_empty() {
                    eprintln!("[taskboard] ignoring unknown event: {raw}");
                }
                IngestOutcome::IgnoredUnknownEvent { raw: raw.clone() }
            }
            EventKind::SessionStart | EventKind::AgentStart => {
                let source = if event.kind == EventKind::AgentStart {
                    StartSource::AgentStart
                } else {
                    StartSource::SessionStart
                };
                let key = group_key(&event.session);
                let at = event.timestamp.and_then(UtcMillis::from_epoch_field);
                let group = self.group_entry(key.clone(), &event.session);
                if let Some((anchor, _)) = at {
                    group.anchor = Some(Anchor { at: anchor, source });
                }
                IngestOutcome::StartRecorded {
                    session_key: key,
                    source,
                    at: at.map(|(at, _)| at),
                    seconds_corrected: at.is_some_and(|(_, corrected)| corrected),
                }
            }
            EventKind::AgentSettled => self.insert_card(event, now),
        }
    }

    /// Insert one card at index 0 of its session.
    pub fn insert_card(&mut self, event: AgentEvent, now: UtcMillis) -> IngestOutcome {
        let (request, seconds_corrected) =
            match event.timestamp.and_then(UtcMillis::from_epoch_field) {
                Some((at, corrected)) => (Some(at), corrected),
                None => (None, false),
            };

        let key = group_key(&event.session);
        let card_id = self.next_card_id;
        self.next_card_id = self.next_card_id.saturating_add(1);

        let group = self.group_entry(key.clone(), &event.session);
        let start = resolve_start(group.anchor, group.meta.created_at, request);
        let duration_ms = match (start, request) {
            (Some(start), Some(request)) => request.saturating_since(start.at),
            _ => 0,
        };
        let duration_anomaly = matches!((start, request), (Some(s), Some(r)) if r.is_before(s.at));

        // The title comes from this event's session_data, not from the merged
        // session metadata, so a later event that drops `session_name` falls
        // back to `cwd` as section 3 requires.
        let card_meta = SessionMeta::from_input(&event.session);
        let (title, title_source) = resolve_main_field(&card_meta);
        let secondary = resolve_secondary(&card_meta, title_source, &title);

        group.cards.insert(
            0,
            Card {
                id: card_id,
                title,
                title_source,
                secondary,
                start,
                request,
                duration_ms,
                duration_anomaly,
            },
        );
        // Section 5: the next card of this session measures from this instant.
        group.anchor = Some(Anchor {
            at: now,
            source: StartSource::Recorded,
        });

        self.revision = self.revision.saturating_add(1);

        IngestOutcome::CardInserted {
            card_id,
            session_key: key,
            start_source: start.map(|s| s.source),
            duration_ms,
            seconds_corrected,
        }
    }

    /// Remove exactly one card. An empty group is removed with it.
    pub fn delete_card(&mut self, card_id: CardId) -> bool {
        let key = self.groups.iter().find_map(|(key, group)| {
            if group.cards.iter().any(|card| card.id == card_id) {
                Some(key.clone())
            } else {
                None
            }
        });

        let Some(key) = key else {
            return false;
        };

        if let Some(group) = self.groups.get_mut(&key) {
            group.cards.retain(|card| card.id != card_id);
            if group.cards.is_empty() {
                self.groups.remove(&key);
            }
        }

        self.revision = self.revision.saturating_add(1);
        true
    }

    /// Build the render snapshot.
    pub fn snapshot(&self, ctx: RenderCtx) -> BoardSnapshot {
        let (zone_label, zone_fallback) = zone_label(ctx.zone, ctx.now);
        let now_display = ctx.now.display(ctx.zone);

        let mut groups: Vec<GroupView> = self
            .groups
            .iter()
            .filter(|(_, group)| !group.cards.is_empty())
            .map(|(key, group)| group.view(key, ctx))
            .collect();
        groups.sort_by_key(|group| group.order);

        let card_count = groups.iter().map(|group| group.cards.len()).sum();

        BoardSnapshot {
            revision: self.revision,
            generated_at_utc_ms: ctx.now.as_millis(),
            generated_at_local: now_display.full,
            generated_at_utc: ctx.now.utc_text().unwrap_or_default(),
            zone_label,
            zone_fallback,
            session_count: groups.len(),
            card_count,
            groups,
        }
    }

    /// Complete information about one card, for the detail popover.
    ///
    /// Returns `None` when no card has this id. The values cover the stored
    /// fields, the raw UTC times and the source of each derived field.
    pub fn card_detail(&self, card_id: CardId, ctx: RenderCtx) -> Option<CardDetail> {
        let (key, group) = self
            .groups
            .iter()
            .find(|(_, group)| group.cards.iter().any(|card| card.id == card_id))?;
        let card = group.cards.iter().find(|card| card.id == card_id)?;
        let (heading, _) = resolve_main_field(&group.meta);

        let start_local = card
            .start
            .map(|start| start.at.display(ctx.zone).full)
            .unwrap_or_else(|| PLACEHOLDER.to_string());
        let start_utc = card
            .start
            .and_then(|start| start.at.utc_text())
            .unwrap_or_else(|| PLACEHOLDER.to_string());
        let start_ms = card
            .start
            .map(|start| start.at.as_millis().to_string())
            .unwrap_or_else(|| PLACEHOLDER.to_string());
        let start_source = card
            .start
            .map(|start| start.source.as_str().to_string())
            .unwrap_or_else(|| PLACEHOLDER.to_string());
        let request_local = card
            .request
            .map(|request| request.display(ctx.zone).full)
            .unwrap_or_else(|| PLACEHOLDER.to_string());
        let request_utc = card
            .request
            .and_then(|request| request.utc_text())
            .unwrap_or_else(|| PLACEHOLDER.to_string());
        let request_ms = card
            .request
            .map(|request| request.as_millis().to_string())
            .unwrap_or_else(|| PLACEHOLDER.to_string());

        let mut lines = Vec::new();
        lines.push(DetailLine::new("Title source", card.title_source.as_str()));
        if let Some((value, source)) = card.secondary.as_ref() {
            lines.push(DetailLine::new(
                "Second value",
                format!("{value} ({})", source.as_str()),
            ));
        }
        lines.push(DetailLine::new("Session id", key));
        lines.push(DetailLine::new(
            "Persisted",
            match group.meta.persisted {
                Some(true) => "yes",
                Some(false) => "no",
                None => PLACEHOLDER,
            },
        ));
        lines.push(DetailLine::new(
            "Version",
            group
                .meta
                .version
                .map(|version| version.to_string())
                .unwrap_or_else(|| PLACEHOLDER.to_string()),
        ));
        lines.push(DetailLine::new("Start (local)", start_local));
        lines.push(DetailLine::new("Start (UTC)", start_utc));
        lines.push(DetailLine::new("Start epoch ms", start_ms));
        lines.push(DetailLine::new("Start basis", start_source));
        lines.push(DetailLine::new("Request (local)", request_local));
        lines.push(DetailLine::new("Request (UTC)", request_utc));
        lines.push(DetailLine::new("Request epoch ms", request_ms));
        lines.push(DetailLine::new(
            "Duration",
            format_duration(card.duration_ms),
        ));
        lines.push(DetailLine::new(
            "Duration (ms)",
            card.duration_ms.to_string(),
        ));
        if card.duration_anomaly {
            lines.push(DetailLine::warn(
                "Anomaly",
                "Start time is after the request time. The duration is clamped to 0.",
            ));
        }
        lines.push(DetailLine::new("Card id", card.id.to_string()));

        Some(CardDetail {
            id: card.id,
            title: card.title.clone(),
            session: heading,
            lines,
        })
    }

    /// Find or create the group for a session and merge its metadata.
    fn group_entry(&mut self, key: String, input: &SessionInput) -> &mut Group {
        let order = self.next_order;
        self.next_order = self.next_order.saturating_add(1);

        let group = self.groups.entry(key).or_insert_with(|| Group {
            meta: SessionMeta::default(),
            order,
            anchor: None,
            cards: Vec::new(),
        });
        group.merge_meta(input);
        group
    }
}

impl SessionMeta {
    /// Build the metadata carried by one event.
    pub fn from_input(input: &SessionInput) -> Self {
        Self {
            session_id: input.session_id_value().map(str::to_string),
            session_name: input.session_name_value().map(str::to_string),
            cwd: input.cwd_value().map(str::to_string),
            created_at: input
                .created_at
                .as_deref()
                .and_then(UtcMillis::from_rfc3339),
            persisted: input.persisted,
            version: input.version,
        }
    }
}

impl Group {
    /// Merge non-empty fields of one event into the stored metadata.
    fn merge_meta(&mut self, input: &SessionInput) {
        if let Some(session_id) = input.session_id_value() {
            self.meta.session_id = Some(session_id.to_string());
        }
        if let Some(session_name) = input.session_name_value() {
            self.meta.session_name = Some(session_name.to_string());
        }
        if let Some(cwd) = input.cwd_value() {
            self.meta.cwd = Some(cwd.to_string());
        }
        if let Some(persisted) = input.persisted {
            self.meta.persisted = Some(persisted);
        }
        if let Some(version) = input.version {
            self.meta.version = Some(version);
        }
        if let Some(raw) = input.created_at.as_deref() {
            match UtcMillis::from_rfc3339(raw) {
                Some(at) => self.meta.created_at = Some(at),
                None => {
                    if !raw.trim().is_empty() {
                        eprintln!("[taskboard] created_at is not RFC 3339: {raw:?}");
                    }
                }
            }
        }
    }

    /// Build the render view of this group.
    fn view(&self, key: &str, ctx: RenderCtx) -> GroupView {
        let (heading, heading_source) = resolve_main_field(&self.meta);
        GroupView {
            order: self.order,
            session_key: key.to_string(),
            is_ungrouped: key == RESERVED_SESSION_KEY,
            heading,
            heading_source,
            session_id: self.meta.session_id.clone(),
            persisted: self.meta.persisted,
            version: self.meta.version,
            card_count: self.cards.len(),
            cards: self.cards.iter().map(|card| card.view(ctx)).collect(),
        }
    }
}

impl Card {
    /// Build the render view of this card, converting UTC times to the display
    /// time zone.
    fn view(&self, ctx: RenderCtx) -> CardView {
        let start = self.start.map(|start| start.at.display(ctx.zone));
        let request = self.request.map(|request| request.display(ctx.zone));

        CardView {
            id: self.id,
            title: self.title.clone(),
            title_source: self.title_source,
            secondary: self
                .secondary
                .as_ref()
                .map(|(value, source)| SecondaryView {
                    value: value.clone(),
                    source: *source,
                }),
            start_clock: start.as_ref().map(|text| text.clock.clone()),
            start_local: start.as_ref().map(|text| text.full.clone()),
            start_utc: self.start.and_then(|start| start.at.utc_text()),
            start_utc_ms: self.start.map(|start| start.at.as_millis()),
            start_source: self.start.map(|start| start.source),
            request_clock: request.as_ref().map(|text| text.clock.clone()),
            request_local: request.as_ref().map(|text| text.full.clone()),
            request_utc: self.request.and_then(|request| request.utc_text()),
            request_utc_ms: self.request.map(|request| request.as_millis()),
            duration_label: format_duration(self.duration_ms),
            duration_ms: self.duration_ms,
            duration_anomaly: self.duration_anomaly,
            time_fallback_utc: start.as_ref().is_some_and(|text| text.utc_fallback)
                || request.as_ref().is_some_and(|text| text.utc_fallback),
        }
    }
}

/// Group key of a session: trimmed `session_id`, or [`RESERVED_SESSION_KEY`].
pub fn group_key(session: &SessionInput) -> String {
    match session.session_id_value() {
        Some(id) => id.to_string(),
        None => RESERVED_SESSION_KEY.to_string(),
    }
}

/// Main display field: `session_name` → `cwd` → `session_id` → `—`.
///
/// A candidate is usable when it is present and not blank after trimming.
/// Only one value is returned.
pub fn resolve_main_field(meta: &SessionMeta) -> (String, FieldSource) {
    let candidates: [(Option<&String>, FieldSource); 3] = [
        (meta.session_name.as_ref(), FieldSource::SessionName),
        (meta.cwd.as_ref(), FieldSource::Cwd),
        (meta.session_id.as_ref(), FieldSource::SessionId),
    ];

    for (value, source) in candidates {
        if let Some(text) = value
            .map(|text| text.trim())
            .filter(|text| !text.is_empty())
        {
            return (text.to_string(), source);
        }
    }

    (PLACEHOLDER.to_string(), FieldSource::None)
}

/// Optional second value. It must not repeat the title.
fn resolve_secondary(
    meta: &SessionMeta,
    title_source: FieldSource,
    title: &str,
) -> Option<(String, FieldSource)> {
    let candidate = match title_source {
        FieldSource::SessionName => meta
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| (text.to_string(), FieldSource::Cwd)),
        _ => None,
    };

    candidate.filter(|(value, _)| value != title)
}

/// Resolve a card start time.
///
/// Ladder: session anchor (`agent_start`, `session_start`, or the recorded
/// insert time) → `created_at` → the settled timestamp itself.
fn resolve_start(
    anchor: Option<Anchor>,
    created_at: Option<UtcMillis>,
    request: Option<UtcMillis>,
) -> Option<StartInstant> {
    if let Some(anchor) = anchor {
        return Some(StartInstant {
            at: anchor.at,
            source: anchor.source,
        });
    }
    if let Some(at) = created_at {
        return Some(StartInstant {
            at,
            source: StartSource::CreatedAt,
        });
    }
    request.map(|at| StartInstant {
        at,
        source: StartSource::RequestTimestamp,
    })
}

/// Parameters of one render pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderCtx {
    /// Current UTC time, used for the header clock.
    pub now: UtcMillis,
    /// Zone used for every displayed time.
    pub zone: Zone,
}

impl RenderCtx {
    /// Render with local time and the current system clock.
    pub fn local_now() -> Self {
        Self {
            now: UtcMillis::now(),
            zone: Zone::Local,
        }
    }

    /// Render at a fixed time, in local time.
    pub fn at(now: UtcMillis) -> Self {
        Self {
            now,
            zone: Zone::Local,
        }
    }

    /// Render at a fixed time in UTC, for tests and diagnostics.
    pub fn at_utc(now: UtcMillis) -> Self {
        Self {
            now,
            zone: Zone::Utc,
        }
    }
}

/// One render pass of the whole board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BoardSnapshot {
    /// Change counter of the board. The UI redraws the list only when it changes.
    pub revision: u64,
    /// UTC epoch milliseconds of this render pass.
    pub generated_at_utc_ms: i64,
    /// Local text of this render pass.
    pub generated_at_local: String,
    /// UTC text of this render pass.
    pub generated_at_utc: String,
    /// Zone label, for example `UTC+08:00`.
    pub zone_label: String,
    /// `true` when local conversion failed and UTC text is shown.
    pub zone_fallback: bool,
    /// Number of groups with cards.
    pub session_count: usize,
    /// Number of cards.
    pub card_count: usize,
    /// Groups, in first-seen order.
    pub groups: Vec<GroupView>,
}

/// One session group in a render pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GroupView {
    /// First-seen order.
    pub order: u64,
    /// Map key of the group.
    pub session_key: String,
    /// `true` for [`RESERVED_SESSION_KEY`].
    pub is_ungrouped: bool,
    /// Main display value of the session, same ladder as a card title.
    pub heading: String,
    /// Source of `heading`.
    pub heading_source: FieldSource,
    /// Raw `session_id`, when usable.
    pub session_id: Option<String>,
    /// `persisted`, when known.
    pub persisted: Option<bool>,
    /// `version`, when known.
    pub version: Option<u64>,
    /// Number of cards in this group.
    pub card_count: usize,
    /// Cards, newest first.
    pub cards: Vec<CardView>,
}

/// One card in a render pass. All times are display text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CardView {
    /// Card id, used by the delete command.
    pub id: CardId,
    /// Single main display value.
    pub title: String,
    /// Source of `title`.
    pub title_source: FieldSource,
    /// Optional second value; never equal to `title`.
    pub secondary: Option<SecondaryView>,
    /// Start time, `HH:MM:SS` local.
    pub start_clock: Option<String>,
    /// Start time, full local text.
    pub start_local: Option<String>,
    /// Start time, full UTC text, for tooltips.
    pub start_utc: Option<String>,
    /// Start time, UTC epoch milliseconds.
    pub start_utc_ms: Option<i64>,
    /// Source of the start time.
    pub start_source: Option<StartSource>,
    /// Request time, `HH:MM:SS` local.
    pub request_clock: Option<String>,
    /// Request time, full local text.
    pub request_local: Option<String>,
    /// Request time, full UTC text, for tooltips.
    pub request_utc: Option<String>,
    /// Request time, UTC epoch milliseconds.
    pub request_utc_ms: Option<i64>,
    /// Duration text, for example `1h 02m 05s`.
    pub duration_label: String,
    /// Duration in milliseconds.
    pub duration_ms: i64,
    /// `true` when the start time is after the request time.
    pub duration_anomaly: bool,
    /// `true` when a time fell back to UTC text.
    pub time_fallback_utc: bool,
}

/// Optional second value of a card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecondaryView {
    /// The value.
    pub value: String,
    /// Which field supplied the value.
    pub source: FieldSource,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taskboard::event::EventKind;

    fn session(
        id: Option<&str>,
        name: Option<&str>,
        cwd: Option<&str>,
        created_at: Option<&str>,
    ) -> SessionInput {
        SessionInput {
            session_id: id.map(str::to_string),
            created_at: created_at.map(str::to_string),
            cwd: cwd.map(str::to_string),
            session_name: name.map(str::to_string),
            persisted: None,
            version: None,
        }
    }

    fn event(kind: &str, timestamp: i64, session: SessionInput) -> AgentEvent {
        AgentEvent {
            kind: EventKind::from_raw(kind),
            timestamp: Some(timestamp),
            session,
        }
    }

    fn ctx(now: i64) -> RenderCtx {
        RenderCtx::at_utc(UtcMillis::from_millis(now))
    }

    #[test]
    fn settled_event_inserts_one_card() {
        let mut board = Board::new();
        let outcome = board.ingest(
            event(
                "agent_settled",
                1_000,
                session(Some("s1"), Some("Task"), None, None),
            ),
            UtcMillis::from_millis(1_000),
        );
        assert!(matches!(
            outcome,
            IngestOutcome::CardInserted { card_id: 1, .. }
        ));
        assert_eq!(board.card_count(), 1);
        assert_eq!(board.session_count(), 1);

        let snapshot = board.snapshot(ctx(2_000));
        assert_eq!(snapshot.groups.len(), 1);
        assert_eq!(snapshot.groups[0].cards.len(), 1);
        assert_eq!(snapshot.groups[0].cards[0].title, "Task");
        assert_eq!(
            snapshot.groups[0].cards[0].title_source,
            FieldSource::SessionName
        );
    }

    #[test]
    fn start_and_session_events_do_not_insert_cards() {
        let mut board = Board::new();
        board.ingest(
            event("session_start", 500, session(Some("s1"), None, None, None)),
            UtcMillis::from_millis(500),
        );
        board.ingest(
            event("agent_start", 600, session(Some("s1"), None, None, None)),
            UtcMillis::from_millis(600),
        );
        assert_eq!(board.card_count(), 0);
        assert_eq!(board.session_count(), 0);
        // A group without cards is not rendered.
        assert!(board.snapshot(ctx(700)).groups.is_empty());
    }

    #[test]
    fn three_settled_events_make_three_cards_newest_first() {
        let mut board = Board::new();
        for ts in [1_000, 2_000, 3_000] {
            board.ingest(
                event(
                    "agent_settled",
                    ts,
                    session(Some("s1"), Some("Same"), None, None),
                ),
                UtcMillis::from_millis(ts),
            );
        }
        let snapshot = board.snapshot(ctx(4_000));
        let group = &snapshot.groups[0];
        assert_eq!(group.card_count, 3);
        let ids: Vec<CardId> = group.cards.iter().map(|card| card.id).collect();
        assert_eq!(ids, vec![3, 2, 1]);
        let requests: Vec<i64> = group
            .cards
            .iter()
            .map(|card| card.request_utc_ms.unwrap())
            .collect();
        assert_eq!(requests, vec![3_000, 2_000, 1_000]);
        // Equal titles must not be merged.
        assert!(group.cards.iter().all(|card| card.title == "Same"));
    }

    #[test]
    fn sessions_are_isolated_and_ordered_by_first_seen() {
        let mut board = Board::new();
        board.ingest(
            event(
                "agent_settled",
                1_000,
                session(Some("a"), Some("A"), None, None),
            ),
            UtcMillis::from_millis(1_000),
        );
        board.ingest(
            event(
                "agent_settled",
                2_000,
                session(Some("b"), Some("B"), None, None),
            ),
            UtcMillis::from_millis(2_000),
        );
        board.ingest(
            event(
                "agent_settled",
                3_000,
                session(Some("a"), Some("A"), None, None),
            ),
            UtcMillis::from_millis(3_000),
        );

        let snapshot = board.snapshot(ctx(4_000));
        assert_eq!(snapshot.groups.len(), 2);
        assert_eq!(snapshot.groups[0].session_key, "a");
        assert_eq!(snapshot.groups[1].session_key, "b");
        assert_eq!(snapshot.groups[0].cards.len(), 2);
        assert_eq!(snapshot.groups[1].cards.len(), 1);
        assert_eq!(snapshot.groups[0].cards[0].request_utc_ms, Some(3_000));
    }

    #[test]
    fn duplicate_events_insert_another_card() {
        let mut board = Board::new();
        let duplicate = event(
            "agent_settled",
            5_000,
            session(Some("s1"), Some("T"), None, None),
        );
        board.ingest(duplicate.clone(), UtcMillis::from_millis(5_000));
        board.ingest(duplicate, UtcMillis::from_millis(5_000));
        assert_eq!(board.card_count(), 2);
    }

    #[test]
    fn title_priority_shows_exactly_one_value() {
        let mut board = Board::new();

        // Each event carries its own session_data; the card uses that data.
        board.ingest(
            event(
                "agent_settled",
                1_000,
                session(Some("sid"), Some("Name"), Some("/cwd"), None),
            ),
            UtcMillis::from_millis(1_000),
        );
        // Whitespace-only name must not win.
        board.ingest(
            event(
                "agent_settled",
                2_000,
                session(Some("sid"), Some("   "), Some("/cwd"), None),
            ),
            UtcMillis::from_millis(2_000),
        );
        board.ingest(
            event(
                "agent_settled",
                3_000,
                session(Some("sid"), None, Some(""), None),
            ),
            UtcMillis::from_millis(3_000),
        );
        board.ingest(
            event(
                "agent_settled",
                4_000,
                session(Some("sid"), None, None, None),
            ),
            UtcMillis::from_millis(4_000),
        );

        let snapshot = board.snapshot(ctx(6_000));
        assert_eq!(snapshot.groups.len(), 1);
        let group = &snapshot.groups[0];
        let titles: Vec<(&str, FieldSource)> = group
            .cards
            .iter()
            .map(|card| (card.title.as_str(), card.title_source))
            .collect();
        assert_eq!(
            titles,
            vec![
                ("sid", FieldSource::SessionId),
                ("sid", FieldSource::SessionId),
                ("/cwd", FieldSource::Cwd),
                ("Name", FieldSource::SessionName),
            ]
        );

        // The top card has a secondary cwd value; the title is not repeated.
        let bottom = &group.cards[3];
        assert_eq!(bottom.title, "Name");
        let secondary = bottom.secondary.as_ref().unwrap();
        assert_eq!(secondary.value, "/cwd");
        assert_ne!(secondary.value, bottom.title);
        assert_eq!(group.heading, "Name");
    }

    #[test]
    fn a_card_without_any_usable_field_shows_a_placeholder() {
        let mut board = Board::new();
        board.ingest(
            event("agent_settled", 1_000, session(None, None, None, None)),
            UtcMillis::from_millis(1_000),
        );
        let card = &board.snapshot(ctx(2_000)).groups[0].cards[0];
        assert_eq!(card.title, PLACEHOLDER);
        assert_eq!(card.title_source, FieldSource::None);
    }

    #[test]
    fn title_falls_back_to_cwd_when_name_is_missing() {
        let mut board = Board::new();
        board.ingest(
            event(
                "agent_settled",
                1_000,
                session(Some("sid"), None, Some("/home/user/project"), None),
            ),
            UtcMillis::from_millis(1_000),
        );
        let card = &board.snapshot(ctx(2_000)).groups[0].cards[0];
        assert_eq!(card.title, "/home/user/project");
        assert_eq!(card.title_source, FieldSource::Cwd);
        assert_eq!(card.secondary, None);
    }

    #[test]
    fn blank_session_id_uses_the_reserved_group() {
        let mut board = Board::new();
        board.ingest(
            event("agent_settled", 1_000, session(None, None, None, None)),
            UtcMillis::from_millis(1_000),
        );
        board.ingest(
            event(
                "agent_settled",
                2_000,
                session(Some("   "), None, None, None),
            ),
            UtcMillis::from_millis(2_000),
        );
        let snapshot = board.snapshot(ctx(3_000));
        assert_eq!(snapshot.groups.len(), 1);
        let group = &snapshot.groups[0];
        assert_eq!(group.session_key, RESERVED_SESSION_KEY);
        assert!(group.is_ungrouped);
        assert_eq!(group.cards.len(), 2);
        assert_eq!(group.cards[0].title, PLACEHOLDER);
        assert_eq!(group.cards[0].title_source, FieldSource::None);
    }

    #[test]
    fn start_priority_is_agent_start_then_session_start_then_created_at() {
        let created = "2025-09-12T10:30:00.000Z";

        // 1. agent_start wins over everything.
        let mut board = Board::new();
        board.ingest(
            event(
                "session_start",
                1_000,
                session(Some("s1"), None, None, Some(created)),
            ),
            UtcMillis::from_millis(1_000),
        );
        board.ingest(
            event("agent_start", 2_000, session(Some("s1"), None, None, None)),
            UtcMillis::from_millis(2_000),
        );
        board.ingest(
            event(
                "agent_settled",
                5_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(5_000),
        );
        let card = &board.snapshot(ctx(6_000)).groups[0].cards[0];
        assert_eq!(card.start_utc_ms, Some(2_000));
        assert_eq!(card.start_source, Some(StartSource::AgentStart));
        assert_eq!(card.duration_ms, 3_000);

        // 2. session_start wins when there is no agent_start.
        let mut board = Board::new();
        board.ingest(
            event(
                "session_start",
                1_000,
                session(Some("s1"), None, None, Some(created)),
            ),
            UtcMillis::from_millis(1_000),
        );
        board.ingest(
            event(
                "agent_settled",
                5_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(5_000),
        );
        let card = &board.snapshot(ctx(6_000)).groups[0].cards[0];
        assert_eq!(card.start_utc_ms, Some(1_000));
        assert_eq!(card.start_source, Some(StartSource::SessionStart));
        assert_eq!(card.duration_ms, 4_000);

        // 3. created_at is used when no start event was seen.
        let mut board = Board::new();
        board.ingest(
            event(
                "agent_settled",
                5_000,
                session(Some("s1"), None, None, Some(created)),
            ),
            UtcMillis::from_millis(5_000),
        );
        let card = &board.snapshot(ctx(6_000)).groups[0].cards[0];
        assert_eq!(card.start_utc_ms, Some(1_757_673_000_000));
        assert_eq!(card.start_source, Some(StartSource::CreatedAt));
        assert_eq!(card.duration_ms, 0); // start is after the request: clamped
        assert!(card.duration_anomaly);
    }

    #[test]
    fn session_start_updates_later_cards_without_rewriting_earlier_cards() {
        let mut board = Board::new();
        board.ingest(
            event(
                "agent_settled",
                1_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(1_000),
        );
        board.ingest(
            event(
                "session_start",
                10_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(10_000),
        );
        board.ingest(
            event(
                "agent_settled",
                12_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(12_000),
        );

        let cards = &board.snapshot(ctx(13_000)).groups[0].cards;
        // Card 2 uses the session_start anchor.
        assert_eq!(cards[0].start_utc_ms, Some(10_000));
        assert_eq!(cards[0].start_source, Some(StartSource::SessionStart));
        assert_eq!(cards[0].duration_ms, 2_000);
        // Card 1 is unchanged.
        assert_eq!(cards[1].start_utc_ms, Some(1_000));
        assert_eq!(cards[1].start_source, Some(StartSource::RequestTimestamp));
        assert_eq!(cards[1].duration_ms, 0);
    }

    #[test]
    fn insert_records_a_start_anchor_for_the_next_card() {
        let mut board = Board::new();
        board.ingest(
            event(
                "agent_settled",
                1_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(4_000),
        );
        board.ingest(
            event(
                "agent_settled",
                9_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(9_000),
        );
        let cards = &board.snapshot(ctx(10_000)).groups[0].cards;
        assert_eq!(cards[0].start_utc_ms, Some(4_000));
        assert_eq!(cards[0].start_source, Some(StartSource::Recorded));
        assert_eq!(cards[0].duration_ms, 5_000);
    }

    #[test]
    fn an_invalid_created_at_falls_back_to_the_request_timestamp() {
        let mut board = Board::new();
        board.ingest(
            event(
                "agent_settled",
                7_000,
                session(Some("s1"), None, None, Some("bad")),
            ),
            UtcMillis::from_millis(7_000),
        );
        let card = &board.snapshot(ctx(8_000)).groups[0].cards[0];
        assert_eq!(card.start_utc_ms, Some(7_000));
        assert_eq!(card.start_source, Some(StartSource::RequestTimestamp));
        assert_eq!(card.duration_ms, 0);
    }

    #[test]
    fn a_start_after_the_request_is_clamped_to_zero() {
        let mut board = Board::new();
        board.ingest(
            event("agent_start", 5_000, session(Some("s1"), None, None, None)),
            UtcMillis::from_millis(5_000),
        );
        board.ingest(
            event(
                "agent_settled",
                4_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(4_000),
        );
        let card = &board.snapshot(ctx(6_000)).groups[0].cards[0];
        assert_eq!(card.duration_ms, 0);
        assert_eq!(card.duration_label, "0s");
        assert!(card.duration_anomaly);
    }

    #[test]
    fn card_detail_lists_the_complete_card() {
        let mut board = Board::new();
        board.ingest(
            event(
                "agent_settled",
                1_000,
                session(Some("s1"), Some("Name"), Some("/home/user/project"), None),
            ),
            UtcMillis::from_millis(1_000),
        );
        let detail = board.card_detail(1, ctx(2_000)).unwrap();

        assert_eq!(detail.id, 1);
        assert_eq!(detail.title, "Name");
        assert_eq!(detail.session, "Name");

        let get = |label: &str| {
            detail
                .lines
                .iter()
                .find(|line| line.label == label)
                .map(|line| line.value.as_str())
                .unwrap_or_else(|| panic!("no line {label}"))
        };
        assert_eq!(get("Title source"), "session_name");
        assert_eq!(get("Second value"), "/home/user/project (cwd)");
        assert_eq!(get("Session id"), "s1");
        assert_eq!(get("Start epoch ms"), "1000");
        assert_eq!(get("Start basis"), "request_timestamp");
        assert_eq!(get("Request epoch ms"), "1000");
        assert_eq!(get("Duration"), "0s");
        assert_eq!(get("Duration (ms)"), "0");
        assert_eq!(get("Card id"), "1");
        assert!(!detail.lines.iter().any(|line| line.warn));

        // The local and UTC forms of the same instant are both present.
        assert!(detail
            .lines
            .iter()
            .any(|line| line.label == "Start (local)"));
        assert!(detail.lines.iter().any(|line| line.label == "Start (UTC)"));
        assert!(detail
            .lines
            .iter()
            .any(|line| line.label == "Request (local)"));
        assert!(detail
            .lines
            .iter()
            .any(|line| line.label == "Request (UTC)"));
    }

    #[test]
    fn card_detail_marks_a_clamped_duration() {
        let mut board = Board::new();
        board.ingest(
            event("agent_start", 5_000, session(Some("s1"), None, None, None)),
            UtcMillis::from_millis(5_000),
        );
        board.ingest(
            event(
                "agent_settled",
                4_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(4_000),
        );

        let detail = board.card_detail(1, ctx(6_000)).unwrap();
        let anomaly = detail
            .lines
            .iter()
            .find(|line| line.label == "Anomaly")
            .expect("anomaly line");
        assert!(anomaly.warn);
        assert!(anomaly.value.contains("clamped"));
        assert_eq!(
            anomaly.value,
            "Start time is after the request time. The duration is clamped to 0."
        );
    }

    #[test]
    fn card_detail_of_an_unknown_id_is_none() {
        let mut board = Board::new();
        board.ingest(
            event(
                "agent_settled",
                1_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(1_000),
        );
        assert!(board.card_detail(1, ctx(2_000)).is_some());
        assert!(board.card_detail(2, ctx(2_000)).is_none());
        board.delete_card(1);
        assert!(board.card_detail(1, ctx(2_000)).is_none());
    }

    #[test]
    fn a_settled_event_without_a_timestamp_still_inserts_a_card() {
        let mut board = Board::new();
        board.ingest(
            AgentEvent {
                kind: EventKind::AgentSettled,
                timestamp: None,
                session: session(Some("s1"), Some("Name"), None, None),
            },
            UtcMillis::from_millis(1_000),
        );
        let card = &board.snapshot(ctx(2_000)).groups[0].cards[0];
        assert_eq!(card.request_utc_ms, None);
        assert_eq!(card.request_clock, None);
        assert_eq!(card.duration_ms, 0);
        assert_eq!(card.duration_label, "0s");
        assert_eq!(card.start_utc_ms, None);
    }

    #[test]
    fn delete_removes_one_card_and_keeps_the_rest() {
        let mut board = Board::new();
        for ts in [1_000, 2_000, 3_000] {
            board.ingest(
                event(
                    "agent_settled",
                    ts,
                    session(Some("s1"), Some("T"), None, None),
                ),
                UtcMillis::from_millis(ts),
            );
        }
        assert!(board.delete_card(2));
        let snapshot = board.snapshot(ctx(4_000));
        assert_eq!(snapshot.groups[0].cards.len(), 2);
        assert_eq!(
            snapshot.groups[0]
                .cards
                .iter()
                .map(|card| card.id)
                .collect::<Vec<_>>(),
            vec![3, 1]
        );
        assert!(!board.delete_card(99));

        assert!(board.delete_card(3));
        assert!(board.delete_card(1));
        assert_eq!(board.card_count(), 0);
        assert!(board.snapshot(ctx(5_000)).groups.is_empty());
    }

    #[test]
    fn delete_keeps_other_sessions() {
        let mut board = Board::new();
        board.ingest(
            event("agent_settled", 1_000, session(Some("a"), None, None, None)),
            UtcMillis::from_millis(1_000),
        );
        board.ingest(
            event("agent_settled", 2_000, session(Some("b"), None, None, None)),
            UtcMillis::from_millis(2_000),
        );
        assert!(board.delete_card(1));
        let snapshot = board.snapshot(ctx(3_000));
        assert_eq!(snapshot.groups.len(), 1);
        assert_eq!(snapshot.groups[0].session_key, "b");
    }

    #[test]
    fn unknown_events_are_ignored() {
        let mut board = Board::new();
        let outcome = board.ingest(
            event("turn_end", 1_000, session(Some("s1"), None, None, None)),
            UtcMillis::from_millis(1_000),
        );
        assert_eq!(
            outcome,
            IngestOutcome::IgnoredUnknownEvent {
                raw: "turn_end".to_string()
            }
        );
        assert_eq!(board.card_count(), 0);
        assert_eq!(board.revision(), 0);
    }

    #[test]
    fn seconds_timestamps_are_detected() {
        let mut board = Board::new();
        let outcome = board.ingest(
            event(
                "agent_settled",
                1_757_626_496,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(1_757_626_496_000),
        );
        match outcome {
            IngestOutcome::CardInserted {
                seconds_corrected, ..
            } => assert!(seconds_corrected),
            other => panic!("unexpected outcome: {other:?}"),
        }
        let card = &board.snapshot(ctx(1_757_626_500_000)).groups[0].cards[0];
        assert_eq!(card.request_utc_ms, Some(1_757_626_496_000));
    }

    #[test]
    fn local_rendering_matches_utc_storage() {
        let mut board = Board::new();
        board.ingest(
            event(
                "session_start",
                1_757_626_400_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(1_757_626_400_000),
        );
        board.ingest(
            event(
                "agent_settled",
                1_757_626_496_789,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(1_757_626_496_789),
        );

        let utc = board.snapshot(ctx(1_757_626_500_000));
        let local = board.snapshot(RenderCtx {
            now: UtcMillis::from_millis(1_757_626_500_000),
            zone: Zone::Local,
        });

        let utc_card = &utc.groups[0].cards[0];
        let local_card = &local.groups[0].cards[0];
        assert_eq!(utc_card.start_utc_ms, local_card.start_utc_ms);
        assert_eq!(utc_card.request_utc_ms, local_card.request_utc_ms);
        assert_eq!(
            utc_card.start_utc.as_deref(),
            Some("2025-09-11 21:33:20.000 UTC")
        );
        assert_eq!(
            utc_card.request_utc.as_deref(),
            Some("2025-09-11 21:34:56.789 UTC")
        );
        assert!(local_card.start_local.is_some());
        assert_eq!(utc_card.duration_ms, 96_789);
        assert_eq!(utc_card.duration_label, "1m 36s");
        if !local_card.time_fallback_utc {
            // The local text must describe the same instant as the stored UTC value.
            use chrono::{Local, NaiveDateTime, TimeZone};
            let text = local_card.start_local.as_deref().unwrap();
            let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap();
            if let Some(back) = Local.from_local_datetime(&naive).single() {
                assert_eq!(back.timestamp_millis(), local_card.start_utc_ms.unwrap());
            }
        }
    }

    #[test]
    fn revision_changes_only_on_insert_or_delete() {
        let mut board = Board::new();
        assert_eq!(board.revision(), 0);
        board.ingest(
            event("agent_start", 1_000, session(Some("s1"), None, None, None)),
            UtcMillis::from_millis(1_000),
        );
        assert_eq!(board.revision(), 0);
        board.ingest(
            event(
                "agent_settled",
                2_000,
                session(Some("s1"), None, None, None),
            ),
            UtcMillis::from_millis(2_000),
        );
        assert_eq!(board.revision(), 1);
        board.delete_card(1);
        assert_eq!(board.revision(), 2);
    }
}
