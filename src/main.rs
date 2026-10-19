// pi-jingle-server (MVP)
//
// Receives every HTTP request and prints it to the command line:
//   timestamp (epoch ms), peer address, method, path + query, headers, body.
//
// This is the prototype receiver for pi-jingle's `sounds-remote` notifications
// (see ../pi-jingle/docs/remote-notifications.md), which POST a JSON payload of
// { session_id, event, timestamp } to the fixed path /api/jingle/notify.
//
// Usage:
//   pi-jingle-server [PORT]          (default 8080, or JINGLE_PORT env var)

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, Request},
    response::Response,
    Router,
};
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;

const DEFAULT_PORT: u16 = 8080;
const MAX_BODY: usize = 1024 * 1024; // 1 MiB cap; larger bodies are truncated

#[tokio::main]
async fn main() {
    let port = port_from_args_or_env();
    let addr = format!("0.0.0.0:{port}");

    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };

    println!("pi-jingle-server listening on http://{addr}");
    println!("printing all received requests. Ctrl+C to stop.");

    // Single fallback handler: every method, every path, is printed.
    let app = Router::new().fallback(handle_request);

    if let Err(e) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}

fn port_from_args_or_env() -> u16 {
    std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .or_else(|| std::env::var("JINGLE_PORT").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(DEFAULT_PORT)
}

async fn handle_request(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response<Body> {
    let (parts, body) = req.into_parts();

    // Collect the full body (capped) before printing so the dump is complete.
    let body: Bytes = axum::body::to_bytes(body, MAX_BODY)
        .await
        .unwrap_or_default();

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let mut out = String::new();
    out.push_str(&"-".repeat(60));
    out.push('\n');
    out.push_str(&format!("[time]    {ts} (epoch ms)\n"));
    out.push_str(&format!("[from]    {peer}\n"));
    out.push_str(&format!("[request] {} {}\n", parts.method, parts.uri));
    out.push_str("[headers]\n");
    for (name, value) in parts.headers.iter() {
        out.push_str(&format!(
            "  {}: {}\n",
            name,
            value.to_str().unwrap_or("<binary>")
        ));
    }
    if body.is_empty() {
        out.push_str("[body]    (empty)\n");
    } else {
        out.push_str(&format!("[body]    {}\n", String::from_utf8_lossy(&body)));
    }
    println!("{out}");

    // Minimal ack so clients (including pi-jingle) see a clean 200.
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from("{\"ok\":true}"))
        .unwrap()
}
