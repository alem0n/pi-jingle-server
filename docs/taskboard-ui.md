# Agent task board: design and integration

This document describes the Tauri window in this project. The window shows one
card for each `agent_settled` event. The module keeps all state in memory. It
does not write files and it does not keep raw JSON.

## 1. Files

| File | Purpose |
|---|---|
| `src/taskboard/mod.rs` | Public entry point: `TaskBoard`. |
| `src/taskboard/event.rs` | Lenient JSON parser and input types. |
| `src/taskboard/model.rs` | Map, cards, start time, render snapshot. |
| `src/taskboard/time.rs` | UTC storage, RFC 3339 parsing, local time text. |
| `src/taskboard/snap.rs` | Pure geometry for the top-edge park, the band and the slide rectangle. |
| `src/taskboard/detail.rs` | Pure geometry for the card detail panel: left or right of the window. |
| `src/taskboard/window.rs` | Tauri windows, drag, park, detail panel and IPC commands. |
| `src/desktop.rs` | Server thread, event bridge, Tauri setup. |
| `ui/index.html`, `ui/app.js`, `ui/styles.css` | Task board page. |
| `ui/detail.html`, `ui/detail.js`, `ui/detail.css` | Card detail page, in its own window. |
| `tauri.conf.json` | Window definition, CSP, window label. |
| `capabilities/default.json` | Tauri capabilities for the task board and detail windows. |
| `icons/icon.ico` | Window icon. |

The `desktop` feature is enabled by default. A build without it has no Tauri
dependency and no window:

```text
cargo build                        # window + HTTP receiver
cargo build --no-default-features  # HTTP receiver only
```

## 2. Public interface

### 2.1 Entry point

```rust
pub struct TaskBoard { /* Mutex<Board> */ }

impl TaskBoard {
    pub fn new() -> Self;
    pub fn on_event(&self, event: AgentEvent) -> IngestOutcome;
    pub fn on_event_at(&self, event: AgentEvent, now: UtcMillis) -> IngestOutcome;
    pub fn on_event_json(&self, text: &str) -> Result<IngestOutcome, EventError>;
    pub fn on_event_json_bytes(&self, bytes: &[u8]) -> Result<IngestOutcome, EventError>;
    pub fn update(&self, ctx: RenderCtx) -> BoardSnapshot;
    pub fn delete_card(&self, card_id: CardId) -> bool;
    pub fn revision(&self) -> u64;
    pub fn card_count(&self) -> usize;
    pub fn session_count(&self) -> usize;
}
```

`TaskBoard` is `Send + Sync`. The HTTP receiver, the bridge thread and the
window commands use one instance.

### 2.2 Event input

`on_event_json` parses one notification body and calls `on_event`. A malformed
body returns `Err(EventError)` and the board does not change.

The input types are:

```rust
pub struct AgentEvent {
    pub kind: EventKind,           // session_start | agent_start | agent_settled | Other(String)
    pub timestamp: Option<i64>,    // raw `timestamp` field, UTC epoch ms
    pub session: SessionInput,
}

pub struct SessionInput {
    pub session_id: Option<String>,
    pub created_at: Option<String>, // RFC 3339 text, as sent
    pub cwd: Option<String>,
    pub session_name: Option<String>,
    pub persisted: Option<bool>,
    pub version: Option<u64>,
}

pub enum IngestOutcome {
    CardInserted { card_id: u64, session_key: String, start_source: Option<StartSource>,
                   duration_ms: i64, seconds_corrected: bool },
    StartRecorded { session_key: String, source: StartSource, at: Option<UtcMillis>,
                    seconds_corrected: bool },
    IgnoredUnknownEvent { raw: String },
}
```

`AgentEvent::from_json(text)` converts the JSON body without touching a board.
`SessionInput::from_legacy_placeholders(...)` converts the placeholder fields of
the HTTP receiver, where `Unknown` means "absent".

The parser accepts these inputs without an error:

- `session_data` missing, `null` or empty;
- any `session_data` field missing, `null` or of the wrong type;
- `timestamp` missing, or given as text or float;
- `event` missing or unknown;
- extra fields, including a misspelled `create_at`.

`In` = one JSON body or one `AgentEvent`. `Out` = `IngestOutcome`, or
`Err(EventError)` for invalid JSON. A valid body with unusable fields is not an
error.

### 2.3 Render snapshot

`update(RenderCtx)` returns `BoardSnapshot`. It has no raw input. All times are
display text plus the UTC epoch millisecond value:

