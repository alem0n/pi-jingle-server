//! Screen-edge snap geometry: move the window out of the screen and back.
//!
//! This module is pure math. It does not call Tauri, so the behaviour can be
//! unit tested without a window:
//!
//! * A window whose top edge enters the snap threshold at the top of a screen
//!   is moved up until only a thin band at its bottom edge stays visible. The
//!   window keeps its size. It is not resized.
//! * Expanding moves the whole window back into the screen: the same x, the
//!   same size and the top edge at the screen top.
//! * A repeated park and expand produces the same rectangles.
//! * A screen change re-clamps the parked band and the expanded window.
//!
//! All constants are physical pixels (device pixels), not logical pixels.

/// Distance from the screen top edge that triggers the park, in physical px.
///
/// The window top must be at or above `screen.y + SNAP_TOP_THRESHOLD_PX`.
pub const SNAP_TOP_THRESHOLD_PX: i32 = 24;

/// Height of the visible band of a parked window, in physical px.
///
/// The band is the bottom edge of the window. Its width is the window width.
pub const PARKED_BAND_PX: u32 = 8;

/// Distance between the initial window and each screen edge, in physical px.
pub const EDGE_MARGIN_PX: i32 = 8;

/// Distance between the screen top edge and the initial window position, in
/// physical px.
pub const INITIAL_TOP_INSET_PX: i32 = 40;

/// Default expanded width, in physical px.
pub const DEFAULT_WINDOW_WIDTH: u32 = 440;

/// Default expanded height, in physical px.
pub const DEFAULT_WINDOW_HEIGHT: u32 = 580;

/// Smallest expanded width, in physical px.
pub const MIN_WINDOW_WIDTH_PX: u32 = 360;

/// Smallest expanded height, in physical px.
pub const MIN_WINDOW_HEIGHT_PX: u32 = 240;

/// A screen (monitor) in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl ScreenRect {
    /// Create a screen rectangle.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// X coordinate of the right edge.
    pub fn right(self) -> i32 {
        self.x.saturating_add(self.width as i32)
    }

    /// Y coordinate of the bottom edge.
    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }

    /// `true` when the point is inside this screen.
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// A window rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl Rect {
    /// Create a window rectangle.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Y coordinate of the top edge.
    pub const fn top(self) -> i32 {
        self.y
    }

    /// Y coordinate of the bottom edge.
    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }

    /// X coordinate of the horizontal center.
    pub fn center_x(self) -> i32 {
        self.x.saturating_add(self.width as i32 / 2)
    }

    /// `true` when the point is inside this rectangle.
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x.saturating_add(self.width as i32)
            && y >= self.y
            && y < self.y.saturating_add(self.height as i32)
    }
}

/// Window phase of the snap behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapPhase {
    /// The whole window is inside the screen.
    Expanded,
    /// The window is above the screen top edge. Only the bottom band is
    /// visible.
    Parked,
}

/// Snap state machine.
///
/// One instance lives as long as the window. All methods return the rectangle
/// the caller must apply, or `None` when the window must not move.
///
/// A park keeps the window size and the x position. The window moves up until
/// the bottom band of [`PARKED_BAND_PX`] px stays at the screen top edge. An
/// expand moves the same window down to the screen top. Nothing is resized.
#[derive(Clone, Debug)]
pub struct SnapController {
    phase: SnapPhase,
    restore: Option<Rect>,
    drag_start_top: Option<i32>,
}

impl SnapController {
    /// Create an expanded controller with a known window rectangle.
    pub fn new(restore: Rect) -> Self {
        Self {
            phase: SnapPhase::Expanded,
            restore: Some(restore),
            drag_start_top: None,
        }
    }

    /// Current phase.
    pub fn phase(&self) -> SnapPhase {
        self.phase
    }

    /// `true` when the window is above the screen top edge.
    pub fn is_parked(&self) -> bool {
        self.phase == SnapPhase::Parked
    }

    /// Last known window rectangle, used for the next expand.
    pub fn restore_rect(&self) -> Option<Rect> {
        self.restore
    }

    /// Remember the window rectangle at the start of a drag.
    pub fn begin_drag(&mut self, rect: Rect) {
        if self.phase == SnapPhase::Expanded {
            self.restore = Some(rect);
        }
        self.drag_start_top = Some(rect.top());
    }

    /// Remember the window rectangle at the end of a drag.
    pub fn end_drag(&mut self, rect: Rect) {
        if self.phase == SnapPhase::Expanded {
            self.restore = Some(rect);
        }
        self.drag_start_top = None;
    }

