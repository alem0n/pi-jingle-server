//! Tauri window adapter for the task board (`desktop` feature).
//!
//! The window is undecorated. Dragging is done in the web layer: the header
//! sends the pointer delta, and Rust moves the window in physical pixels. This
//! keeps the snap decision under our control during the whole drag, so the park
//! happens once and cannot flicker.
//!
//! A drag to the screen top moves the whole window above the screen top edge.
//! Only a thin band at the bottom edge of the window stays visible. A hover or a
//! click on that band moves the whole window back into the screen at the same x
//! position. A pointer leave moves it out again. Nothing is resized.
//!
//! Geometry rules live in [`super::snap`]. This file only reads the screen,
//! applies rectangles and speaks IPC.
//!
//! Channel summary:
//!
//! * `taskboard_snapshot` — the render loop pulls a snapshot about every 200 ms.
//! * `taskboard_delete_card` — the delete button.
//! * `taskboard_drag_begin` / `_move` / `_end` — window drag.
//! * `taskboard_park` / `taskboard_expand` / `taskboard_hover` — window phase.
//! * `taskboard://window-state` — Rust tells the web layer that the phase changed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use super::detail::place_detail;
use super::model::{BoardSnapshot, CardDetail, CardId, RenderCtx};
use super::snap::{
    default_window_rect, Rect, ScreenRect, SnapController, MIN_WINDOW_HEIGHT_PX,
    MIN_WINDOW_WIDTH_PX, PARKED_BAND_PX, SNAP_TOP_THRESHOLD_PX,
};
use super::TaskBoard;

/// Label of the window in `tauri.conf.json`.
pub const WINDOW_LABEL: &str = "taskboard";

/// Label of the card detail window. It shows the panel outside the task board.
pub const DETAIL_LABEL: &str = "detail";

/// Event that carries one [`CardDetail`] to the detail window.
pub const DETAIL_EVENT: &str = "taskboard://detail";

/// Event the window emits when the phase changes.
pub const WINDOW_STATE_EVENT: &str = "taskboard://window-state";

/// A drag older than this is treated as stale and ignored.
const DRAG_TIMEOUT: Duration = Duration::from_secs(30);

/// Time after a park before a hover may expand the window again.
///
/// The pointer is normally on the new band right after a park. Without this
/// delay the window would expand again immediately. A click always expands
/// the window, whatever the delay.
const HOVER_GRACE: Duration = Duration::from_millis(300);

/// How long the pointer must stay outside a peek panel before it is parked.
/// The delay stops a park during a short pointer move across the edge.
const LEAVE_GRACE: Duration = Duration::from_millis(200);

/// How often the watchdog re-checks the screen bounds and the pointer.
const WATCHDOG_INTERVAL: Duration = Duration::from_millis(200);

/// Length of the park and expand slide.
const SLIDE_DURATION: Duration = Duration::from_millis(160);

/// Pause between two slide steps.
const SLIDE_FRAME: Duration = Duration::from_millis(16);

/// Panel width of the detail window, in CSS px.
pub const DETAIL_PANEL_WIDTH_CSS: f64 = 320.0;

/// Measured panel size must fit in the hidden detail window before the first
/// show, so the window starts big enough at any scale factor up to 300 %.
const DETAIL_INITIAL_WIDTH_CSS: f64 = DETAIL_PANEL_WIDTH_CSS + 24.0;
const DETAIL_INITIAL_HEIGHT_CSS: f64 = 900.0;

/// Shared state of the task board window.
#[derive(Debug)]
pub struct TaskboardWindow {
    board: Arc<TaskBoard>,
    snap: Mutex<SnapController>,
    drag: Mutex<Option<DragSession>>,
    /// `true` when the pointer must leave and re-enter the band before a
    /// hover expands the window again.
    hover_armed: AtomicBool,
    /// When the window was parked, used for the hover grace period.
    parked_since: Mutex<Option<Instant>>,
    /// `true` while the expanded window is a temporary hover panel. A peek
    /// panel is parked again when the pointer leaves it. A drag makes the
    /// window permanent.
    peek: AtomicBool,
    /// When the pointer was first seen outside a peek panel.
    peek_outside_since: Mutex<Option<Instant>>,
    /// Change counter of the slide. A newer value stops the running slide.
    slide_id: AtomicU64,
    /// `true` while a slide runs. The watchdog keeps its hands off then.
    sliding: AtomicBool,
    /// New-card light of the parked band.
    unseen: Mutex<Unseen>,
    /// Card shown in the detail window and the row it belongs to.
    detail: Mutex<DetailState>,
}

