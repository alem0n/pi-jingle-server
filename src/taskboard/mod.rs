//! Agent task board: a Tauri window that lists finished agent tasks.
//!
//! The module is self-contained and has no dependency on the HTTP receiver:
//! the caller parses nothing and stores nothing, it only pushes events in and
//! asks for a render snapshot.
//!
//! ## Example
//!
//! ```
//! use pi_jingle_server::taskboard::{AgentEvent, RenderCtx, TaskBoard};
//!
//! let board = TaskBoard::new();
//! let json = r#"{
//!   "session_data": {
//!     "session_id": "7f3c9a2e-1b4d-4c8a-9e2f-3a1b2c3d4e5f",
//!     "persisted": true,
//!     "created_at": "2025-09-12T10:30:00.000Z",
//!     "version": 3,
//!     "cwd": "/home/user/project",
//!     "session_name": "Refactor auth module"
//!   },
//!   "event": "agent_settled",
//!   "timestamp": 1757626496789
//! }"#;
//!
//! board.on_event_json(json).unwrap();
//! let snapshot = board.update(RenderCtx::local_now());
//! assert_eq!(snapshot.card_count, 1);
//! assert_eq!(snapshot.groups[0].cards[0].title, "Refactor auth module");
//! ```
//!
//! ## Data
//!
//! One map holds everything: `HashMap<session key, Group>`. A group holds its
//! metadata, its start anchor and `Vec<Card>`, newest card first. No raw JSON,
//! no file, no cache outside this map.
//!
//! ## Structure
//!
//! * [`event`] — input types and lenient JSON parsing.
//! * [`model`] — map, cards, start-time resolution, render DTOs.
//! * [`time`] — UTC storage, RFC 3339 parsing, local display.
//! * [`snap`] — screen-edge snap and collapse geometry (pure math).
//! * [`window`] — Tauri window, drag, snap and IPC commands (`desktop` feature).

pub mod event;
pub mod model;
pub mod snap;
pub mod time;
#[cfg(feature = "desktop")]
pub mod window;

pub use event::{AgentEvent, EventError, EventKind, SessionInput};
pub mod detail;
pub use detail::{place_detail, DetailPlacement, DETAIL_GAP_PX, DETAIL_MARGIN_PX};
pub use model::{
    group_key, resolve_main_field, BoardSnapshot, CardDetail, CardId, CardView, DetailLine,
    FieldSource, GroupView, IngestOutcome, RenderCtx, SecondaryView, SessionMeta, StartSource,
    PLACEHOLDER, RESERVED_SESSION_KEY,
};
pub use snap::{
    default_window_rect, Rect, ScreenRect, SnapController, SnapPhase, DEFAULT_WINDOW_HEIGHT,
    DEFAULT_WINDOW_WIDTH, EDGE_MARGIN_PX, INITIAL_TOP_INSET_PX, MIN_WINDOW_HEIGHT_PX,
    MIN_WINDOW_WIDTH_PX, PARKED_BAND_PX, SNAP_TOP_THRESHOLD_PX,
};
pub use time::{format_duration, UtcMillis, Zone};

use std::sync::{Mutex, MutexGuard};

/// The task board. Cheap to share: all access goes through one mutex.
///
/// The type is `Send + Sync`, so the HTTP receiver, a background thread and
/// the window commands can all use the same instance.
#[derive(Debug)]
pub struct TaskBoard {
    inner: Mutex<model::Board>,
}

