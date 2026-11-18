//! Ultra-lightweight (<5ms cold start) cross-platform hook bridge CLI.
//! Reads agent hook JSON from stdin, forwards it to the local `agent-ing` daemon
//! over 127.0.0.1 HTTP with a tight 120ms timeout, and always prints a valid
//! passive JSON response on stdout with exit code 0 so agent execution never blocks or fails.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaemonInfo {
    port: u16,
    token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireEnvelope {
    agent: String,
    event_hint: Option<String>,
    client_pid: u32,
    parent_pid: Option<u32>,
    env_hints: HashMap<String, String>,
    payload: serde_json::Value,
}

fn app_home_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".agent-ing"))
}

fn current_parent_pid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(std::os::unix::process::parent_id())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn collect_env_hints() -> HashMap<String, String> {
    let keys = [
        "TERM_PROGRAM",
        "ITERM_SESSION_ID",
        "GHOSTTY_RESOURCES_DIR",
        "WT_SESSION",
        "VSCODE_PID",
        "VSCODE_IPC_HOOK_CLI",
        "CURSOR_TRACE_ID",
        "__CFBundleIdentifier",
        "CODEX_THREAD_ID",
    ];
    let mut map = HashMap::new();
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                map.insert(k.to_string(), v);
            }
        }
    }
    map
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut agent = String::from("unknown");
    let mut event_hint: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--agent" | "-a" => {
                if let Some(val) = args.next() {
                    agent = val;
                }
            }
            "--event" | "-e" => {
                event_hint = args.next();
            }
            _ => {}
        }
    }

    // Read stdin payload up to 512 KiB
    let mut raw_input = String::new();
    let _ = std::io::stdin()
        .take(512 * 1024)
        .read_to_string(&mut raw_input);

    let payload: serde_json::Value = if raw_input.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&raw_input).unwrap_or_else(|_| serde_json::json!({ "raw": raw_input }))
    };

    // Determine safe stdout output per agent contract BEFORE network I/O
    let is_antigravity = matches!(
        agent.to_ascii_lowercase().as_str(),
        "antigravity" | "agy"
    );
    let is_agy_stop = is_antigravity
        && payload
            .get("hookEventName")
            .and_then(|v| v.as_str())
            .or(event_hint.as_deref())
            .is_some_and(|ev| ev.eq_ignore_ascii_case("Stop"));

    // Best-effort forward to local daemon
    if let Some(daemon_path) = app_home_dir().map(|d| d.join("daemon.json")) {
        if let Ok(text) = std::fs::read_to_string(daemon_path) {
            if let Ok(daemon) = serde_json::from_str::<DaemonInfo>(&text) {
                let envelope = WireEnvelope {
                    agent: agent.clone(),
                    event_hint,
                    client_pid: std::process::id(),
                    parent_pid: current_parent_pid(),
                    env_hints: collect_env_hints(),
                    payload,
                };
                if let Ok(body) = serde_json::to_vec(&envelope) {
                    let addr = SocketAddr::from(([127, 0, 0, 1], daemon.port));
                    if let Ok(mut stream) =
                        TcpStream::connect_timeout(&addr, Duration::from_millis(120))
                    {
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(150)));
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
                        let req = format!(
                            "POST /api/hook HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nX-Agent-Ing-Token: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            daemon.port,
                            daemon.token,
                            body.len()
                        );
                        if stream.write_all(req.as_bytes()).is_ok()
                            && stream.write_all(&body).is_ok()
                        {
                            let _ = stream.flush();
                            let mut resp_buf = [0u8; 128];
                            let _ = stream.read(&mut resp_buf);
                        }
                    }
                }
            }
        }
    }

    // Antigravity `Stop` hook expects a `decision` string; any value other than `"continue"`
    // allows the agent loop to finish normally. All other hooks accept `{}`.
    if is_agy_stop {
        println!(r#"{{"decision":"allow"}}"#);
    } else {
        println!("{{}}");
    }
}