/// What the detail window shows.
#[derive(Clone, Debug, Default)]
struct DetailState {
    /// Card of the open panel, `None` when the panel is closed.
    card: Option<CardDetail>,
    /// Top edge of the row in CSS px, relative to the task board page.
    row_top_css: f64,
}

/// Tracks cards that arrived while the window was parked.
///
/// The band blinks from the first unseen card until the user opens the window.
/// An expanded window counts as seen: its list is on the screen.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unseen {
    seen: u64,
    blink: bool,
}

impl Unseen {
    /// Feed the current board revision. Returns `true` while the band blinks.
    pub fn observe(&mut self, parked: bool, revision: u64) -> bool {
        if parked {
            if revision != self.seen {
                self.blink = true;
            }
        } else {
            self.seen = revision;
            self.blink = false;
        }
        self.blink
    }

    /// `true` while the band blinks.
    pub fn is_blinking(&self) -> bool {
        self.blink
    }
}

/// One pointer drag in progress.
#[derive(Clone, Copy, Debug)]
struct DragSession {
    start_inner: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    scale: f64,
    started: Instant,
}

impl TaskboardWindow {
    /// Create the window state for a board.
    pub fn new(board: Arc<TaskBoard>) -> Self {
        let initial = Rect::new(
            0,
            0,
            super::snap::DEFAULT_WINDOW_WIDTH,
            super::snap::DEFAULT_WINDOW_HEIGHT,
        );
        Self {
            board,
            snap: Mutex::new(SnapController::new(initial)),
            drag: Mutex::new(None),
            hover_armed: AtomicBool::new(false),
            parked_since: Mutex::new(None),
            peek: AtomicBool::new(false),
            peek_outside_since: Mutex::new(None),
            slide_id: AtomicU64::new(0),
            sliding: AtomicBool::new(false),
            unseen: Mutex::new(Unseen::default()),
            detail: Mutex::new(DetailState::default()),
        }
    }

    /// The board behind this window.
    pub fn board(&self) -> &Arc<TaskBoard> {
        &self.board
    }

    /// `true` when the window is above the screen top edge.
    pub fn is_parked(&self) -> bool {
        lock(&self.snap).is_parked()
    }

    /// Mark the window as parked now.
    fn mark_parked(&self) {
        *lock(&self.parked_since) = Some(Instant::now());
    }

    /// Mark the window as expanded now.
    fn mark_expanded(&self) {
        *lock(&self.parked_since) = None;
    }

    /// `true` when the pointer may expand the window by hovering the band.
    fn hover_ready(&self) -> bool {
        lock(&self.parked_since).is_some_and(|since| since.elapsed() >= HOVER_GRACE)
    }

    /// `true` while the expanded window is a temporary hover panel.
    fn peek_active(&self) -> bool {
        self.peek.load(Ordering::SeqCst)
    }

    /// `true` while a slide moves the window.
    fn is_sliding(&self) -> bool {
        self.sliding.load(Ordering::SeqCst)
    }

    /// Stop the running slide, if any. The window stays where it is.
    fn stop_slide(&self) {
        self.slide_id.fetch_add(1, Ordering::SeqCst);
        self.sliding.store(false, Ordering::SeqCst);
    }

    /// Forget the card of the detail window.
    fn clear_detail(&self) {
        lock(&self.detail).card = None;
    }
}

/// What the web layer receives on every render tick.
#[derive(Clone, Debug, Serialize)]
pub struct UiSnapshot {
    /// Cards and groups.
    #[serde(flatten)]
    pub board: BoardSnapshot,
    /// Window phase and the geometry constants in use.
    pub window: WindowView,
}

/// Window phase for the web layer.
#[derive(Clone, Debug, Serialize)]
pub struct WindowView {
    /// `true` when the window is above the screen top edge.
    pub parked: bool,
    /// `true` while the band must blink: cards arrived while parked.
    pub unseen_cards: bool,
    /// Device pixel ratio of this window.
    pub scale_factor: f64,
    /// Height of the visible band in physical px.
    pub parked_band_px: u32,
    /// Snap threshold in physical px.
    pub snap_threshold_px: i32,
}

/// Payload of [`WINDOW_STATE_EVENT`].
#[derive(Clone, Copy, Debug, Serialize)]
pub struct WindowStatePayload {
    /// New phase.
    pub parked: bool,
}