impl Default for TaskBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskBoard {
    /// Create an empty board.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(model::Board::new()),
        }
    }

    /// Push one parsed event. The UTC time of this call becomes the session
    /// start anchor for the next card (section 5 of the specification).
    pub fn on_event(&self, event: AgentEvent) -> IngestOutcome {
        self.on_event_at(event, UtcMillis::now())
    }

    /// Push one parsed event with an explicit "now". Use this in tests and
    /// when the caller has its own clock.
    pub fn on_event_at(&self, event: AgentEvent, now: UtcMillis) -> IngestOutcome {
        self.board().ingest(event, now)
    }

    /// Parse and push one raw JSON body. A malformed body returns an error and
    /// leaves the board unchanged.
    pub fn on_event_json(&self, text: &str) -> Result<IngestOutcome, EventError> {
        self.on_event_json_bytes(text.as_bytes())
    }

    /// Parse and push one raw JSON body from bytes.
    pub fn on_event_json_bytes(&self, bytes: &[u8]) -> Result<IngestOutcome, EventError> {
        let event = AgentEvent::from_json_bytes(bytes)?;
        Ok(self.on_event(event))
    }

    /// Render pass. Converts every stored UTC time for display and returns the
    /// JSON-ready snapshot for the window. The board itself is not modified.
    pub fn update(&self, ctx: RenderCtx) -> BoardSnapshot {
        self.board().snapshot(ctx)
    }

    /// Remove one card by id. Returns `true` when a card was removed.
    pub fn delete_card(&self, card_id: CardId) -> bool {
        self.board().delete_card(card_id)
    }

    /// Complete information about one card, for the detail popover.
    ///
    /// Returns `None` when no card has this id. The board is not modified.
    pub fn card_detail(&self, card_id: CardId, ctx: RenderCtx) -> Option<CardDetail> {
        self.board().card_detail(card_id, ctx)
    }

    /// Change counter. It increases on every insert and delete.
    pub fn revision(&self) -> u64 {
        self.board().revision()
    }

    /// Number of cards in memory.
    pub fn card_count(&self) -> usize {
        self.board().card_count()
    }

    /// Number of sessions that hold at least one card.
    pub fn session_count(&self) -> usize {
        self.board().session_count()
    }

    /// Lock the model. A poisoned lock is recovered; a panic in one caller
    /// must not disable the window.
    fn board(&self) -> MutexGuard<'_, model::Board> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"session_data":{"session_id":"7f3c9a2e-1b4d-4c8a-9e2f-3a1b2c3d4e5f","persisted":true,"created_at":"2025-09-12T10:30:00.000Z","version":3,"cwd":"/home/user/project","session_name":"Refactor auth module"},"event":"agent_settled","timestamp":1757626496789}"#;

    #[test]
    fn json_entry_point_inserts_a_card() {
        let board = TaskBoard::new();
        let outcome = board.on_event_json(SAMPLE).unwrap();
        assert!(matches!(
            outcome,
            IngestOutcome::CardInserted { card_id: 1, .. }
        ));
        assert_eq!(board.card_count(), 1);

        let snapshot = board.update(RenderCtx::at_utc(UtcMillis::from_millis(1_757_626_500_000)));
        let card = &snapshot.groups[0].cards[0];
        assert_eq!(card.title, "Refactor auth module");
        assert_eq!(card.start_source, Some(StartSource::CreatedAt));
        assert_eq!(card.start_utc_ms, Some(1_757_673_000_000));
        assert_eq!(card.request_utc_ms, Some(1_757_626_496_789));
        assert_eq!(card.duration_ms, 0);
        assert!(card.duration_anomaly);
    }

    #[test]
    fn malformed_json_does_not_change_the_board() {
        let board = TaskBoard::new();
        assert!(board.on_event_json("{").is_err());
        assert_eq!(board.card_count(), 0);
        assert_eq!(board.revision(), 0);
    }

    #[test]
    fn three_settled_events_keep_three_cards_in_one_group() {
        let board = TaskBoard::new();
        for ts in [1_757_626_496_000_i64, 1_757_626_497_000, 1_757_626_498_000] {
            let json = format!(
                r#"{{"session_data":{{"session_id":"s1","session_name":"Same","cwd":"/w"}},"event":"agent_settled","timestamp":{ts}}}"#
            );
            board.on_event_json(&json).unwrap();
        }
        let snapshot = board.update(RenderCtx::at_utc(UtcMillis::from_millis(1_757_626_500_000)));
        assert_eq!(snapshot.session_count, 1);
        assert_eq!(snapshot.card_count, 3);
        assert_eq!(snapshot.groups[0].cards.len(), 3);
        assert_eq!(
            snapshot.groups[0].cards[0].request_utc_ms,
            Some(1_757_626_498_000)
        );
        assert_eq!(
            snapshot.groups[0].cards[2].request_utc_ms,
            Some(1_757_626_496_000)
        );
    }

    #[test]
    fn delete_works_through_the_public_api() {
        let board = TaskBoard::new();
        let one =
            r#"{"session_data":{"session_id":"s1"},"event":"agent_settled","timestamp":1000}"#;
        board.on_event_json(one).unwrap();
        board.on_event_json(one).unwrap();
        assert_eq!(board.card_count(), 2);
        assert!(board.delete_card(1));
        assert_eq!(board.card_count(), 1);
        assert!(!board.delete_card(1));
    }
}
