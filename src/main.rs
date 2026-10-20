//! pi-jingle-server entry point.
//!
//! Default build (`desktop` feature): the HTTP receiver plus the Tauri task
//! board window.
//!
//! Headless build (`cargo build --no-default-features`): the HTTP receiver
//! only, with no Tauri dependency.

// Windows must not open a console window for the task board program. The
// attribute keeps the parent console when the program starts from a terminal,
// so logs stay visible there, and gives no console when it starts from Explorer
// or Start-Process. The headless receiver keeps its console.
#![cfg_attr(all(windows, feature = "desktop"), windows_subsystem = "windows")]

#[cfg(feature = "desktop")]
mod desktop;

#[cfg(feature = "desktop")]
fn main() {
    desktop::run();
}

#[cfg(not(feature = "desktop"))]
#[tokio::main]
async fn main() {
    let port = pi_jingle_server::port_from_args_or_env();
    let state = pi_jingle_server::AppState::default();

    println!("pi-jingle-server listening on http://0.0.0.0:{port}");
    println!(
        "parsing POST {}; serving GET {}; dumping all other requests. Ctrl+C to stop.",
        pi_jingle_server::NOTIFY_PATH,
        pi_jingle_server::EVENTS_PATH
    );

    if let Err(err) = pi_jingle_server::serve(state, port).await {
        eprintln!("server error: {err}");
        std::process::exit(1);
    }
}