/// Answer of [`taskboard_drag_begin`].
#[derive(Clone, Copy, Debug, Serialize)]
pub struct DragInfo {
    /// Current window rectangle.
    pub x: i32,
    /// Current window rectangle.
    pub y: i32,
    /// Current window rectangle.
    pub width: u32,
    /// Current window rectangle.
    pub height: u32,
    /// Device pixel ratio to convert CSS pointer deltas to physical pixels.
    pub scale_factor: f64,
    /// `true` when the window is parked; a drag is then ignored.
    pub parked: bool,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Render tick: board snapshot plus window phase.
#[tauri::command]
pub fn taskboard_snapshot(
    ui: State<'_, Arc<TaskboardWindow>>,
    window: WebviewWindow,
) -> UiSnapshot {
    let ui: &TaskboardWindow = &ui;
    let board = ui.board.update(RenderCtx::local_now());
    let parked = ui.is_parked();
    let unseen_cards = lock(&ui.unseen).observe(parked, board.revision);
    UiSnapshot {
        board,
        window: WindowView {
            parked,
            unseen_cards,
            scale_factor: window.scale_factor().unwrap_or(1.0),
            parked_band_px: PARKED_BAND_PX,
            snap_threshold_px: SNAP_TOP_THRESHOLD_PX,
        },
    }
}

/// Delete one card. Returns `true` when a card was removed.
#[tauri::command]
pub fn taskboard_delete_card(ui: State<'_, Arc<TaskboardWindow>>, card_id: CardId) -> bool {
    let ui: &TaskboardWindow = &ui;
    ui.board.delete_card(card_id)
}

/// Complete information about one card, for the detail popover.
#[tauri::command]
pub fn taskboard_card_detail(
    ui: State<'_, Arc<TaskboardWindow>>,
    card_id: CardId,
) -> Option<CardDetail> {
    let ui: &TaskboardWindow = &ui;
    ui.board.card_detail(card_id, RenderCtx::local_now())
}

/// Show the panel of one card in the separate detail window.
///
/// `row_top_css` is the top edge of the card row in CSS px, relative to the
/// task board page. The panel is placed at that height next to the window.
#[tauri::command]
pub fn taskboard_show_detail(
    app: tauri::AppHandle,
    ui: State<'_, Arc<TaskboardWindow>>,
    card_id: CardId,
    row_top_css: f64,
) -> bool {
    let ui: &TaskboardWindow = &ui;
    let Some(card) = ui.board.card_detail(card_id, RenderCtx::local_now()) else {
        return false;
    };
    {
        let mut detail = lock(&ui.detail);
        detail.card = Some(card.clone());
        detail.row_top_css = row_top_css;
    }
    if let Some(window) = app.get_webview_window(DETAIL_LABEL) {
        let _ = window.emit(DETAIL_EVENT, &card);
    }
    true
}

/// Size report of the detail window: place the panel, then show it.
///
/// The detail page measures its own panel, so the window gets exactly the size
/// of the content. `width` and `height` are CSS px.
#[tauri::command]
pub fn taskboard_detail_size(
    app: tauri::AppHandle,
    ui: State<'_, Arc<TaskboardWindow>>,
    width: f64,
    height: f64,
) -> bool {
    let ui: &TaskboardWindow = &ui;
    if ui.is_parked() || lock(&ui.detail).card.is_none() {
        return false;
    }
    let Some(main) = app.get_webview_window(WINDOW_LABEL) else {
        return false;
    };
    let Some(detail) = app.get_webview_window(DETAIL_LABEL) else {
        return false;
    };

    let scale = detail.scale_factor().unwrap_or(1.0);
    let panel_width = ((width.max(1.0) * scale).round() as u32).max(1);
    let panel_height = ((height.max(1.0) * scale).round() as u32).max(1);

    let client = current_rect(&main);
    let main_scale = main.scale_factor().unwrap_or(1.0);
    let anchor = lock(&ui.detail).row_top_css;
    let row_top = client
        .y
        .saturating_add((anchor * main_scale).round() as i32);
    let screen = screen_for(&main);
    let place = place_detail(client, screen, row_top, panel_width, panel_height);

    if let Err(err) = detail.set_size(PhysicalSize::new(panel_width, panel_height)) {
        eprintln!("[taskboard] detail set_size failed: {err}");
    }
    apply_inner_position(&detail, place.x, place.y);
    if !detail.is_visible().unwrap_or(false) {
        let _ = detail.show();
    }
    true
}

/// Hide the detail window and forget its card.
#[tauri::command]
pub fn taskboard_hide_detail(app: tauri::AppHandle, ui: State<'_, Arc<TaskboardWindow>>) {
    let ui: &TaskboardWindow = &ui;
    ui.clear_detail();
    if let Some(window) = app.get_webview_window(DETAIL_LABEL) {
        let _ = window.hide();
    }
}

/// Card the detail window must show when its page loads.
#[tauri::command]
pub fn taskboard_detail_sync(ui: State<'_, Arc<TaskboardWindow>>) -> Option<CardDetail> {
    let ui: &TaskboardWindow = &ui;
    lock(&ui.detail).card.clone()
}

/// Exit the application. The close button in the header calls this.
#[tauri::command]
pub fn taskboard_quit(app: tauri::AppHandle) {
    quit(&app);
}

/// Quit the program. The header close button and the tray menu call this.
///
/// The program has no taskbar entry, so a close request of the window hides it
/// instead of closing it. This function marks a real exit first, so the close
/// handler lets the exit through.
pub fn quit(app: &AppHandle) {
    QUITTING.store(true, Ordering::SeqCst);
    app.exit(0);
}

/// `true` after a quit request. A close request must then close the program.
pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// Start a window drag.
#[tauri::command]
pub fn taskboard_drag_begin(
    ui: State<'_, Arc<TaskboardWindow>>,
    window: WebviewWindow,
) -> DragInfo {
    let ui: &TaskboardWindow = &ui;
    let current = current_rect(&window);
    let parked = ui.is_parked();
    let scale = window.scale_factor().unwrap_or(1.0);

    if !parked {
        // The user takes control of the window. It stops being a hover panel
        // and any slide in progress stops.
        ui.stop_slide();
        ui.clear_detail();
        hide_detail_window(&window);
        ui.peek.store(false, Ordering::SeqCst);
        lock(&ui.snap).begin_drag(current);
        *lock(&ui.drag) = Some(DragSession {
            start_inner: PhysicalPosition::new(current.x, current.y),
            size: PhysicalSize::new(current.width, current.height),
            scale,
            started: Instant::now(),
        });
    }

    DragInfo {
        x: current.x,
        y: current.y,
        width: current.width,
        height: current.height,
        scale_factor: scale,
        parked,
    }
}

/// Move the window by a pointer delta measured in CSS pixels.
#[tauri::command]
pub fn taskboard_drag_move(
    ui: State<'_, Arc<TaskboardWindow>>,
    window: WebviewWindow,
    dx: f64,
    dy: f64,
) {
    let ui: &TaskboardWindow = &ui;
    let Some(session) = *lock(&ui.drag) else {
        return;
    };
    if session.started.elapsed() > DRAG_TIMEOUT {
        *lock(&ui.drag) = None;
        return;
    }
    if ui.is_parked() {
        return;
    }

    let target = Rect::new(
        session
            .start_inner
            .x
            .saturating_add((dx * session.scale).round() as i32),
        session
            .start_inner
            .y
            .saturating_add((dy * session.scale).round() as i32),
        session.size.width,
        session.size.height,
    );

    // The monitor under the top center of the dragged window decides the edge.
    let screen = screen_at(&window, target.center_x(), target.y.saturating_add(1));

    let parked_now = {
        let mut snap = lock(&ui.snap);
        snap.drag_step(target, screen)
    };

    if let Some(parked) = parked_now {
        *lock(&ui.drag) = None;
        ui.hover_armed.store(false, Ordering::SeqCst);
        ui.mark_parked();
        emit_state(&window, true);
        slide(ui, &window, current_rect(&window), parked);
        return;
    }

    apply_inner_position(&window, target.x, target.y);
}

/// Finish a window drag.
#[tauri::command]
pub fn taskboard_drag_end(ui: State<'_, Arc<TaskboardWindow>>, window: WebviewWindow) {
    let ui: &TaskboardWindow = &ui;
    *lock(&ui.drag) = None;
    if ui.is_parked() {
        return;
    }

    let screen = screen_for(&window);
    let current = current_rect(&window);
    let wanted = {
        let mut snap = lock(&ui.snap);
        let wanted = snap.reconcile(screen, current);
        snap.end_drag(wanted.unwrap_or(current));
        wanted
    };
    if let Some(rect) = wanted {
        apply_rect(&window, rect);
    }
}

/// Move the window above the screen top edge, for example from the snap button.
#[tauri::command]
pub fn taskboard_park(ui: State<'_, Arc<TaskboardWindow>>, window: WebviewWindow) {
    let ui: &TaskboardWindow = &ui;
    park_now(ui, &window);
}

/// Move a parked window back into the screen.
#[tauri::command]
pub fn taskboard_expand(ui: State<'_, Arc<TaskboardWindow>>, window: WebviewWindow) {
    let ui: &TaskboardWindow = &ui;
    expand_now(ui, &window);
}

/// Hover report from the band. `inside = false` arms the next hover.
#[tauri::command]
pub fn taskboard_hover(ui: State<'_, Arc<TaskboardWindow>>, window: WebviewWindow, inside: bool) {
    let ui: &TaskboardWindow = &ui;
    if !inside {
        if ui.hover_ready() {
            ui.hover_armed.store(true, Ordering::SeqCst);
        }
        return;
    }
    if !ui.is_parked() {
        return;
    }
    if ui.hover_armed.swap(false, Ordering::SeqCst) {
        expand_now(ui, &window);
    }
}

/// All task board commands, ready for `Builder::invoke_handler`.
pub fn ipc_handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        taskboard_snapshot,
        taskboard_delete_card,
        taskboard_card_detail,
        taskboard_show_detail,
        taskboard_detail_size,
        taskboard_hide_detail,
        taskboard_detail_sync,
        taskboard_quit,
        taskboard_drag_begin,
        taskboard_drag_move,
        taskboard_drag_end,
        taskboard_park,
        taskboard_expand,
        taskboard_hover,
    ]
}