```rust
pub struct BoardSnapshot {
    pub revision: u64,
    pub generated_at_utc_ms: i64,
    pub generated_at_local: String,
    pub generated_at_utc: String,
    pub zone_label: String,        // for example "UTC+08:00"
    pub zone_fallback: bool,       // true when local conversion failed
    pub session_count: usize,
    pub card_count: usize,
    pub groups: Vec<GroupView>,    // first-seen order
}

pub struct GroupView {
    pub order: u64,
    pub session_key: String,
    pub is_ungrouped: bool,        // true for the reserved key "unknown"
    pub heading: String,
    pub heading_source: FieldSource,
    pub session_id: Option<String>,
    pub persisted: Option<bool>,
    pub version: Option<u64>,
    pub card_count: usize,
    pub cards: Vec<CardView>,      // index 0 = newest
}

pub struct CardView {
    pub id: u64,
    pub title: String,             // exactly one value
    pub title_source: FieldSource,
    pub secondary: Option<SecondaryView>,
    pub start_clock: Option<String>,     // local HH:MM:SS
    pub start_local: Option<String>,     // local YYYY-MM-DD HH:MM:SS
    pub start_utc: Option<String>,       // UTC text, for tooltips
    pub start_utc_ms: Option<i64>,
    pub start_source: Option<StartSource>,
    pub request_clock: Option<String>,
    pub request_local: Option<String>,
    pub request_utc: Option<String>,
    pub request_utc_ms: Option<i64>,
    pub duration_label: String,          // for example "1m 26s"
    pub duration_ms: i64,                // 0 or more
    pub duration_anomaly: bool,          // start time is later than request time
    pub time_fallback_utc: bool,
}
```

`RenderCtx { now: UtcMillis, zone: Zone }` selects the clock and the time zone.
Use `RenderCtx::local_now()` in the window and `RenderCtx::at_utc(ms)` in tests.

### 2.4 Window commands and events

The web layer calls these commands:

| Command | Arguments | Result |
|---|---|---|
| `taskboard_snapshot` | none | `UiSnapshot` = `BoardSnapshot` + `window` phase |
| `taskboard_delete_card` | `cardId` | `true` when one card was removed |
| `taskboard_drag_begin` | none | `DragInfo` |
| `taskboard_drag_move` | `dx`, `dy` | none |
| `taskboard_drag_end` | none | none |
| `taskboard_park` | none | none |
| `taskboard_expand` | none | none |
| `taskboard_hover` | `inside` | none |
| `taskboard_card_detail` | `cardId` | `CardDetail` or `null` |
| `taskboard_show_detail` | `cardId`, `rowTopCss` | `true` when the card exists |
| `taskboard_detail_size` | `width`, `height` | `true` when the panel was placed |
| `taskboard_hide_detail` | none | none |
| `taskboard_detail_sync` | none | `CardDetail` for a late page load |
| `taskboard_quit` | none | none, the process exits |

Rust sends the event `taskboard://window-state` with `{ "parked": bool }`
after each phase change. `ipc_handler()` returns the handler for
`Builder::invoke_handler`.

The `window` part of `UiSnapshot` is:

```rust
pub struct WindowView {
    pub parked: bool,             // true while the window is above the screen
    pub unseen_cards: bool,       // true while the band light must blink
    pub scale_factor: f64,
    pub parked_band_px: u32,      // height of the visible band, physical px
    pub snap_threshold_px: i32,
}
```

## 3. In-memory map

The module has one container only:

```rust
pub struct Board {
    groups: HashMap<String, Group>, // session key -> group
    next_card_id: u64,              // card ids start at 1
    next_order: u64,                // first-seen order of a group
    revision: u64,                  // increases on insert and delete
}

pub struct Group {
    pub meta: SessionMeta,
    pub order: u64,
    anchor: Option<Anchor>,         // start anchor for the next card
    pub cards: Vec<Card>,           // index 0 = newest card
}

pub struct SessionMeta {
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub cwd: Option<String>,
    pub created_at: Option<UtcMillis>, // UTC epoch milliseconds
    pub persisted: Option<bool>,
    pub version: Option<u64>,
}

pub struct Card {
    pub id: u64,
    pub title: String,
    pub title_source: FieldSource,          // session_name | cwd | session_id | none
    pub secondary: Option<(String, FieldSource)>,
    pub start: Option<StartInstant>,
    pub request: Option<UtcMillis>,         // UTC epoch milliseconds
    pub duration_ms: i64,
    pub duration_anomaly: bool,
}

struct Anchor { at: UtcMillis, source: StartSource }
struct StartInstant { at: UtcMillis, source: StartSource }

pub struct UtcMillis(i64); // UTC epoch milliseconds, the only time type
```

The session key is the trimmed `session_id`. When `session_id` is missing or
blank, the key is the reserved value `unknown`
(`RESERVED_SESSION_KEY`). Cards with the same key share one `Vec<Card>`. A new
card is inserted at index 0. The newest card is therefore always at the top of
its group.

