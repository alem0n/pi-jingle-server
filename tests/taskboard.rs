//! Acceptance tests for the task board, driven only by the public API.
//!
//! Run with `cargo test --test taskboard`.

use pi_jingle_server::taskboard::{
    AgentEvent, FieldSource, IngestOutcome, RenderCtx, StartSource, TaskBoard, UtcMillis, Zone,
    PLACEHOLDER, RESERVED_SESSION_KEY,
};

const SESSION_ID: &str = "7f3c9a2e-1b4d-4c8a-9e2f-3a1b2c3d4e5f";

/// The example body from the specification.
fn sample_body() -> String {
    format!(
        r#"{{
          "session_data": {{
            "session_id": "{SESSION_ID}",
            "persisted": true,
            "created_at": "2025-09-12T10:30:00.000Z",
            "version": 3,
            "cwd": "/home/user/project",
            "session_name": "Refactor auth module"
          }},
          "event": "agent_settled",
          "timestamp": 1757626496789
        }}"#
    )
}

fn body(event: &str, timestamp: i64, session: &str) -> String {
    format!(r#"{{"session_data":{{{session}}},"event":"{event}","timestamp":{timestamp}}}"#)
}

fn session(session_id: Option<&str>, name: Option<&str>, cwd: Option<&str>) -> String {
    let field = |key: &str, value: Option<&str>| match value {
        Some(text) => format!(r#""{key}":"{text}""#),
        None => format!(r#""{key}":null"#),
    };
    format!(
        "{},{},{}",
        field("session_id", session_id),
        field("session_name", name),
        field("cwd", cwd)
    )
}

fn render_at(ms: i64) -> RenderCtx {
    RenderCtx::at_utc(UtcMillis::from_millis(ms))
}

#[test]
fn the_sample_body_parses_and_creates_one_card() {
    let board = TaskBoard::new();
    let outcome = board.on_event_json(&sample_body()).unwrap();
    assert!(matches!(
        outcome,
        IngestOutcome::CardInserted { card_id: 1, .. }
    ));

    let snapshot = board.update(render_at(1_757_626_500_000));
    assert_eq!(snapshot.session_count, 1);
    assert_eq!(snapshot.card_count, 1);

    let group = &snapshot.groups[0];
    assert_eq!(group.session_key, SESSION_ID);
    assert_eq!(group.version, Some(3));
    assert_eq!(group.persisted, Some(true));

    let card = &group.cards[0];
    assert_eq!(card.title, "Refactor auth module");
    assert_eq!(card.title_source, FieldSource::SessionName);
    assert_eq!(card.secondary.as_ref().unwrap().value, "/home/user/project");
    assert_eq!(card.request_utc_ms, Some(1_757_626_496_789));
    assert_eq!(card.start_utc_ms, Some(1_757_673_000_000));
    assert_eq!(card.start_source, Some(StartSource::CreatedAt));
    // The created_at value is later than the request: the duration is clamped.
    assert_eq!(card.duration_ms, 0);
    assert!(card.duration_anomaly);
}

#[test]
fn timestamp_and_created_at_share_one_internal_utc_representation() {
    let parsed_text = UtcMillis::from_rfc3339("2025-09-12T10:30:00.000Z").unwrap();
    let parsed_ms = UtcMillis::from_epoch_field(1_757_673_000_000).unwrap().0;
    assert_eq!(parsed_text, parsed_ms);
    assert_eq!(parsed_text.as_millis(), 1_757_673_000_000);
    assert_eq!(
        parsed_text.utc_text().as_deref(),
        Some("2025-09-12 10:30:00.000 UTC")
    );
}

#[test]
fn local_display_is_the_same_instant_as_the_stored_utc_value() {
    let board = TaskBoard::new();
    board.on_event_json(&sample_body()).unwrap();

    let utc = board.update(render_at(1_757_626_500_000));
    let local = board.update(RenderCtx {
        now: UtcMillis::from_millis(1_757_626_500_000),
        zone: Zone::Local,
    });

    let utc_card = &utc.groups[0].cards[0];
    let local_card = &local.groups[0].cards[0];
    assert_eq!(utc_card.start_utc_ms, local_card.start_utc_ms);
    assert_eq!(utc_card.request_utc_ms, local_card.request_utc_ms);
    assert_eq!(
        utc_card.start_utc.as_deref(),
        Some("2025-09-12 10:30:00.000 UTC")
    );

    if local_card.time_fallback_utc {
        assert!(local_card.start_local.as_deref().unwrap().ends_with("UTC"));
    } else {
        use chrono::{Local, NaiveDateTime, TimeZone};
        let text = local_card.start_local.as_deref().unwrap();
        let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap();
        if let Some(back) = Local.from_local_datetime(&naive).single() {
            assert_eq!(back.timestamp_millis(), 1_757_673_000_000);
        }
    }
}

#[test]
fn three_settled_events_keep_three_cards_and_the_newest_is_first() {
    let board = TaskBoard::new();
    let session_data = session(Some(SESSION_ID), Some("Same name"), Some("/w"));
    for ts in [1_757_626_496_000_i64, 1_757_626_497_000, 1_757_626_498_000] {
        board
            .on_event_json(&body("agent_settled", ts, &session_data))
            .unwrap();
    }

    let snapshot = board.update(render_at(1_757_626_500_000));
    assert_eq!(snapshot.session_count, 1);
    assert_eq!(snapshot.card_count, 3);
    let cards = &snapshot.groups[0].cards;
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0].request_utc_ms, Some(1_757_626_498_000));
    assert_eq!(cards[1].request_utc_ms, Some(1_757_626_497_000));
    assert_eq!(cards[2].request_utc_ms, Some(1_757_626_496_000));
    // The same title must not be merged.
    assert!(cards.iter().all(|card| card.title == "Same name"));
    // Card ids grow in insertion order.
    assert_eq!(cards[0].id, 3);
    assert_eq!(cards[2].id, 1);
}

#[test]
fn only_agent_settled_inserts_cards() {
    let board = TaskBoard::new();
    let session_data = session(Some(SESSION_ID), Some("Name"), None);
    board
        .on_event_json(&body("session_start", 1_757_626_400_000, &session_data))
        .unwrap();
    board
        .on_event_json(&body("agent_start", 1_757_626_410_000, &session_data))
        .unwrap();
    board
        .on_event_json(&body("turn_end", 1_757_626_420_000, &session_data))
        .unwrap();
    assert_eq!(board.card_count(), 0);
    assert!(board.update(render_at(1_757_626_500_000)).groups.is_empty());

    board
        .on_event_json(&body("agent_settled", 1_757_626_496_789, &session_data))
        .unwrap();
    assert_eq!(board.card_count(), 1);
}

#[test]
fn sessions_are_isolated() {
    let board = TaskBoard::new();
    board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_496_000,
            &session(Some("aaa"), Some("A"), None),
        ))
        .unwrap();
    board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_497_000,
            &session(Some("bbb"), Some("B"), None),
        ))
        .unwrap();
    board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_498_000,
            &session(Some("aaa"), Some("A2"), None),
        ))
        .unwrap();

    let snapshot = board.update(render_at(1_757_626_500_000));
    assert_eq!(snapshot.session_count, 2);
    assert_eq!(snapshot.groups[0].session_key, "aaa");
    assert_eq!(snapshot.groups[0].card_count, 2);
    assert_eq!(snapshot.groups[1].session_key, "bbb");
    assert_eq!(snapshot.groups[1].card_count, 1);
}