// ---------------------------------------------------------------------------
// Window lifecycle
// ---------------------------------------------------------------------------

/// `true` after a quit request from the header button or the tray menu.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Place, show and start watching the window. Call this from the Tauri setup
/// hook, after the window exists.
pub fn attach(window: &WebviewWindow, ui: &Arc<TaskboardWindow>) -> tauri::Result<()> {
    let screen = screen_for(window);
    let initial = default_window_rect(screen);
    // Apply once before showing, then again after showing: Windows can ignore a
    // size request for a window that is still hidden.
    apply_rect(window, initial);
    window.show()?;
    apply_rect(window, initial);
    window.set_min_size(Some(PhysicalSize::new(
        MIN_WINDOW_WIDTH_PX,
        MIN_WINDOW_HEIGHT_PX,
    )))?;
    create_detail_window(window)?;
    lock(&ui.snap).begin_drag(initial);
    window.set_focus()?;

    let actual = current_rect(window);
    eprintln!(
        "[taskboard] window ready: client {actual:?} (wanted {initial:?}), screen {screen:?}, scale {:?}",
        window.scale_factor()
    );

    let watchdog_window = window.clone();
    let watchdog_ui = ui.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(WATCHDOG_INTERVAL);
        watchdog(&watchdog_window, &watchdog_ui);
    });

    Ok(())
}