The group order comes from `order`, not from the map iteration order. Groups
are sorted by `order` in the snapshot. Different sessions never mix.

No field of `Card` or `SessionMeta` holds the original JSON text. The task board
keeps no cache and writes no file. The HTTP receiver still keeps its own event
log for `GET /api/jingle/events`. That log belongs to the receiver and the task
board does not read it.

### 3.1 One internal time type

Two input forms become one type:

| Input | Function | Result |
|---|---|---|
| `timestamp` (epoch ms) | `UtcMillis::from_epoch_field(i64)` | `UtcMillis` |
| `created_at` (RFC 3339) | `UtcMillis::from_rfc3339(&str)` | `Option<UtcMillis>` |

`UtcMillis` is a newtype around `i64` epoch milliseconds in UTC. The board
compares, subtracts and formats only this type. The conversion has no loss:
`1757626496789` and `2025-09-11T21:34:56.789Z` give the same value.

## 4. Integration

### 4.1 Initialize

```rust
use std::sync::Arc;
use pi_jingle_server::taskboard::{RenderCtx, TaskBoard};

let board = Arc::new(TaskBoard::new());
```

`src/desktop.rs` does this and stores the clone in Tauri state:

```rust
let ui = Arc::new(TaskboardWindow::new(board.clone()));
tauri::Builder::default()
    .manage(ui.clone())
    .invoke_handler(ipc_handler())
    .setup(move |app| {
        let window = app.get_webview_window(WINDOW_LABEL).ok_or("no window")?;
        attach(&window, &ui)?;
        Ok(())
    })
    .build(tauri::generate_context!())?
    .run(|_, _| {});
```

### 4.2 Push events

The receiver broadcasts each accepted notification. One bridge thread converts
it and pushes it:

```rust
loop {
    match receiver.blocking_recv() {
        Ok(stored) => { board.on_event(agent_event(&stored.notification)); }
        Err(RecvError::Lagged(n)) => eprintln!("dropped {n} events"),
        Err(RecvError::Closed) => return,
    }
}
```

`agent_event` maps the receiver placeholders to `None`:

```rust
AgentEvent {
    kind: EventKind::from_raw(&note.event),
    timestamp: i64::try_from(note.timestamp).ok(),
    session: SessionInput::from_legacy_placeholders(
        &note.session_data.session_id,
        &note.session_data.created_at,
        &note.session_data.cwd,
        &note.session_data.session_name,
        &note.session_data.persisted,
        &note.session_data.version,
    ),
}
```

Any other caller can push the same way. A caller with JSON can use
`board.on_event_json(&text)`.

### 4.3 Render loop

The web layer runs a 200 ms timer. Each tick calls `taskboard_snapshot`. The
command calls `board.update(RenderCtx::local_now())` and adds the window phase:

```rust
let snapshot = board.update(RenderCtx::local_now());
// UiSnapshot { board: snapshot, window: WindowView { parked, unseen_cards, ... } }
```

The page redraws the list only when `revision` changes. The footer clock updates
on each tick. A Rust change wakes the loop immediately through the
`taskboard://window-state` event.

### 4.4 Delete

The delete button calls `taskboard_delete_card` with `{ cardId }`. The command
calls `board.delete_card(card_id)`. The model removes that card and keeps the
other cards of the session. When the last card of a session is removed, the
group entry is removed too. The page removes the row at once and refreshes the
snapshot.

## 5. Time handling

- Storage: UTC epoch milliseconds in `UtcMillis`. `created_at` is parsed, not
  stored as text.
- RFC 3339 parsing: `UtcMillis::from_rfc3339` calls
  `chrono::DateTime::parse_from_rfc3339` and then `timestamp_millis()`.
  Dependency: `chrono 0.4` with the `clock` feature.
- Local conversion: `UtcMillis::display(Zone::Local)` calls
  `chrono::Local::timestamp_millis_opt(ms)`. `DisplayTime` holds the full text
  (`YYYY-MM-DD HH:MM:SS`), the clock text (`HH:MM:SS`) and a fallback flag.
- Fallback: when `LocalResult::None` occurs (for example a daylight-saving
  gap), `display` returns UTC text with the suffix ` UTC` and
  `utc_fallback = true`. `zone_label` then returns `UTC` and
  `BoardSnapshot::zone_fallback` is `true`.
- Zone label: `zone_label(Zone::Local, now)` uses
  `Local::now().offset().local_minus_utc()` and returns text such as
  `UTC+08:00`. `format_utc_offset` builds the text.
- Duration text: `format_duration(ms)` returns `0s`, `3.250s`, `2m 05s` or
  `1h 02m 05s`.
