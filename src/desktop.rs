//! Desktop entry point: HTTP receiver thread, event bridge and Tauri window.
//!
//! The task board never reads the network. It receives one parsed event per
//! notification through `TaskBoard::on_event`, and the window pulls a render
//! snapshot from it.

use std::sync::Arc;

use pi_jingle_server::taskboard::window::{attach, ipc_handler, TaskboardWindow, WINDOW_LABEL};
use pi_jingle_server::taskboard::{AgentEvent, EventKind, SessionInput, TaskBoard};
use pi_jingle_server::{
    port_from_args_or_env, serve, AppState, Notification, StoredEvent, EVENTS_PATH, NOTIFY_PATH,
};
use tauri::Manager;
use tokio::sync::broadcast;

/// Start the receiver, the bridge and the window.
pub fn run() {
    let port = port_from_args_or_env();
    let state = AppState::default();
    let board = Arc::new(TaskBoard::new());

    start_server(state.clone(), port);
    spawn_bridge(state.subscribe(), board.clone());

    println!("pi-jingle-server listening on http://0.0.0.0:{port}");
    println!("parsing POST {NOTIFY_PATH}; serving GET {EVENTS_PATH}; task board window is open");

    let ui = Arc::new(TaskboardWindow::new(board));

    let app = tauri::Builder::default()
        .manage(ui.clone())
        .invoke_handler(ipc_handler())
        .setup(move |app| {
            let window = app
                .get_webview_window(WINDOW_LABEL)
                .ok_or("window 'taskboard' is missing from tauri.conf.json")?;
            attach(&window, &ui)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the task board window");

    app.run(|_app_handle, _event| {});
}

/// Run the HTTP receiver on its own thread and Tokio runtime. A bind error is
/// logged; the window still opens.
fn start_server(state: AppState, port: u16) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("[server] cannot start the Tokio runtime: {err}");
                return;
            }
        };
        runtime.block_on(async move {
            if let Err(err) = serve(state, port).await {
                eprintln!("[server] {err}");
            }
        });
    });
}

/// Forward every accepted notification to the board.
fn spawn_bridge(mut receiver: broadcast::Receiver<StoredEvent>, board: Arc<TaskBoard>) {
    std::thread::spawn(move || loop {
        match receiver.blocking_recv() {
            Ok(stored) => {
                board.on_event(agent_event(&stored.notification));
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                eprintln!("[taskboard] bridge lagged; {skipped} events were dropped");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    });
}

/// Convert one receiver notification into a board event.
///
/// The receiver writes the literal text `Unknown` for absent fields, so those
/// values become `None` here. The UI then shows `—` instead of `Unknown`.
fn agent_event(note: &Notification) -> AgentEvent {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_jingle_server::SessionData;

    #[test]
    fn a_receiver_notification_becomes_a_board_event() {
        let note = Notification {
            session_data: SessionData {
                session_id: "abc".to_string(),
                persisted: "true".to_string(),
                created_at: "2025-09-12T10:30:00.000Z".to_string(),
                version: "3".to_string(),
                cwd: "/home/user/project".to_string(),
                session_name: "Refactor auth module".to_string(),
            },
            event: "agent_settled".to_string(),
            timestamp: 1_757_626_496_789,
        };

        let event = agent_event(&note);
        assert_eq!(event.kind, EventKind::AgentSettled);
        assert_eq!(event.timestamp, Some(1_757_626_496_789));
        assert_eq!(event.session.session_id.as_deref(), Some("abc"));
        assert_eq!(event.session.persisted, Some(true));
        assert_eq!(event.session.version, Some(3));
    }

    #[test]
    fn unknown_placeholders_become_absent() {
        let note = Notification {
            session_data: SessionData::default(),
            event: "agent_start".to_string(),
            timestamp: 1,
        };
        let event = agent_event(&note);
        assert_eq!(event.session, SessionInput::default());
    }
}