/// Show or hide the task board window. The tray icon calls this.
///
/// A hidden window keeps its geometry. A show expands a parked window, so the
/// list is visible without a pointer hover. The detail window follows the task
/// board.
pub fn set_board_visible(app: &AppHandle, ui: &TaskboardWindow, visible: bool) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };

    if !visible {
        ui.clear_detail();
        hide_detail_window(&window);
        let _ = window.hide();
        return;
    }

    if ui.is_parked() {
        ui.stop_slide();
        *lock(&ui.drag) = None;
        ui.hover_armed.store(false, Ordering::SeqCst);
        ui.peek.store(false, Ordering::SeqCst);
        *lock(&ui.peek_outside_since) = None;
        let screen = screen_for(&window);
        if let Some(rect) = lock(&ui.snap).expand(screen) {
            ui.mark_expanded();
            emit_state(&window, false);
            apply_rect(&window, rect);
        }
    }

    let _ = window.show();
    let _ = window.set_focus();
}

/// Re-check the screen bounds and the pointer state once.
fn watchdog(window: &WebviewWindow, ui: &TaskboardWindow) {
    if lock(&ui.drag).is_some() {
        // Never fight a drag in progress.
        return;
    }
    if ui.is_sliding() {
        // Never fight a slide in progress.
        return;
    }

    let screen = screen_for(window);
    let current = current_rect(window);
    let wanted = lock(&ui.snap).reconcile(screen, current);
    if let Some(rect) = wanted {
        apply_rect(window, rect);
    }

    let rect = current_rect(window);
    let Ok(position) = window.cursor_position() else {
        return;
    };
    let inside = rect.contains(position.x as i32, position.y as i32);

    if ui.is_parked() {
        *lock(&ui.peek_outside_since) = None;
        // Hover detection. The web layer also reports pointer enter and leave,
        // but a window that appears under the cursor never receives that event,
        // so the cursor position is checked here as well.
        if !inside {
            if ui.hover_ready() {
                ui.hover_armed.store(true, Ordering::SeqCst);
            }
        } else if ui.hover_armed.swap(false, Ordering::SeqCst) {
            expand_now(ui, window);
        }
        return;
    }

    if !ui.peek_active() {
        *lock(&ui.peek_outside_since) = None;
        return;
    }

    // A peek panel is parked again when the pointer leaves it. The short delay
    // stops a park during a quick move across the window edge.
    if inside {
        *lock(&ui.peek_outside_since) = None;
        return;
    }

    let leave_confirmed = {
        let mut outside = lock(&ui.peek_outside_since);
        match *outside {
            None => {
                *outside = Some(Instant::now());
                false
            }
            Some(since) => since.elapsed() >= LEAVE_GRACE,
        }
    };

    if leave_confirmed {
        *lock(&ui.peek_outside_since) = None;
        if park_now(ui, window) {
            // The pointer is outside, so the next hover can expand at once.
            ui.hover_armed.store(true, Ordering::SeqCst);
        }
    }
}