- No retroactive change: a card keeps the start time and the duration that it
  received at insert. A later start event changes only later cards. The
  `revision` value does not change, so the list is not redrawn.
- Seconds: `UtcMillis::from_epoch_field` treats a value between
  `1_000_000_000` and `100_000_000_000` as epoch seconds and multiplies it by
  1000. `IngestOutcome::…seconds_corrected` reports the conversion.
- Negative input: `timestamp` below zero becomes `None`.

## 6. Main display field

The card title uses this order. The first usable value wins and only that value
is shown:

1. `session_data.session_name`
2. `session_data.cwd`
3. `session_data.session_id`
4. `—` (`PLACEHOLDER`)

A value is usable when it is present, not `null`, and not empty after
`trim()`. A value with only spaces is not usable. The title is never an empty
string and the title is never a join of two values.

The title uses the `session_data` of the card's own event. A later event does
not change an existing card. When the title comes from `session_name`, the cwd
can appear as the small second line. That line is omitted when the value equals
the title. When the title comes from `cwd` or `session_id`, there is no second
line.

The group heading uses the same order, with the merged session metadata.
`title_source` and `heading_source` report the chosen field for tests and
tooltips.

## 7. Park above the screen and expand

The window is not resized. A park moves the whole window above the screen top
edge, so only a thin band at the bottom edge of the window stays visible. An
expand moves the same window back into the screen. The list keeps its size, its
x position and its content.

All values are physical pixels.

| Constant | Value | Meaning |
|---|---|---|
| `SNAP_TOP_THRESHOLD_PX` | 24 | The window parks when its top edge is at or above `screen.y + 24`. |
| `PARKED_BAND_PX` | 8 | Height of the visible band. Its width is the window width. |
| `INITIAL_TOP_INSET_PX` | 40 | Distance from the screen top to the initial window position. |
| `EDGE_MARGIN_PX` | 8 | Distance from the initial window to each screen edge. |
| `DEFAULT_WINDOW_WIDTH` | 440 | Initial and fallback width. |
| `DEFAULT_WINDOW_HEIGHT` | 580 | Initial and fallback height. |
| `MIN_WINDOW_WIDTH_PX` | 360 | Minimum width. |
| `MIN_WINDOW_HEIGHT_PX` | 240 | Minimum height. |
| `HOVER_GRACE` | 300 ms | Time before a hover can expand the window. |
| `LEAVE_GRACE` | 200 ms | Time the pointer must stay outside a peek panel. |
| `WATCHDOG_INTERVAL` | 200 ms | Screen and pointer check interval. |
| `SLIDE_DURATION` | 160 ms | Length of the park and expand slide. |
| `SLIDE_FRAME` | 16 ms | Pause between two slide steps. |

The drag workflow:

1. The pointer goes down on the header. The page calls `taskboard_drag_begin`.
2. The page sends the pointer delta in CSS pixels with `taskboard_drag_move`.
   Rust multiplies the delta by the scale factor and moves the window.
3. Rust compares the top edge of the target rectangle with the screen top.
4. When the top edge enters the threshold, the window is parked. The size and
   the x position do not change. The y position becomes
   `screen.y - height + PARKED_BAND_PX`. Only the bottom band stays visible.
5. The park ends the drag. More move events in the same drag are ignored. The
   state therefore cannot oscillate during one drag.

Park geometry: `SnapController::park` keeps the width, the height and the x
position. Only y changes. The page hides the list and shows the band. The band
is the bottom `PARKED_BAND_PX` px of the window at the screen top edge.

Expand policy: `SnapController::expand` moves the window back with the top edge
at `screen.y`, the same x, and the same width and height. The whole list is
visible again. A repeated park and expand produces the exact same rectangles.
This is the "move the list back into the screen" strategy.

Slide: a park and an expand do not jump. `slide` moves the window in
`SLIDE_DURATION` with steps of `SLIDE_FRAME`. The curve is an ease-out: fast at
the start, slow at the end, no bounce. Every step sets only the y position, so
the list does not resize or reflow during the move. A drag, a new park or a new
expand stops the running slide at once; the watchdog also stays away while a
slide runs. When the sizes differ, for example after a screen change, the
rectangle is applied at once and the slide is skipped.

The page keeps the list visible while the window slides up, then shows the
band. On an expand it shows the list at once, and the window slides in.

Status light: the band holds one lamp. The lamp is a short horizontal bar of
40 × 4 px.

- Idle: dim steady light.
- A card arrives while the window is parked: the lamp blinks on and off every
  450 ms.
- The user opens the window (click or hover): the blink stops, the lamp returns
  to idle, and the new cards count as seen.
