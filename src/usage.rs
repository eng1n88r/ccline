use crate::style::{dim, pct_color};
use crate::util::{file_age, home, now_secs, spawn_self};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const USAGE_TTL: Duration = Duration::from_secs(60);
const USAGE_BACKOFF: Duration = Duration::from_secs(300);
const LOCK_MAX_AGE: Duration = Duration::from_secs(60);

pub(crate) fn usage_segments(data: &Value, segs: &mut Vec<String>) {
    let api = load_usage_cache();

    let session =
        bucket_pct(&data["rate_limits"]["five_hour"]).or_else(|| bucket_pct(&api["five_hour"]));
    let weekly =
        bucket_pct(&data["rate_limits"]["seven_day"]).or_else(|| bucket_pct(&api["seven_day"]));
    let fable = scoped_limit_pct(&api, "fable");

    for (label, pct) in [("5h", session), ("wk", weekly), ("Fable", fable)] {
        if let Some(p) = pct {
            segs.push(format!("{} {}{:.0}%\x1b[0m", dim(label), pct_color(p), p));
        }
    }
}

/// Utilization of a rate-limit bucket. A 0 with no reset time is the API's
/// "no data" placeholder, not a real zero — treat it as absent.
fn bucket_pct(b: &Value) -> Option<f64> {
    let p = b["utilization"].as_f64()?;
    (p > 0.0 || !b["resets_at"].is_null()).then_some(p)
}

/// Percent for a `weekly_scoped` limit whose model display name matches.
/// Placeholder limits (percent 0, no reset time) are skipped.
fn scoped_limit_pct(api: &Value, model: &str) -> Option<f64> {
    api["limits"].as_array()?.iter().find_map(|l| {
        let scoped = l["kind"].as_str() == Some("weekly_scoped");
        let name = l["scope"]["model"]["display_name"].as_str().unwrap_or("");
        let placeholder = l["percent"].as_f64() == Some(0.0) && l["resets_at"].is_null();
        (scoped && !placeholder && name.to_lowercase().contains(model))
            .then(|| l["percent"].as_f64())
            .flatten()
    })
}

fn cache_dir() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .or_else(|| env::var_os("LOCALAPPDATA")) // Windows
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"))
        .join("ccline")
}