fn park_now(ui: &TaskboardWindow, window: &WebviewWindow) -> bool {
    let screen = screen_for(window);
    let current = current_rect(window);
    let parked = lock(&ui.snap).park(screen, current);
    let Some(parked) = parked else {
        return false;
    };

    *lock(&ui.drag) = None;
    ui.hover_armed.store(false, Ordering::SeqCst);
    ui.peek.store(false, Ordering::SeqCst);
    *lock(&ui.peek_outside_since) = None;
    ui.mark_parked();
    // The panel sits next to the window, so it must go when the window moves.
    ui.clear_detail();
    hide_detail_window(window);
    // The page keeps the list visible while the window slides up, then shows
    // the band. It starts the change on this event.
    emit_state(window, true);
    slide(ui, window, current, parked);
    true
}

fn expand_now(ui: &TaskboardWindow, window: &WebviewWindow) -> bool {
    let screen = screen_for(window);
    let rect = lock(&ui.snap).expand(screen);
    let Some(rect) = rect else {
        return false;
    };
    let current = current_rect(window);

    *lock(&ui.drag) = None;
    *lock(&ui.peek_outside_since) = None;
    ui.hover_armed.store(false, Ordering::SeqCst);
    ui.peek.store(true, Ordering::SeqCst);
    ui.mark_expanded();
    emit_state(window, false);
    slide(ui, window, current, rect);
    let _ = window.set_focus();
    true
}

/// Move the window from `from` to `to` in a short slide.
///
/// Only the y position changes: a park and an expand never resize the window.
/// When the size differs, for example after a screen change, the rectangle is
/// applied at once. A drag or a newer slide stops the loop.
fn slide(ui: &TaskboardWindow, window: &WebviewWindow, from: Rect, to: Rect) {
    if from.width != to.width || from.height != to.height {
        apply_rect(window, to);
        return;
    }
    if from.y == to.y {
        apply_inner_position(window, to.x, to.y);
        return;
    }

    let id = ui.slide_id.fetch_add(1, Ordering::SeqCst) + 1;
    ui.sliding.store(true, Ordering::SeqCst);
    let steps = slide_steps();
    for step in 1..=steps {
        if ui.slide_id.load(Ordering::SeqCst) != id {
            // A drag or a newer slide took over. The owner clears the flag.
            return;
        }
        apply_inner_position(window, to.x, eased_y(from.y, to.y, step, steps));
        std::thread::sleep(SLIDE_FRAME);
    }
    if ui.slide_id.load(Ordering::SeqCst) == id {
        apply_inner_position(window, to.x, to.y);
        ui.sliding.store(false, Ordering::SeqCst);
    }
}

/// Number of position steps in a slide.
fn slide_steps() -> u32 {
    let frame = SLIDE_FRAME.as_millis().max(1);
    (SLIDE_DURATION.as_millis() / frame).max(2) as u32
}