    /// Evaluate one drag step.
    ///
    /// Returns `Some(parked)` when the window top entered the snap threshold
    /// and the window must move above the screen now. A parked window ignores
    /// further drag steps, so the state cannot flicker during one drag.
    ///
    /// A window that starts the drag at the top edge (a peek panel) does not
    /// park. It is already at the edge and the user wants to move it away.
    pub fn drag_step(&mut self, target: Rect, screen: ScreenRect) -> Option<Rect> {
        if self.phase == SnapPhase::Parked {
            return None;
        }
        let threshold = screen.y.saturating_add(SNAP_TOP_THRESHOLD_PX);
        if target.top() > threshold {
            return None;
        }
        if self.drag_start_top.is_some_and(|top| top <= threshold) {
            return None;
        }
        self.park(screen, target)
    }

    /// Move the window above the screen top edge.
    ///
    /// The window keeps its size and its x position. The bottom band stays
    /// visible at the screen top edge.
    pub fn park(&mut self, screen: ScreenRect, from: Rect) -> Option<Rect> {
        if self.phase == SnapPhase::Parked {
            return None;
        }
        self.phase = SnapPhase::Parked;
        let expanded = expanded_rect(screen, from);
        self.restore = Some(expanded);
        Some(parked_rect(screen, expanded))
    }

    /// Move the whole window back into the screen.
    ///
    /// The window returns to the same x position and the same size, with the
    /// top edge at the screen top.
    pub fn expand(&mut self, screen: ScreenRect) -> Option<Rect> {
        if self.phase == SnapPhase::Expanded {
            return None;
        }
        self.phase = SnapPhase::Expanded;

        let base = self.restore.unwrap_or_else(|| default_window_rect(screen));
        let rect = expanded_rect(screen, base);
        self.restore = Some(rect);
        Some(rect)
    }

    /// Keep the window correct after a screen change or a resize.
    pub fn reconcile(&mut self, screen: ScreenRect, current: Rect) -> Option<Rect> {
        match self.phase {
            SnapPhase::Parked => {
                let expanded = expanded_rect(screen, current);
                let wanted = parked_rect(screen, expanded);
                if wanted == current {
                    None
                } else {
                    Some(wanted)
                }
            }
            SnapPhase::Expanded => {
                // Do not force the top edge and do not push a top-flush
                // window down. Only keep the window inside the screen.
                let wanted = clamp_into(screen, current, 0);
                if wanted == current {
                    None
                } else {
                    self.restore = Some(wanted);
                    Some(wanted)
                }
            }
        }
    }

    /// The visible band of a parked window: the bottom edge of the window at
    /// the screen top edge.
    pub fn band_rect(screen: ScreenRect, parked: Rect) -> Rect {
        let height = PARKED_BAND_PX.min(parked.height.max(1));
        Rect {
            x: parked.x,
            y: screen.y,
            width: parked.width,
            height,
        }
    }
}

/// Default expanded window: top-right corner of the screen.
pub fn default_window_rect(screen: ScreenRect) -> Rect {
    let width = DEFAULT_WINDOW_WIDTH.min(screen.width.max(1));
    let height = DEFAULT_WINDOW_HEIGHT.min(screen.height.max(1));
    let rect = Rect {
        x: screen
            .right()
            .saturating_sub(EDGE_MARGIN_PX)
            .saturating_sub(width as i32),
        y: screen.y.saturating_add(INITIAL_TOP_INSET_PX),
        width,
        height,
    };
    clamp_into(screen, rect, EDGE_MARGIN_PX)
}

/// Clamp a rectangle to the expanded position: top edge at the screen top,
/// inside the screen with the edge margin.
fn expanded_rect(screen: ScreenRect, rect: Rect) -> Rect {
    let wanted = Rect {
        y: screen.y,
        ..rect
    };
    let clamped = clamp_into(screen, wanted, EDGE_MARGIN_PX);
    Rect {
        y: screen.y,
        height: clamped.height.min(screen.height.max(1)),
        ..clamped
    }
}

/// Move an expanded rectangle above the screen top edge. The bottom band stays
/// visible.
fn parked_rect(screen: ScreenRect, expanded: Rect) -> Rect {
    let band = PARKED_BAND_PX.min(expanded.height.max(1));
    Rect {
        y: screen
            .y
            .saturating_sub(expanded.height as i32 - band as i32),
        ..expanded
    }
}