#[test]
fn the_title_uses_the_first_usable_field_only() {
    let board = TaskBoard::new();

    // 1. session_name wins and is the only value shown.
    board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_496_000,
            &session(Some("sid"), Some("Name"), Some("/cwd")),
        ))
        .unwrap();
    // 2. a blank session_name falls back to cwd.
    board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_497_000,
            &session(Some("sid"), Some("   "), Some("/cwd")),
        ))
        .unwrap();
    // 3. no name and a blank cwd fall back to session_id.
    board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_498_000,
            &session(Some("sid"), None, Some("")),
        ))
        .unwrap();
    // 4. no usable field at all: the reserved group shows the placeholder.
    board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_499_000,
            &session(None, None, None),
        ))
        .unwrap();

    let snapshot = board.update(render_at(1_757_626_500_000));
    assert_eq!(snapshot.groups.len(), 2);

    let named = &snapshot.groups[0].cards;
    assert_eq!(named[0].title, "sid");
    assert_eq!(named[0].title_source, FieldSource::SessionId);
    assert_eq!(named[1].title, "/cwd");
    assert_eq!(named[1].title_source, FieldSource::Cwd);
    assert_eq!(named[1].secondary, None);
    assert_eq!(named[2].title, "Name");
    assert_eq!(named[2].title_source, FieldSource::SessionName);
    assert_eq!(named[2].secondary.as_ref().unwrap().value, "/cwd");
    assert_ne!(named[2].secondary.as_ref().unwrap().value, named[2].title);

    let reserved = &snapshot.groups[1];
    assert_eq!(reserved.session_key, RESERVED_SESSION_KEY);
    assert!(reserved.is_ungrouped);
    assert_eq!(reserved.cards[0].title, PLACEHOLDER);
    assert_eq!(reserved.cards[0].title_source, FieldSource::None);
}