/// y position of one slide step, with an ease-out curve.
///
/// The window moves fast at the start and slowly at the end, so it settles
/// without a bounce. Step 1 is close to `from` and the last step is `to`.
fn eased_y(from: i32, to: i32, step: u32, steps: u32) -> i32 {
    let steps = steps.max(1);
    let t = (step.min(steps) as f64) / (steps as f64);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    from + ((to - from) as f64 * eased).round() as i32
}

/// Hide the detail window, if it exists.
fn hide_detail_window(window: &WebviewWindow) {
    if let Some(detail) = window.app_handle().get_webview_window(DETAIL_LABEL) {
        let _ = detail.hide();
    }
}

/// Create the always-on-top panel window. It stays hidden until a card is
/// shown. The window does not take pointer input, so a click goes through it to
/// the window below.
fn create_detail_window(main: &WebviewWindow) -> tauri::Result<()> {
    let app = main.app_handle();
    if app.get_webview_window(DETAIL_LABEL).is_some() {
        return Ok(());
    }

    let detail =
        WebviewWindowBuilder::new(app, DETAIL_LABEL, WebviewUrl::App("detail.html".into()))
            .title("Pi Jingle - Task detail")
            .inner_size(DETAIL_INITIAL_WIDTH_CSS, DETAIL_INITIAL_HEIGHT_CSS)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .visible(false)
            .build()?;

    let _ = detail.set_ignore_cursor_events(true);
    Ok(())
}

/// Apply a window rectangle. The size stays the same during a park or an
/// expand; the call is kept for the initial placement and screen changes.
fn apply_rect(window: &WebviewWindow, rect: Rect) {
    set_geometry(window, rect);
}

/// Set the client size first, then the outer position. On Windows the size
/// must be applied first, otherwise the system can move the window while it
/// resizes. The frame can change with the size, so the geometry is applied
/// after a short pause and then checked once.
fn set_geometry(window: &WebviewWindow, rect: Rect) {
    if let Err(err) = window.set_size(PhysicalSize::new(rect.width.max(1), rect.height.max(1))) {
        eprintln!("[taskboard] set_size failed: {err}");
    }
    // Let the event loop apply the size change before the frame offset is
    // measured. The offset is wrong during the change.
    std::thread::sleep(Duration::from_millis(40));
    apply_inner_position(window, rect.x, rect.y);

    // One correction step: the frame can change with the window size.
    std::thread::sleep(Duration::from_millis(20));
    if let Ok(inner) = window.inner_position() {
        let (dx, dy) = (rect.x - inner.x, rect.y - inner.y);
        if dx != 0 || dy != 0 {
            let outer = window
                .outer_position()
                .unwrap_or(PhysicalPosition::new(0, 0));
            let _ = window.set_position(PhysicalPosition::new(outer.x + dx, outer.y + dy));
        }
    }
}

/// Position the client rectangle at `x, y` on the screen.
///
/// The Windows frame adds a few invisible pixels around the client area, so
/// the outer position is derived from the current frame offset. All task board
/// geometry uses client coordinates.
fn apply_inner_position(window: &WebviewWindow, x: i32, y: i32) {
    let (offset_x, offset_y) = frame_offset(window);
    if let Err(err) = window.set_position(PhysicalPosition::new(x + offset_x, y + offset_y)) {
        eprintln!("[taskboard] set_position failed: {err}");
    }
}

/// Outer origin minus client origin, in physical pixels.
fn frame_offset(window: &WebviewWindow) -> (i32, i32) {
    let outer = window
        .outer_position()
        .unwrap_or(PhysicalPosition::new(0, 0));
    let inner = window.inner_position().unwrap_or(outer);
    (outer.x - inner.x, outer.y - inner.y)
}

fn emit_state(window: &WebviewWindow, parked: bool) {
    if let Err(err) = window.emit(WINDOW_STATE_EVENT, WindowStatePayload { parked }) {
        eprintln!("[taskboard] emit failed: {err}");
    }
}

/// Client rectangle of the window: the part the web layer draws into.
fn current_rect(window: &WebviewWindow) -> Rect {
    let outer = window
        .outer_position()
        .unwrap_or(PhysicalPosition::new(0, 0));
    let position = window.inner_position().unwrap_or(outer);
    let size = window.inner_size().unwrap_or(PhysicalSize::new(
        super::snap::DEFAULT_WINDOW_WIDTH,
        super::snap::DEFAULT_WINDOW_HEIGHT,
    ));
    Rect::new(
        position.x,
        position.y,
        size.width.max(1),
        size.height.max(1),
    )
}