/// Move and shrink a rectangle until it fits inside the screen.
fn clamp_into(screen: ScreenRect, rect: Rect, margin: i32) -> Rect {
    let margin = margin.max(0);
    // A screen that is smaller than two margins cannot keep the margin.
    let margin_x = if screen.width as i32 > margin * 2 {
        margin
    } else {
        0
    };
    let margin_y = if screen.height as i32 > margin * 2 {
        margin
    } else {
        0
    };

    let max_width = (screen.width as i32 - margin_x * 2).max(1) as u32;
    let max_height = (screen.height as i32 - margin_y * 2).max(1) as u32;
    let width = rect.width.clamp(1, max_width);
    let height = rect.height.clamp(1, max_height);

    let min_x = screen.x.saturating_add(margin_x);
    let max_x = screen
        .right()
        .saturating_sub(margin_x)
        .saturating_sub(width as i32)
        .max(min_x);
    let x = rect.x.clamp(min_x, max_x);

    let min_y = screen.y.saturating_add(margin_y);
    let max_y = screen
        .bottom()
        .saturating_sub(margin_y)
        .saturating_sub(height as i32)
        .max(min_y);
    let y = rect.y.clamp(min_y, max_y);

    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> ScreenRect {
        ScreenRect::new(0, 0, 1920, 1080)
    }

    fn window() -> Rect {
        Rect::new(200, 300, 440, 580)
    }

    #[test]
    fn park_moves_the_window_above_the_top_edge() {
        let mut snap = SnapController::new(window());
        let parked = snap.park(screen(), window()).unwrap();

        assert_eq!(snap.phase(), SnapPhase::Parked);
        assert_eq!(parked.x, window().x);
        assert_eq!(parked.width, window().width);
        assert_eq!(parked.height, window().height);
        // Only the bottom band stays visible.
        assert_eq!(parked.y, -580 + PARKED_BAND_PX as i32);
        assert_eq!(parked.bottom(), screen().y + PARKED_BAND_PX as i32);
    }

    #[test]
    fn expand_moves_the_whole_window_back_into_the_screen() {
        let mut snap = SnapController::new(window());
        let parked = snap.park(screen(), window()).unwrap();
        let expanded = snap.expand(screen()).unwrap();

        assert_eq!(snap.phase(), SnapPhase::Expanded);
        assert_eq!(expanded.y, screen().y);
        assert_eq!(expanded.x, parked.x);
        assert_eq!(expanded.width, parked.width);
        assert_eq!(expanded.height, parked.height);
        assert_eq!(snap.expand(screen()), None, "expand must be idempotent");
    }

    #[test]
    fn park_keeps_the_window_size_and_does_not_resize_it() {
        let mut snap = SnapController::new(window());
        let parked = snap.park(screen(), window()).unwrap();
        assert_eq!(parked.width, window().width);
        assert_eq!(parked.height, window().height);
    }

    #[test]
    fn snap_triggers_at_the_threshold_only() {
        let mut snap = SnapController::new(window());
        let mut target = window();
        snap.begin_drag(target);

        target.y = SNAP_TOP_THRESHOLD_PX + 1;
        assert_eq!(snap.drag_step(target, screen()), None);
        assert_eq!(snap.phase(), SnapPhase::Expanded);

        target.y = SNAP_TOP_THRESHOLD_PX;
        let parked = snap.drag_step(target, screen()).unwrap();
        assert_eq!(snap.phase(), SnapPhase::Parked);
        assert_eq!(parked.x, target.x);
        assert_eq!(parked.bottom(), screen().y + PARKED_BAND_PX as i32);
    }

    #[test]
    fn a_parked_window_ignores_more_drag_steps() {
        let mut snap = SnapController::new(window());
        let mut target = window();
        target.y = 0;
        assert!(snap.drag_step(target, screen()).is_some());
        assert_eq!(snap.drag_step(target, screen()), None);
        target.y = 500;
        assert_eq!(snap.drag_step(target, screen()), None);
        assert!(snap.is_parked());
    }

    #[test]
    fn a_drag_that_starts_at_the_top_does_not_park() {
        let mut snap = SnapController::new(window());
        let peek = Rect::new(200, 0, 440, 580);
        snap.begin_drag(peek);

        assert_eq!(snap.drag_step(peek, screen()), None);
        assert_eq!(snap.drag_step(Rect::new(200, 5, 440, 580), screen()), None);
        assert_eq!(snap.phase(), SnapPhase::Expanded);

        snap.end_drag(Rect::new(200, 120, 440, 580));
        // The next drag starts below the threshold and can park again.
        snap.begin_drag(Rect::new(200, 120, 440, 580));
        assert!(snap
            .drag_step(Rect::new(200, 0, 440, 580), screen())
            .is_some());
    }

    #[test]
    fn the_visible_band_is_the_bottom_edge_of_the_window() {
        let parked = parked_rect(screen(), expanded_rect(screen(), window()));
        let band = SnapController::band_rect(screen(), parked);

        assert_eq!(band.y, screen().y);
        assert_eq!(band.height, PARKED_BAND_PX);
        assert_eq!(band.width, window().width);
        assert_eq!(band.x, parked.x);
        assert_eq!(band.bottom(), parked.bottom());
        assert!(parked.contains(band.center_x(), band.y + 1));
    }

    #[test]
    fn park_keeps_the_window_inside_the_screen_horizontally() {
        let mut snap = SnapController::new(window());
        let parked = snap.park(screen(), Rect::new(-500, 300, 440, 580)).unwrap();
        assert_eq!(parked.x, screen().x + EDGE_MARGIN_PX);

        let mut snap = SnapController::new(window());
        let parked = snap
            .park(screen(), Rect::new(90_000, 300, 440, 580))
            .unwrap();
        assert_eq!(
            parked.x,
            screen().right() - EDGE_MARGIN_PX - parked.width as i32
        );
    }

    #[test]
    fn a_screen_change_keeps_the_band_at_the_new_top_edge() {
        let mut snap = SnapController::new(window());
        let parked = snap.park(screen(), window()).unwrap();
        let smaller = ScreenRect::new(0, 0, 500, 400);

        let moved = snap.reconcile(smaller, parked).unwrap();
        assert_eq!(moved.bottom(), smaller.y + PARKED_BAND_PX as i32);
        assert_eq!(moved.height, 384);
        assert!(moved.height <= window().height);
        assert!(moved.x >= smaller.x);
        assert!(moved.x + moved.width as i32 <= smaller.right());
        assert_eq!(snap.reconcile(smaller, moved), None);
        assert!(snap.is_parked());
    }

    #[test]
    fn a_screen_change_clamps_an_expanded_window() {
        let mut snap = SnapController::new(window());
        let current = Rect::new(1400, 800, 440, 580);
        let smaller = ScreenRect::new(0, 0, 1024, 768);
        let moved = snap.reconcile(smaller, current).unwrap();
        assert!(moved.x + moved.width as i32 <= smaller.right());
        assert!(moved.y + moved.height as i32 <= smaller.bottom());
        assert_eq!(snap.reconcile(smaller, moved), None);
        assert_eq!(snap.restore_rect(), Some(moved));
    }

    #[test]
    fn a_top_flush_window_is_not_pushed_down() {
        let mut snap = SnapController::new(window());
        let current = Rect::new(300, 0, 440, 580);
        assert_eq!(snap.reconcile(screen(), current), None);
    }

    #[test]
    fn reconcile_keeps_a_window_inside_the_screen_where_it_is() {
        let mut snap = SnapController::new(Rect::new(300, 300, 440, 580));
        let current = Rect::new(300, 300, 440, 580);
        assert_eq!(snap.reconcile(screen(), current), None);
        assert_eq!(snap.restore_rect(), Some(current));
    }

    #[test]
    fn repeated_park_and_expand_is_stable() {
        let mut snap = SnapController::new(window());
        let mut seen = Vec::new();
        for _ in 0..3 {
            let from = Rect::new(300, 400, 500, 600);
            snap.begin_drag(from);
            seen.push(snap.park(screen(), from).unwrap());
            seen.push(snap.expand(screen()).unwrap());
        }
        assert_eq!(seen[0], seen[2]);
        assert_eq!(seen[1], seen[3]);
        assert_eq!(seen[1], Rect::new(300, screen().y, 500, 600));
        assert_eq!(seen[0].bottom(), screen().y + PARKED_BAND_PX as i32);
    }

    #[test]
    fn the_default_rect_is_inside_the_screen() {
        let rect = default_window_rect(screen());
        assert!(rect.x >= screen().x);
        assert!(rect.y >= screen().y + INITIAL_TOP_INSET_PX);
        assert!(rect.x + rect.width as i32 <= screen().right());
        assert!(rect.y + rect.height as i32 <= screen().bottom());
    }

    #[test]
    fn tiny_screens_do_not_produce_empty_rectangles() {
        let tiny = ScreenRect::new(0, 0, 200, 6);
        let mut snap = SnapController::new(Rect::new(0, 0, 440, 580));
        let parked = snap.park(tiny, Rect::new(0, 0, 440, 580)).unwrap();
        assert!(parked.width >= 1);
        assert!(parked.height >= 1);
        assert_eq!(parked.height, 6);
        assert_eq!(parked.bottom(), PARKED_BAND_PX.min(6) as i32);

        let expanded = snap.expand(tiny).unwrap();
        assert!(expanded.width >= 1);
        assert!(expanded.height >= 1);
        assert_eq!(expanded.y, tiny.y);
        assert!(expanded.y + expanded.height as i32 <= tiny.bottom());
    }
}