#[test]
fn start_time_priority_and_session_start_update() {
    let board = TaskBoard::new();
    let session_data = session(Some(SESSION_ID), Some("Name"), None);

    board
        .on_event_json(&body("session_start", 1_757_626_400_000, &session_data))
        .unwrap();
    board
        .on_event_json(&body("agent_start", 1_757_626_410_000, &session_data))
        .unwrap();
    board
        .on_event_json(&body("agent_settled", 1_757_626_496_789, &session_data))
        .unwrap();

    let snapshot = board.update(render_at(1_757_626_500_000));
    let card = &snapshot.groups[0].cards[0];
    // agent_start is the most recent start event, so it wins.
    assert_eq!(card.start_utc_ms, Some(1_757_626_410_000));
    assert_eq!(card.start_source, Some(StartSource::AgentStart));
    assert_eq!(card.duration_ms, 86_789);
    assert_eq!(card.duration_label, "1m 26s");
    assert!(!card.duration_anomaly);

    // A session_start that arrives later updates only later cards.
    board
        .on_event_json(&body("session_start", 1_757_626_490_000, &session_data))
        .unwrap();
    board
        .on_event_json(&body("agent_settled", 1_757_626_495_000, &session_data))
        .unwrap();

    let snapshot = board.update(render_at(1_757_626_500_000));
    let cards = &snapshot.groups[0].cards;
    assert_eq!(cards[0].start_utc_ms, Some(1_757_626_490_000));
    assert_eq!(cards[0].start_source, Some(StartSource::SessionStart));
    assert_eq!(cards[0].duration_ms, 5_000);
    // The first card keeps its original values.
    assert_eq!(cards[1].start_utc_ms, Some(1_757_626_410_000));
    assert_eq!(cards[1].duration_ms, 86_789);
}

#[test]
fn created_at_is_the_last_resort_before_the_request_time() {
    let board = TaskBoard::new();
    board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_496_789,
            &session(Some(SESSION_ID), Some("Name"), None),
        ))
        .unwrap();

    let card = &board.update(render_at(1_757_626_500_000)).groups[0].cards[0];
    // No start event and no created_at: the request time is used.
    assert_eq!(card.start_utc_ms, Some(1_757_626_496_789));
    assert_eq!(card.start_source, Some(StartSource::RequestTimestamp));
    assert_eq!(card.duration_ms, 0);
}

#[test]
fn a_start_after_the_request_never_produces_a_negative_duration() {
    let board = TaskBoard::new();
    let session_data = session(Some(SESSION_ID), Some("Name"), None);
    board
        .on_event_json(&body("agent_start", 1_757_626_496_000, &session_data))
        .unwrap();
    board
        .on_event_json(&body("agent_settled", 1_757_626_400_000, &session_data))
        .unwrap();

    let card = &board.update(render_at(1_757_626_500_000)).groups[0].cards[0];
    assert_eq!(card.duration_ms, 0);
    assert_eq!(card.duration_label, "0s");
    assert!(card.duration_anomaly);
}

#[test]
fn delete_removes_exactly_one_card() {
    let board = TaskBoard::new();
    let session_data = session(Some(SESSION_ID), Some("Same name"), None);
    for ts in [1_757_626_496_000_i64, 1_757_626_497_000, 1_757_626_498_000] {
        board
            .on_event_json(&body("agent_settled", ts, &session_data))
            .unwrap();
    }

    assert!(board.delete_card(2));
    assert_eq!(board.card_count(), 2);
    let cards = &board.update(render_at(1_757_626_500_000)).groups[0].cards;
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].id, 3);
    assert_eq!(cards[1].id, 1);
    assert!(!board.delete_card(2));

    // Deleting the last card of a session removes the group as well.
    assert!(board.delete_card(3));
    assert!(board.delete_card(1));
    assert_eq!(board.session_count(), 0);
    assert!(board.update(render_at(1_757_626_500_000)).groups.is_empty());
}