- A park with no new cards does not blink. The next card starts the blink
  again.

`Unseen::observe(parked, revision)` compares the board revision with the last
revision the user saw. It sets `WindowView::unseen_cards`, and the page drives
the lamp from that flag with a timer. The lamp does not need a CSS animation,
so it also blinks when the system asks for reduced motion.

Close button: the top-left corner of the header has a close button. It calls
`taskboard_quit`, which calls `AppHandle::exit(0)`. The receiver stops with the
process. The board is in memory only, so a close discards the cards.

### Detail panel

A click on a card row shows the complete card. A hover shows the same panel
after 280 ms. The panel is a second window, so it can sit outside the task
board window:

- The `detail` window is 320 CSS px wide. It has no decorations, a transparent
  background, no shadow and no taskbar entry. It stays on top, takes no focus
  and takes no pointer input, so a click goes through the panel to the window
  below.
- The detail page draws the card, measures the panel and calls
  `taskboard_detail_size`. Rust converts the CSS px to physical px, places the
  window, then shows it. The window stays hidden until it has the right size
  and place, so the panel does not jump.

Placement (`detail::place_detail`):

- The panel opens next to the task board window, at the height of the row.
- A side has room when the space is at least `panel width + DETAIL_GAP_PX`.
- Both sides have room: the panel goes to the left side.
- One side has room: the panel goes to that side.
- No side has room: the panel goes to the side with more space.
- The result is clamped to the screen with `DETAIL_MARGIN_PX`.

| Constant | Value | Meaning |
|---|---|---|
| `DETAIL_GAP_PX` | 8 | Distance between the panel and the task board window. |
| `DETAIL_MARGIN_PX` | 8 | Smallest distance between the panel and a screen edge. |
| `DETAIL_PANEL_WIDTH_CSS` | 320 | Panel width in CSS px. |
| `DETAIL_DELAY_MS` | 280 | Hover delay before the panel opens. A click skips it. |

A pinned panel (opened by a click) stays when the pointer leaves. It closes on
a second click on the same row, a click on an empty area, `Esc`, a list scroll,
a new card, a park, a drag or a delete. The panel also closes when the window
parks or moves: it is placed next to the window, so the old place would be
wrong.

The default initial position is the top-right corner of the primary screen,
40 px below the top edge.

Hover and click:

- A click on the band always expands the window.
- A hover expands the window only when the pointer is outside the band after
  `HOVER_GRACE` and then enters the band. This rule stops an immediate
  re-expand when the pointer is still on the band after the park.
- An expanded window from a hover or a click is a peek panel. When the pointer
  leaves it for `LEAVE_GRACE`, the window is parked again. The band returns at
  the same x, the same width and the same height.
- A drag makes the window permanent. The pointer grab on the header clears the
  peek state, so the window stays open after the drag.
- A drag that starts at the top edge (a peek panel) does not park. The user can
  move the panel down. A drag that starts below the snap threshold parks the
  window when the top edge enters the threshold.
- The page reports `pointerenter` and `pointerleave` with `taskboard_hover`.
  The watchdog also reads `cursor_position()` every `WATCHDOG_INTERVAL`. A
  window that appears under the cursor still expands.

Screen changes:

- `snap::reconcile` runs every `WATCHDOG_INTERVAL` and after each drag.
- A parked window keeps its band at the top edge of the new screen. The width
  and the height shrink only when the window is larger than the screen.
- An expanded window is clamped inside the screen. A top-flush window is not
  pushed down, and a window at another y position is not moved to the top.
- `screen_for` uses the center of the window bottom edge. For a parked window
  that point is inside the visible band.

Coordinate note: Windows adds a small invisible frame around the client area.
`current_rect` uses `inner_position` and `inner_size`. `apply_inner_position`
adds the measured frame offset. The constants therefore describe the visible
client area.

Scale factor note: the drag multiplies the CSS pointer delta by the scale
factor of the window at the start of the drag. A drag that crosses to a monitor
with a different scale factor can drift. The next drag is exact again.

Only the top edge parks. The left, right and bottom edges do not park. The
request marks these edges as optional.

Initial placement: the window starts at the top-right corner with the default
size, 40 px below the top edge. The configuration file keeps the window hidden
until `attach` applies the geometry.

## 8. Decisions and edge cases