/// Screen of the window.
///
/// The probe point is the center of the window bottom edge. For a parked
/// window this point is inside the visible band at the screen top edge.
fn screen_for(window: &WebviewWindow) -> ScreenRect {
    let rect = current_rect(window);
    let probe_y = rect.bottom().saturating_sub(PARKED_BAND_PX as i32 / 2);
    screen_at(window, rect.center_x(), probe_y)
}

/// Screen that holds the point `x, y`.
fn screen_at(window: &WebviewWindow, x: i32, y: i32) -> ScreenRect {
    monitor_at(window, x, y)
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            ScreenRect::new(position.x, position.y, size.width, size.height)
        })
        .unwrap_or(ScreenRect::new(0, 0, 1920, 1080))
}

fn monitor_at(window: &WebviewWindow, x: i32, y: i32) -> Option<tauri::Monitor> {
    if let Ok(monitors) = window.available_monitors() {
        for monitor in monitors {
            let position = monitor.position();
            let size = monitor.size();
            let rect = ScreenRect::new(position.x, position.y, size.width, size.height);
            if rect.contains(x, y) {
                return Some(monitor);
            }
        }
    }
    window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
}

/// Lock without panicking when another caller panicked while holding the lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_view_exposes_the_geometry_constants() {
        let view = WindowView {
            parked: false,
            unseen_cards: false,
            scale_factor: 1.25,
            parked_band_px: PARKED_BAND_PX,
            snap_threshold_px: SNAP_TOP_THRESHOLD_PX,
        };
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("\"parked_band_px\":8"));
        assert!(json.contains("\"snap_threshold_px\":24"));
        assert!(json.contains("\"unseen_cards\":false"));
    }

    #[test]
    fn a_slide_starts_near_the_old_y_and_ends_exactly_at_the_new_y() {
        let (from, to) = (40, -540);
        let steps = slide_steps();
        assert!(steps >= 2);
        let first = eased_y(from, to, 1, steps);
        assert!(first < from);
        assert!(from - first > 40, "the first step must be fast");
        assert_eq!(eased_y(from, to, steps, steps), to);
        assert_eq!(eased_y(from, to, steps + 5, steps), to);
    }

    #[test]
    fn a_slide_slows_down_near_the_end() {
        let steps = slide_steps();
        let half = eased_y(40, -540, steps / 2, steps);
        let front = (40 - half).abs();
        let back = (half + 540).abs();
        assert!(front > back, "ease-out moved {front} px then {back} px");
    }

    #[test]
    fn a_slide_moves_in_one_direction_only() {
        let steps = slide_steps();
        let mut last = 40;
        for step in 1..=steps {
            let y = eased_y(40, -540, step, steps);
            assert!(y <= last, "step {step} moved back up");
            last = y;
        }
        assert_eq!(last, -540);
    }

    #[test]
    fn the_band_blinks_only_for_cards_that_arrive_while_parked() {
        let mut unseen = Unseen::default();
        // Expanded: the list is on the screen, so nothing is unseen.
        assert!(!unseen.observe(false, 3));
        // Park with the same revision: no new card, no blink.
        assert!(!unseen.observe(true, 3));
        // A card arrives while parked: blink.
        assert!(unseen.observe(true, 4));
        assert!(unseen.observe(true, 4));
        // The user opens the window: the light stops and the card is seen.
        assert!(!unseen.observe(false, 4));
        assert!(!unseen.observe(true, 4));
        // The next card starts the light again.
        assert!(unseen.observe(true, 5));
        assert!(unseen.is_blinking());
    }

    #[test]
    fn a_delete_while_parked_also_blinks() {
        let mut unseen = Unseen::default();
        assert!(!unseen.observe(false, 1));
        assert!(!unseen.observe(true, 1));
        assert!(unseen.observe(true, 2));
    }

    #[test]
    fn a_new_window_state_starts_expanded() {
        let state = TaskboardWindow::new(Arc::new(TaskBoard::new()));
        assert!(!state.is_parked());
        assert_eq!(state.board().card_count(), 0);
    }

    #[test]
    fn the_ipc_handler_is_registered() {
        // The handler closure must be constructible for the default runtime.
        let _handler = ipc_handler();
    }
}
