//! System tray icon of the task board program.
//!
//! The task board window has no taskbar entry (`skipTaskbar` in
//! `tauri.conf.json`). The tray icon is the only entry point in the taskbar
//! area. It shows or hides the window, and it can quit the program.
//!
//! The icon is embedded in the program, so no icon file is necessary at run
//! time. The handle returned by `TrayIconBuilder::build` is reference counted:
//! the icon goes away when the last handle is dropped. The program keeps one
//! handle in its state for the whole run.

use std::sync::Arc;

use pi_jingle_server::taskboard::window::{quit, set_board_visible, TaskboardWindow, WINDOW_LABEL};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

/// Id of the tray icon.
pub const TRAY_ID: &str = "pi-jingle-tray";

/// Menu item that shows or hides the task board window.
pub const MENU_TOGGLE_ID: &str = "tray-toggle-board";

/// Menu item that quits the program.
pub const MENU_QUIT_ID: &str = "tray-quit";

/// Tooltip of the tray icon.
pub const TRAY_TOOLTIP: &str = "Pi Jingle - agent tasks";

/// PNG bytes of the tray icon.
const TRAY_ICON_PNG: &[u8] = include_bytes!("../../icons/icon.png");

/// Build the tray icon. Call this from the Tauri setup hook.
pub fn create(app: &tauri::App, ui: Arc<TaskboardWindow>) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(
        app,
        MENU_TOGGLE_ID,
        "Show or hide the task board",
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT_ID, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &quit_item])?;

    let menu_ui = ui.clone();

    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(TRAY_ICON_PNG)?)
        .tooltip(TRAY_TOOLTIP)
        .menu(&menu)
        // A left click toggles the window. Only a right click opens the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            MENU_TOGGLE_ID => toggle_board(app, &menu_ui),
            MENU_QUIT_ID => quit(app),
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| {
            let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            else {
                return;
            };
            toggle_board(tray.app_handle(), &ui);
        })
        .build(app)?;

    // The icon disappears when the last handle is dropped. Keep one handle in
    // the program state, so the icon stays for the whole run.
    app.manage(tray);
    Ok(())
}

/// Show or hide the task board. One tray click and the menu call this.
fn toggle_board(app: &AppHandle, ui: &TaskboardWindow) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    set_board_visible(app, ui, toggled_visibility(visible));
}

/// Window visibility after one tray click. A visible window becomes hidden.
pub fn toggled_visibility(visible: bool) -> bool {
    !visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tray_click_hides_a_visible_window_and_shows_a_hidden_one() {
        assert!(!toggled_visibility(true));
        assert!(toggled_visibility(false));
    }

    #[test]
    fn the_embedded_tray_icon_is_a_png_image() {
        let icon = Image::from_bytes(TRAY_ICON_PNG).expect("the icon bytes are a PNG image");
        assert!(icon.width() > 0 && icon.height() > 0);
    }

    #[test]
    fn the_menu_ids_are_distinct() {
        assert_ne!(MENU_TOGGLE_ID, MENU_QUIT_ID);
    }
}
