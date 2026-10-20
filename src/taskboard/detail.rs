//! Placement of the card detail panel.
//!
//! The panel is a window of its own, so it can sit outside the task board. It
//! opens to the left or to the right of the task board window, aligned with the
//! card row that the user points at.
//!
//! All values are physical pixels.

use super::snap::{Rect, ScreenRect};

/// Smallest gap between the panel and the task board window.
pub const DETAIL_GAP_PX: i32 = 8;

/// Smallest distance between the panel and a screen edge.
pub const DETAIL_MARGIN_PX: i32 = 8;

/// Where to put the detail panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetailPlacement {
    /// Left edge of the panel, client coordinates.
    pub x: i32,
    /// Top edge of the panel, client coordinates.
    pub y: i32,
    /// `true` when the panel sits left of the task board window.
    pub on_left: bool,
}

/// Place the panel next to `window`, aligned with `row_top`.
///
/// The panel goes to a side that has enough space for it. When both sides have
/// enough space, the panel goes to the left. When neither side has enough
/// space, the panel goes to the side with more space, and the result is
/// clamped so the panel stays inside the screen.
pub fn place_detail(
    window: Rect,
    screen: ScreenRect,
    row_top: i32,
    panel_width: u32,
    panel_height: u32,
) -> DetailPlacement {
    let width = panel_width as i32;
    let height = panel_height as i32;
    let need = width.saturating_add(DETAIL_GAP_PX);
    let space_left = window.x.saturating_sub(screen.x);
    let space_right = screen
        .right()
        .saturating_sub(window.x.saturating_add(window.width as i32));

    let fits_left = space_left >= need;
    let fits_right = space_right >= need;
    let on_left = match (fits_left, fits_right) {
        // Both sides have space: use the left side.
        (true, true) => true,
        (true, false) => true,
        (false, true) => false,
        // No side has space: use the side with more space.
        (false, false) => space_left >= space_right,
    };

    let x = if on_left {
        window.x.saturating_sub(width).saturating_sub(DETAIL_GAP_PX)
    } else {
        window
            .x
            .saturating_add(window.width as i32)
            .saturating_add(DETAIL_GAP_PX)
    };

    let margin = DETAIL_MARGIN_PX;
    let min_x = screen.x.saturating_add(margin);
    let min_y = screen.y.saturating_add(margin);
    let max_x = screen
        .right()
        .saturating_sub(width)
        .saturating_sub(margin)
        .max(min_x);
    let max_y = screen
        .bottom()
        .saturating_sub(height)
        .saturating_sub(margin)
        .max(min_y);

    DetailPlacement {
        x: x.clamp(min_x, max_x),
        y: row_top.clamp(min_y, max_y),
        on_left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1920 x 1080 screen at the origin.
    fn screen() -> ScreenRect {
        ScreenRect::new(0, 0, 1920, 1080)
    }

    /// A 440 x 580 task board window.
    fn window(x: i32) -> Rect {
        Rect::new(x, 40, 440, 580)
    }

    const PANEL: (u32, u32) = (344, 300);

    #[test]
    fn both_sides_have_space_so_the_panel_goes_left() {
        let place = place_detail(window(700), screen(), 120, PANEL.0, PANEL.1);
        assert!(place.on_left);
        assert_eq!(place.x, 700 - 344 - DETAIL_GAP_PX);
        assert_eq!(place.y, 120);
    }

    #[test]
    fn the_panel_goes_right_when_only_the_right_side_has_space() {
        // The window starts at x = 100: 100 px on the left is too small.
        let place = place_detail(window(100), screen(), 120, PANEL.0, PANEL.1);
        assert!(!place.on_left);
        assert_eq!(place.x, 100 + 440 + DETAIL_GAP_PX);
    }

    #[test]
    fn the_panel_goes_left_when_only_the_left_side_has_space() {
        // The window ends at the right edge: the right side has no space.
        let place = place_detail(window(1472), screen(), 120, PANEL.0, PANEL.1);
        assert!(place.on_left);
        assert_eq!(place.x, 1472 - 344 - DETAIL_GAP_PX);
    }

    #[test]
    fn without_space_the_panel_uses_the_larger_side_and_stays_on_screen() {
        // 700 px screen, window at 100: left 100, right 160, panel 344 + 8.
        let small = ScreenRect::new(0, 0, 700, 400);
        let place = place_detail(window(100), small, 120, PANEL.0, PANEL.1);
        assert!(!place.on_left);
        assert_eq!(place.x, 700 - 344 - DETAIL_MARGIN_PX);
        assert!(place.x >= DETAIL_MARGIN_PX);
    }

    #[test]
    fn the_panel_stays_inside_the_screen_vertically() {
        let top = place_detail(window(700), screen(), -50, PANEL.0, PANEL.1);
        assert_eq!(top.y, DETAIL_MARGIN_PX);

        let bottom = place_detail(window(700), screen(), 2_000, PANEL.0, PANEL.1);
        assert_eq!(bottom.y, 1080 - 300 - DETAIL_MARGIN_PX);
    }

    #[test]
    fn a_secondary_screen_keeps_the_panel_in_its_own_bounds() {
        // A screen right of the primary one, starting at x = 1920.
        let second = ScreenRect::new(1920, 0, 1920, 1080);
        // 380 px of space on the left of the window: enough for the panel.
        let place = place_detail(window(2300), second, 200, PANEL.0, PANEL.1);
        assert!(place.on_left);
        assert_eq!(place.x, 2300 - 344 - DETAIL_GAP_PX);
        assert!(place.x >= second.x + DETAIL_MARGIN_PX);
    }

    #[test]
    fn a_wide_panel_is_clamped_to_the_screen() {
        let place = place_detail(window(700), screen(), 120, 1_900, 300);
        assert_eq!(place.x, 1920 - 1_900 - DETAIL_MARGIN_PX);
        assert!(place.x >= DETAIL_MARGIN_PX);
    }
}