| Case | Decision |
|---|---|
| Duplicate `agent_settled` (same session and timestamp) | Insert another card. The board does not deduplicate. |
| Different timestamps, same title | Two cards. Titles never merge cards. |
| Unknown `event` | Ignore. Write one line to stderr. The board does not change. |
| `session_data` missing | Use empty metadata. The card goes to the group `unknown`. |
| `session_id` missing or blank | Group key `unknown`. The title still uses `session_name` or `cwd` when usable, and `—` otherwise. |
| `session_name` blank | Use `cwd`, then `session_id`, then `—`. |
| `cwd` blank | Do not use it. Do not show an empty string. |
| `created_at` invalid | Ignore the value. The board uses the last valid `created_at` of the session. When none exists, the start time falls back to the request timestamp. |
| `timestamp` missing on `agent_settled` | Insert the card. The request time shows `—` and the duration is 0. The start time still follows the ladder below. |
| Start time after request time | Duration 0 with `duration_anomaly = true`. The row shows an amber value. |
| `timestamp` in seconds | Convert to milliseconds when the value is in the seconds range. |
| Local time zone unavailable | Show UTC text with the suffix `UTC`. |
| Delete the last card of a session | Remove the group entry. The start anchor is lost. |
| Start event without a card | Keep the group and its anchor in the map, but do not render the group. |
| Hover or click on the band | Move the whole window back into the screen: same x, same size, top edge at the screen top. |
| Pointer leaves the peek panel | Move the window above the screen again after `LEAVE_GRACE`. The band returns at the same place. |
| Park or expand | Slide in `SLIDE_DURATION`. A drag stops the slide. |
| Card arrives while parked | Start the band light blink. |
| User opens the window | Stop the blink. The cards count as seen. |
| Card arrives while expanded | No blink. The list is already on the screen. |
| Close button | `taskboard_quit` calls `AppHandle::exit(0)`. The board is in memory only. |
| Click a card row | Show the full card in the `detail` window at once. A second click on the same row closes it. |
| Hover a card row | Show the same panel after 280 ms. A pointer leave closes it. |
| Both sides of the window have room | The panel goes to the left side. |
| One side has room | The panel goes to that side. |
| No side has room | The panel goes to the side with more space and stays inside the screen. |
| Drag the peek panel | The window becomes permanent at the release position. The peek state ends. |
| Drag that starts at the top edge | Do not park during that drag. The user moves the panel away. |

Start-time ladder:

1. The session anchor. The anchor is set by an `agent_start` event, by a
   `session_start` event, or by the UTC time of the previous insert.
2. `session_data.created_at`.
3. The `agent_settled` timestamp.

The most recent start event sets the anchor. Section 3.3 of the request puts
`agent_start` before `session_start`. Section 3.5 of the request says that a
`session_start` event updates the session start. The board satisfies both rules
with one rule: the latest start event wins. In the usual order
(`session_start`, then `agent_start`, then `agent_settled`) the anchor is
`agent_start`, as section 3.3 requires. When a `session_start` event arrives
after the `agent_start` event, the anchor is `session_start`, as section 3.5
requires. Tests cover both orders.

Section 5 of the request also says that the board must record the current UTC
time after an insert. The board sets the anchor to `now` at the end of each
insert. The next card of that session measures from this value. The anchor does
not change a card that already exists.

The HTTP receiver still rejects an unknown event or an empty `session_id` with
status 400. The task board itself accepts these inputs for direct callers. The
receiver is not part of the task board module.

## 9. Manual test checklist

Use PowerShell. Start the program first:

```powershell
cargo build
.\target\debug\pi-jingle-server.exe
```

The receiver listens on port 8080 unless you set `JINGLE_PORT`. The window is
always on top. Send events with this helper:

