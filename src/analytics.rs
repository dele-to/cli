use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Map, Value};
use uuid::Uuid;

/// Same Databuddy website as the dele.to web app (`app/layout.tsx`).
const CLIENT_ID: &str = "CLIENT_ID_HERE";
const TRACK_URL: &str = "https://basket.databuddy.cc/track";
const SDK_NAME: &str = "deleto-cli";

static SESSION_ID: OnceLock<String> = OnceLock::new();
static ANON_ID: OnceLock<String> = OnceLock::new();
static WORKERS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

pub fn track(name: &str, properties: Map<String, Value>) {
    if opted_out() {
        return;
    }
    let name = name.to_string();
    let handle = thread::Builder::new()
        .name("deleto-analytics".into())
        .spawn(move || send(name, properties))
        .ok();
    if let Some(handle) = handle {
        if let Ok(mut workers) = WORKERS.lock() {
            workers.push(handle);
        }
    }
}

pub fn flush() {
    let workers = WORKERS.lock().ok().map(|mut g| std::mem::take(&mut *g));
    if let Some(workers) = workers {
        for handle in workers {
            let _ = handle.join();
        }
    }
}

pub fn props(pairs: &[(&str, Value)]) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("os".into(), Value::String(std::env::consts::OS.to_string()));
    map.insert(
        "cli_version".into(),
        Value::String(env!("CARGO_PKG_VERSION").into()),
    );
    for (key, value) in pairs {
        map.insert((*key).into(), value.clone());
    }
    map
}

pub fn error_reason(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("404") || lower.contains("not found") {
        "not_found"
    } else if lower.contains("401") || lower.contains("unauthorized") {
        "unauthorized"
    } else if lower.contains("429") || lower.contains("rate") {
        "rate_limited"
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "timeout"
    } else if lower.contains("connection") || lower.contains("network") || lower.contains("dns") {
        "network"
    } else if lower.contains("invalid") {
        "invalid_input"
    } else if lower.contains("api") {
        "api_error"
    } else {
        "other"
    }
}

fn opted_out() -> bool {
    env_flag("DELETO_NO_ANALYTICS")
        || env_flag("DATABUDDY_DISABLED")
        || env_flag("DO_NOT_TRACK")
        || env::var("DO_NOT_TRACK").ok().as_deref() == Some("1")
}

fn env_flag(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn send(name: String, properties: Map<String, Value>) {
    let payload = json!([{
        "name": name,
        "timestamp": unix_ms(),
        "properties": properties,
        "anonymousId": anonymous_id(),
        "sessionId": session_id(),
        "websiteId": CLIENT_ID,
        "source": "cli",
    }]);
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(1500))
        .user_agent(concat!("deleto-cli/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    let url = format!("{TRACK_URL}?website_id={CLIENT_ID}");
    let _ = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("databuddy-client-id", CLIENT_ID)
        .header("databuddy-sdk-name", SDK_NAME)
        .header("databuddy-sdk-version", env!("CARGO_PKG_VERSION"))
        .json(&payload)
        .send();
}

fn session_id() -> &'static str {
    SESSION_ID.get_or_init(|| format!("sess_{}", Uuid::new_v4()))
}

fn anonymous_id() -> &'static str {
    ANON_ID.get_or_init(load_or_create_anonymous_id)
}

fn load_or_create_anonymous_id() -> String {
    if let Some(path) = anonymous_id_path() {
        if let Ok(existing) = fs::read_to_string(&path) {
            let trimmed = existing.trim();
            if trimmed.starts_with("anon_") && trimmed.len() > 10 && trimmed.len() < 80 {
                return trimmed.to_string();
            }
        }
        let id = format!("anon_{}", Uuid::new_v4());
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, format!("{id}\n"));
        return id;
    }
    format!("anon_{}", Uuid::new_v4())
}

fn anonymous_id_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".deleto").join("anonymous-id"))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_never_includes_secrets() {
        let properties = props(&[
            ("source", json!("cli")),
            ("max_views", json!(1)),
            ("content_length", json!(12)),
        ]);
        let encoded = serde_json::to_string(&properties).unwrap();
        assert!(!encoded.contains("password"));
        assert!(!encoded.contains("dlt_"));
        assert!(!encoded.contains("http"));
        assert_eq!(properties["source"], "cli");
        assert_eq!(properties["content_length"], 12);
    }

    #[test]
    fn error_reason_is_coarse() {
        assert_eq!(error_reason("failed to create share: unexpected API response (404) from https://dele.to/api/v1/shares"), "not_found");
        assert_eq!(error_reason("invalid v1 share fragment"), "invalid_input");
        assert_eq!(error_reason("connection refused"), "network");
    }
}