/// Returns the cached raw usage-API response, kicking off a detached refresh
/// when the cache is stale. Never blocks on the network.
fn load_usage_cache() -> Value {
    let dir = cache_dir();
    let cache = dir.join("usage.json");
    let stale = file_age(&cache).is_none_or(|age| age > USAGE_TTL);
    // The backoff file holds a blocked-until unix timestamp (from Retry-After
    // on a 429, or a default on other failures).
    let backed_off = fs::read_to_string(dir.join("usage.backoff"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .is_some_and(|until| now_secs() < until);
    if stale && !backed_off && acquire_lock(&dir.join("usage.lock")) {
        spawn_self(&["--refresh-usage"]);
    }
    fs::read_to_string(&cache)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

/// Raw credentials JSON: `~/.claude/.credentials.json`, with the macOS
/// Keychain (where Claude Code stores it there) as fallback.
fn read_credentials() -> String {
    if let Ok(s) = fs::read_to_string(home().join(".claude/.credentials.json")) {
        return s;
    }
    if cfg!(target_os = "macos")
        && let Ok(out) = Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout).into_owned();
    }

    String::new()
}

pub(crate) fn refresh_usage() {
    let dir = cache_dir();
    let lock = dir.join("usage.lock");
    let creds: Value = serde_json::from_str(&read_credentials()).unwrap_or(Value::Null);
    let Some(token) = creds["claudeAiOauth"]["accessToken"].as_str() else {
        let _ = fs::remove_file(&lock);
        return;
    };

    // Headers go through a 0600 curl config file, not argv.
    let cfg_path = dir.join("curl.cfg");
    let cfg = format!(
        "url = \"https://api.anthropic.com/api/oauth/usage\"\n\
         header = \"Authorization: Bearer {token}\"\n\
         header = \"anthropic-beta: oauth-2025-04-20\"\n"
    );
    if write_private(&cfg_path, cfg.as_bytes()).is_err() {
        let _ = fs::remove_file(&lock);
        return;
    }

    // -D dumps response headers so a 429's Retry-After can set the backoff.
    let hdr_path = dir.join("usage.hdr");
    let out = Command::new("curl")
        .args(["-s", "-m", "8", "-D"])
        .arg(&hdr_path)
        .arg("-K")
        .arg(&cfg_path)
        .output();
    let _ = fs::remove_file(&cfg_path);
    let headers = fs::read_to_string(&hdr_path).unwrap_or_default();
    let _ = fs::remove_file(&hdr_path);
    let status = http_status(&headers);

    let cache = dir.join("usage.json");
    let mut fetched = false;
    if status == Some(200)
        && let Ok(out) = &out
            && let Ok(new) = serde_json::from_slice::<Value>(&out.stdout) {
                fetched = true;
                if suspicious_drop(&cache, &new) {
                    // Degraded response under rate limiting: keep the good
                    // cache, refresh its mtime so we don't refetch in a loop.
                    touch(&cache);
                } else {
                    let tmp = dir.join("usage.json.tmp");
                    if fs::write(&tmp, &out.stdout).is_ok() {
                        let _ = fs::rename(&tmp, &cache);
                    }
                }
            }
    if fetched {
        let _ = fs::remove_file(dir.join("usage.backoff"));
    } else {
        // On 429 honor Retry-After (clamped to an hour); anything else —
        // outage, bad response, network error — backs off the default.
        let delay = match status {
            Some(429) => retry_after_secs(&headers)
                .unwrap_or(USAGE_BACKOFF.as_secs())
                .clamp(1, 3600),
            _ => USAGE_BACKOFF.as_secs(),
        };
        let _ = fs::write(dir.join("usage.backoff"), (now_secs() + delay).to_string());
    }
    let _ = fs::remove_file(&lock);
}

/// Status code of the final response in a curl -D header dump (redirects
/// produce several blocks; the last HTTP/ line wins).
pub(crate) fn http_status(headers: &str) -> Option<u32> {
    headers
        .lines().rfind(|l| l.starts_with("HTTP/"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Numeric Retry-After from the final header block. The HTTP-date form is
/// rare on API rate limits and falls back to the default backoff.
pub(crate) fn retry_after_secs(headers: &str) -> Option<u64> {
    headers
        .lines()
        .filter_map(|l| l.split_once(':')).rfind(|(k, _)| k.eq_ignore_ascii_case("retry-after"))?
        .1
        .trim()
        .parse()
        .ok()
}

/// A weekly utilization that falls to 0 while its reset window is unchanged
/// is impossible in real usage — it's a degraded/placeholder response.
fn suspicious_drop(cache: &Path, new: &Value) -> bool {
    let Ok(old) = fs::read_to_string(cache) else {
        return false;
    };
    let old: Value = serde_json::from_str(&old).unwrap_or(Value::Null);
    old["seven_day"]["utilization"].as_f64().unwrap_or(0.0) > 0.0
        && new["seven_day"]["utilization"].as_f64() == Some(0.0)
        && old["seven_day"]["resets_at"] == new["seven_day"]["resets_at"]
}

fn touch(path: &Path) {
    if let Ok(s) = fs::read(path) {
        let _ = fs::write(path, s);
    } else {
        let _ = fs::write(path, b"");
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)
}

fn acquire_lock(lock: &Path) -> bool {
    if let Some(age) = file_age(lock) {
        if age < LOCK_MAX_AGE {
            return false;
        }
        let _ = fs::remove_file(lock);
    }
    let _ = fs::create_dir_all(lock.parent().unwrap_or(Path::new("/")));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_retry_after_from_final_block() {
        let h = "HTTP/1.1 301 Moved\r\nlocation: /x\r\n\r\nHTTP/2 429 \r\nretry-after: 97\r\n\r\n";
        assert_eq!(http_status(h), Some(429));
        assert_eq!(retry_after_secs(h), Some(97));
    }

    #[test]
    fn http_date_retry_after_falls_back() {
        let h = "HTTP/2 429 \r\nRetry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\n\r\n";
        assert_eq!(http_status(h), Some(429));
        assert_eq!(retry_after_secs(h), None);
    }
}