```powershell
function Send-Event($body) {
  Invoke-RestMethod -Method Post -Uri 'http://127.0.0.1:8080/api/jingle/notify' `
    -ContentType 'application/json' -Body $body
}
```

Check each item. Record pass or fail.

### 9.1 Empty state

1. Start the program with no events.
2. Look at the window.

Expected: the header shows "Agent tasks" and "Waiting for agent_settled
events". The list shows "No finished tasks yet". The footer shows the zone label
(for example `UTC+08:00`) and the local time.

### 9.2 Parsing and the UTC to local check

1. Send the sample body:

```powershell
Send-Event '{"session_data":{"session_id":"7f3c9a2e-1b4d-4c8a-9e2f-3a1b2c3d4e5f","persisted":true,"created_at":"2025-09-12T10:30:00.000Z","version":3,"cwd":"/home/user/project","session_name":"Refactor auth module"},"event":"agent_settled","timestamp":1757626496789}'
```

2. Look at the card title.
3. Point at the time cell and read the tooltip.
4. Compare the values:

| Value | UTC | Local at UTC+08:00 |
|---|---|---|
| `created_at` | `2025-09-12 10:30:00.000` | `2025-09-12 18:30:00` |
| `timestamp` 1757626496789 | `2025-09-11 21:34:56.789` | `2025-09-12 05:34:56` |

Expected: the title is `Refactor auth module`. The second line is
`/home/user/project`. The start time is `18:30:00` and the request time is
`05:34:56` when the local zone is UTC+08:00. Your local text can differ by your
offset. The zone label in the footer shows your offset. The tooltip shows the
local text and the UTC text. The duration is `0s` with an amber value, because
the sample `created_at` is later than the sample `timestamp`. This result is
correct: the duration cannot be negative.

### 9.3 Main display field

Send these four bodies in order:

```powershell
Send-Event '{"session_data":{"session_id":"title-check","session_name":"Refactor auth module","cwd":"/home/user/project"},"event":"agent_settled","timestamp":1757626496000}'
Send-Event '{"session_data":{"session_id":"title-check","session_name":"   ","cwd":"/home/user/project"},"event":"agent_settled","timestamp":1757626497000}'
Send-Event '{"session_data":{"session_id":"title-check","session_name":null,"cwd":""},"event":"agent_settled","timestamp":1757626498000}'
Send-Event '{"session_data":{"session_id":null},"event":"agent_settled","timestamp":1757626499000}'
```

Expected, from top to bottom:

1. `title-check` (from `session_id`), in the group `unknown` at the bottom.
2. `title-check` (from `session_id`), in the group `title-check`.
3. `/home/user/project` (from `cwd`).
4. `Refactor auth module` (from `session_name`) with `/home/user/project` as
   the second line.

Check that no card shows two values on the title line. Check that no card shows
an empty title.

### 9.4 Three inserts in one session

1. Send five events for one session: `session_start`, `agent_start`, then three
   `agent_settled` events. Use one session id. Give two `agent_settled` events
   the same `session_name`.

```powershell
$s = '"session_id":"repeat-session","created_at":"2025-09-12T10:30:00.000Z","cwd":"/home/user/project"'
Send-Event "{`"session_data`":{$s,`"session_name`":`"Refactor auth module`"},`"event`":`"session_start`",`"timestamp`":1757626400000}"
Send-Event "{`"session_data`":{$s,`"session_name`":`"Refactor auth module`"},`"event`":`"agent_start`",`"timestamp`":1757626410000}"
Send-Event "{`"session_data`":{$s,`"session_name`":`"Refactor auth module`"},`"event`":`"agent_settled`",`"timestamp`":1757626496789}"
Send-Event "{`"session_data`":{$s,`"session_name`":`"Refactor auth module`"},`"event`":`"agent_settled`",`"timestamp`":1757626500000}"
Send-Event "{`"session_data`":{$s,`"session_name`":`"Second task`"},`"event`":`"agent_settled`",`"timestamp`":1757626510000}"
```

2. Count the cards in the group.

Expected: three cards. The newest event (`Second task`) is at the top. The two
`Refactor auth module` cards are both present, in reverse insertion order. The
`session_start` and `agent_start` events add no card.

3. Point at the time cell of a card.

Expected: the tooltip shows `Start source: agent_start` for the first card. The
other cards use `recorded`, because the board recorded the insert time for the
next card.

### 9.5 Delete

1. Click the `×` button of the middle card.
2. Look at the group.
3. Click the `×` button of the last card of a group.

Expected: step 2 shows one card fewer. The other cards, including cards with the
same title, do not change. Step 3 removes the group from the list when no card
remains.

### 9.6 Duplicate event

1. Send one body twice.
2. Count the cards.

Expected: two cards. The board does not deduplicate.

### 9.7 Park, peek and expand

1. Point at the header and hold the left mouse button.
2. Move the window to the top edge of the screen.
3. Release the button.
4. Move the pointer away for two seconds.
5. Move the pointer onto the thin band at the top edge.
6. Move the pointer away from the window.
7. Move the pointer onto the band again, then hold the header and drag the
   window down by 200 px.
8. Drag the window to the top edge again.

Expected for step 3: the window slides up in about 0.16 s. It keeps its size
and moves above the screen top edge. Only the bottom band of 8 px stays
visible. The list is not visible. The x position does not change.

Expected for step 5: the whole window slides back into the screen. The x
position, the width and the height do not change. The top edge is at the screen
top edge.

Expected for step 6: the window slides above the screen again. The visible band
is at the same x position, with the same width. The behaviour repeats without
drift.

Expected for step 7: the drag makes the window permanent. The window does not
park while the pointer leaves and returns. A drag stops a slide at once.

Expected for step 8: the window parks again, in the same way.

### 9.8 Screen change

1. Park the window.
2. Change the screen resolution, or move the window to the other monitor.
3. Wait two seconds.

Expected: the band stays visible and inside the screen. The band does not
disappear and does not cross a screen edge.

### 9.9 Start time and no negative duration

1. Send `agent_start` with timestamp `1757626499000` for a new session.
2. Send `agent_settled` with timestamp `1757626490000` for the same session.

Expected: the card shows `0s` in amber. The duration is not negative.

### 9.10 Status light

1. Park the window. Do not send an event.
2. Look at the bar in the band. Wait two seconds.
3. Send one `agent_settled` event.
4. Watch the bar for two seconds.
5. Hover the band. The window opens.
6. Move the pointer away. The window parks again.
7. Watch the bar for two seconds.

Expected for step 2: the bar is dim and steady.

Expected for step 4: the bar blinks on and off every 450 ms.

Expected for step 7: the bar is dim and steady again. The blink stops when the
window opens, and it does not return for the same cards.

### 9.11 Close button

1. Open the window.
2. Click the `×` button in the top-left corner.

Expected: the program exits and the window closes. The receiver stops with it.

### 9.12 Card detail panel

1. Point at a card row. Wait 0.5 s.
2. Move the pointer away.
3. Click the same row. Move the pointer to the other side of the screen.
4. Click an empty area of the list.
5. Click the row again. Press `Esc`.
6. Drag the window to the left side of the screen. Point at a card row.

Expected for step 1: the panel opens next to the task board window, at the
height of the row. It shows the complete card: title source, second value,
session id, persisted, version, start time in local time, UTC and epoch ms, the
start basis, the request time, the duration and the anomaly note when present.

Expected for step 2: the panel closes at once.

Expected for step 3: the panel opens at once and stays open when the pointer
leaves the window. A click goes through the panel to the desktop.

Expected for step 4: the panel closes.

Expected for step 5: the panel opens and then closes on `Esc`.

Expected for step 6: the panel opens on the right side of the window. The left
side has no room, because the window is at the left screen edge.

## 10. Build, run, test

```text
cargo build
cargo clippy --all-targets
cargo test
cargo build --no-default-features
```

On Windows the program does not open a console window. Logs go to the terminal
that started the program. Use a redirection when a script starts the program:

```powershell
Start-Process .\target\debug\pi-jingle-server.exe `
  -RedirectStandardOutput target\app.log -RedirectStandardError target\app.err
```

