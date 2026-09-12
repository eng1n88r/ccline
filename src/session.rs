use serde_json::Value;
use std::env;
use std::fs;
use std::process::Command;
use std::time::Duration;

use crate::util::{file_age, spawn_self};

const SESSION_NAME_TTL: Duration = Duration::from_secs(60);

/// Shares the cache file used by the old session-name.cjs script.
pub(crate) fn session_name(data: &Value) -> Option<String> {
    let sid = data["session_id"].as_str()?;
    if !sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let cache = env::temp_dir().join(format!("cc-session-name-{sid}.txt"));
    let stale = file_age(&cache).is_none_or(|age| age > SESSION_NAME_TTL);
    if stale {
        spawn_self(&["--refresh-session-name", sid]);
    }
    let name = fs::read_to_string(&cache).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub(crate) fn refresh_session_name(sid: Option<&str>) {
    let Some(sid) = sid else { return };
    let cache = env::temp_dir().join(format!("cc-session-name-{sid}.txt"));
    // Touch first so concurrent renders don't spawn duplicate refreshers.
    let _ = fs::write(&cache, fs::read_to_string(&cache).unwrap_or_default());
    let Ok(out) = Command::new("claude").args(["agents", "--json"]).output() else {
        return;
    };
    let Ok(agents) = serde_json::from_slice::<Value>(&out.stdout) else {
        return;
    };
    let name = agents
        .as_array()
        .and_then(|a| a.iter().find(|x| x["sessionId"].as_str() == Some(sid)))
        .and_then(|x| x["name"].as_str())
        .unwrap_or("");
    let _ = fs::write(&cache, name);
}