#[test]
fn a_duplicate_event_inserts_another_card() {
    let board = TaskBoard::new();
    let session_data = session(Some(SESSION_ID), Some("Same"), None);
    let body = body("agent_settled", 1_757_626_496_789, &session_data);
    board.on_event_json(&body).unwrap();
    board.on_event_json(&body).unwrap();
    assert_eq!(board.card_count(), 2);
}

#[test]
fn unknown_and_malformed_events_do_not_change_the_board() {
    let board = TaskBoard::new();

    let outcome = board
        .on_event_json(&body(
            "agent_finished",
            1_757_626_496_000,
            &session(Some(SESSION_ID), None, None),
        ))
        .unwrap();
    assert!(matches!(outcome, IngestOutcome::IgnoredUnknownEvent { .. }));

    assert!(board.on_event_json("{not json").is_err());
    assert!(board.on_event_json("").is_err());
    assert_eq!(board.card_count(), 0);
    assert_eq!(board.revision(), 0);
}

#[test]
fn a_missing_session_data_does_not_panic() {
    let board = TaskBoard::new();
    board
        .on_event_json(r#"{"event":"agent_settled","timestamp":1757626496789}"#)
        .unwrap();
    board
        .on_event_json(r#"{"session_data":null,"event":"agent_settled","timestamp":1757626496790}"#)
        .unwrap();
    board
        .on_event_json(r#"{"session_data":{},"event":"agent_settled","timestamp":1757626496791}"#)
        .unwrap();

    let snapshot = board.update(render_at(1_757_626_500_000));
    assert_eq!(snapshot.session_count, 1);
    assert_eq!(snapshot.card_count, 3);
    assert_eq!(snapshot.groups[0].session_key, RESERVED_SESSION_KEY);
    assert!(snapshot.groups[0]
        .cards
        .iter()
        .all(|card| card.title == PLACEHOLDER));
}

#[test]
fn a_seconds_timestamp_is_converted() {
    let board = TaskBoard::new();
    let outcome = board
        .on_event_json(&body(
            "agent_settled",
            1_757_626_496,
            &session(Some(SESSION_ID), None, None),
        ))
        .unwrap();
    assert!(matches!(
        outcome,
        IngestOutcome::CardInserted {
            seconds_corrected: true,
            ..
        }
    ));
    let card = &board.update(render_at(1_757_626_500_000)).groups[0].cards[0];
    assert_eq!(card.request_utc_ms, Some(1_757_626_496_000));
}

#[test]
fn the_revision_changes_only_when_the_list_changes() {
    let board = TaskBoard::new();
    let session_data = session(Some(SESSION_ID), Some("Name"), None);
    assert_eq!(board.update(render_at(1_757_626_500_000)).revision, 0);
    board
        .on_event_json(&body("agent_start", 1_757_626_400_000, &session_data))
        .unwrap();
    assert_eq!(board.update(render_at(1_757_626_500_000)).revision, 0);
    board
        .on_event_json(&body("agent_settled", 1_757_626_496_789, &session_data))
        .unwrap();
    assert_eq!(board.update(render_at(1_757_626_500_000)).revision, 1);
    board.delete_card(1);
    assert_eq!(board.update(render_at(1_757_626_500_000)).revision, 2);
}

#[test]
fn a_typed_event_can_skip_json() {
    use pi_jingle_server::taskboard::{EventKind, SessionInput};

    let board = TaskBoard::new();
    let outcome = board.on_event_at(
        AgentEvent {
            kind: EventKind::AgentSettled,
            timestamp: Some(1_757_626_496_789),
            session: SessionInput {
                session_id: Some(SESSION_ID.to_string()),
                session_name: Some("Typed".to_string()),
                ..SessionInput::default()
            },
        },
        UtcMillis::from_millis(1_757_626_496_800),
    );
    assert!(matches!(outcome, IngestOutcome::CardInserted { .. }));
    let card = &board.update(render_at(1_757_626_500_000)).groups[0].cards[0];
    assert_eq!(card.title, "Typed");
}