The tests cover the map, the JSON parser, the title order, the start time
order, the duration, the delete command, the snap geometry, the slide steps, the
unseen-card tracker and the detail panel placement. The integration tests in
`tests/taskboard.rs` use only the public API.

## 11. Platform support

| Platform | Build | Window | Drag, park and slide | Status |
|---|---|---|---|---|
| Windows 10 and 11 | yes | yes | yes | Verified in this workspace. |
| macOS | should build | should run | should work | Not verified here. Needs the Xcode command line tools. |
| Linux with X11 | should build | should run | should work | Not verified here. Needs `webkit2gtk-4.1` and `libsoup-3.0`. |
| Linux with Wayland | should build | should run | no | Wayland does not let a client place its own window. |

Facts behind the table:

- The task board module calls only public Tauri APIs. It has no Win32 call
  and no Windows crate.
- The only platform attribute is `#![cfg_attr(all(windows, feature =
  "desktop"), windows_subsystem = "windows")]` in `src/main.rs`. On other
  platforms it has no effect.
- The snap geometry is pure math in `src/taskboard/snap.rs`. The unit tests do
  not open a window, so they run on every platform.
- The page uses standard HTML, CSS and JavaScript. It needs Pointer Events and
  `Element.replaceChildren`. WebKit and WebView2 support both.

Limits on Wayland:

- A Wayland client cannot set its own window position. `set_position` and
  `inner_position` do not work. A drag cannot move the window and the park
  cannot work. The detail window has the same limit: the compositor picks its
  place, so the panel does not open next to the row.
- Transparency of the detail window needs a compositor. Most desktops have one.
- `alwaysOnTop` and `skipTaskbar` are compositor dependent. GNOME can ignore
  them.
- Use an X11 session for the park function, or add a fallback: use
  `start_dragging()` for the drag and disable the park when `WAYLAND_DISPLAY`
  is set.

Work needed for a release on macOS and Linux:

1. Add `icons/icon.icns` and PNG sizes to `icons/`. Add them to
   `bundle.icon` in `tauri.conf.json`. Set `bundle.active` to `true`.
2. Install the system build dependencies of Tauri for the target platform.
3. Run `cargo build` and the manual checklist from section 9 on the target.
4. Add a CI matrix for `windows-latest`, `macos-latest` and `ubuntu-latest`.
   The Ubuntu job must install `libwebkit2gtk-4.1-dev`, `libssl-dev`,
   `libayatana-appindicator3-dev`, `librsvg2-dev` and `patchelf`.
