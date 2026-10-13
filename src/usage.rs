use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::style::{dim, pct_color};
use crate::util::{file_age, home, now_secs, spawn_self};

const USAGE_TTL: Duration = Duration::from_secs(60);
const USAGE_BACKOFF: Duration = Duration::from_secs(300);
const LOCK_MAX_AGE: Duration = Duration::from_secs(60);
/// Past this cache age the API-derived segments get a `?` suffix, so a refresh
/// that keeps failing is visible on the status line instead of silently
/// serving days-old numbers.
const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

pub(crate) fn usage_segments(data: &Value, segs: &mut Vec<String>) {
    let (api, age) = load_usage_cache();
    let stale = age.is_some_and(|a| a > STALE_AFTER);

    // Each value carries whether it came from the API cache (and so can be
    // stale) or from the live stdin payload.
    let session = bucket_pct(&data["rate_limits"]["five_hour"])
        .map(|p| (p, false))
        .or_else(|| bucket_pct(&api["five_hour"]).map(|p| (p, true)));
    let weekly = bucket_pct(&data["rate_limits"]["seven_day"])
        .map(|p| (p, false))
        .or_else(|| bucket_pct(&api["seven_day"]).map(|p| (p, true)));
    let fable = scoped_limit_pct(&api, "fable").map(|p| (p, true));
    // Weekly reset countdown, shown after the weekly percent.
    let weekly_reset = reset_epoch(&data["rate_limits"]["seven_day"]["resets_at"])
        .or_else(|| reset_epoch(&api["seven_day"]["resets_at"]))
        .map(|t| fmt_countdown(t.saturating_sub(now_secs())));

    let fable_label = crate::codename::apply("Fable");
    for (label, pct, extra) in [
        ("5h", session, None),
        ("wk", weekly, weekly_reset),
        (fable_label.as_str(), fable, None),
    ] {
        if let Some((p, from_api)) = pct {
            let mark = if from_api && stale { "?" } else { "" };
            let mut seg = format!("{} {}{:.0}%{mark}\x1b[0m", dim(label), pct_color(p), p);
            if let Some(e) = extra {
                seg.push(' ');
                seg.push_str(&dim(&e));
            }
            segs.push(seg);
        }
    }
}

/// `resets_at` as unix seconds. The usage API sends RFC 3339 strings; the
/// stdin payload may send a number (seconds or milliseconds).
fn reset_epoch(v: &Value) -> Option<u64> {
    match v {
        Value::String(s) => parse_rfc3339(s),
        Value::Number(n) => epoch_secs(n.as_f64()?),
        _ => None,
    }
}

/// A positive unix timestamp in seconds or milliseconds, normalized to seconds.
fn epoch_secs(t: f64) -> Option<u64> {
    (t > 0.0).then(|| if t > 1e11 { t / 1000.0 } else { t } as u64)
}

/// Minimal RFC 3339 → unix seconds: `YYYY-MM-DDThh:mm:ss[.frac](Z|±hh:mm)`.
/// Fractional seconds are dropped; no external deps on the render path.
fn parse_rfc3339(s: &str) -> Option<u64> {
    let s = s.trim();
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    if s.len() < 19 || s.as_bytes().get(10).copied() != Some(b'T') {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = &s[19..];
    let rest = match rest.find(|c| c != '.' && !char::is_ascii_digit(&c)) {
        Some(i) if rest.starts_with('.') => &rest[i..],
        Some(_) => rest,
        None => return None,
    };
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6)?.parse::<i64>().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    // Howard Hinnant's days-from-civil.
    let (y, mo) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * mo + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let t = days * 86_400 + h * 3600 + mi * 60 + sec - offset;
    u64::try_from(t).ok()
}

/// Compact countdown: `2d4h`, `3h12m`, `45m`, or `now` once elapsed.
fn fmt_countdown(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h, m) {
        (0, 0, 0) => "now".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h{m:02}m"),
        (d, h, _) => format!("{d}d{h}h"),
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

/// Returns the cached raw usage-API response and its age, kicking off a
/// detached refresh when the cache is stale. Never blocks on the network.
fn load_usage_cache() -> (Value, Option<Duration>) {
    let dir = cache_dir();
    let cache = dir.join("usage.json");
    let age = file_age(&cache);
    let stale = age.is_none_or(|age| age > USAGE_TTL);
    if stale
        && !backoff_until(&dir).is_some_and(|until| now_secs() < until)
        && acquire_lock(&dir.join("usage.lock"))
    {
        spawn_self(&["--refresh-usage"]);
    }
    let v = fs::read_to_string(&cache)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    (v, age)
}

/// The blocked-until unix timestamp from the backoff file (set from
/// Retry-After on a 429, or a default on other failures).
fn backoff_until(dir: &Path) -> Option<u64> {
    fs::read_to_string(dir.join("usage.backoff"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// One OAuth credential from one of Claude Code's stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Credential {
    /// Where it came from: `keychain` or `file`.
    pub(crate) source: &'static str,
    pub(crate) token: String,
    /// Access-token expiry as unix seconds, when the store records one.
    pub(crate) expires_at: Option<u64>,
}

/// Every store Claude Code may keep its credentials in, as raw JSON. On
/// macOS that is the login Keychain (the current default) and the legacy
/// `~/.claude/.credentials.json`; elsewhere only the file.
fn credential_stores() -> Vec<(&'static str, String)> {
    let mut stores = Vec::new();
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
        stores.push((
            "keychain",
            String::from_utf8_lossy(&out.stdout).into_owned(),
        ));
    }
    if let Ok(s) = fs::read_to_string(home().join(".claude/.credentials.json")) {
        stores.push(("file", s));
    }
    stores
}

fn parse_credential(raw: &str, source: &'static str) -> Option<Credential> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let oauth = &v["claudeAiOauth"];
    let token = oauth["accessToken"].as_str()?.to_owned();
    if token.is_empty() {
        return None;
    }
    let expires_at = oauth["expiresAt"].as_f64().and_then(epoch_secs);
    Some(Credential {
        source,
        token,
        expires_at,
    })
}

/// Usable credentials, best first. Expired tokens are dropped (a stale
/// `.credentials.json` left behind after Claude Code moved to the Keychain
/// is the common case), the latest expiry wins, unknown expiry sorts last,
/// and duplicate tokens collapse to one. `now` is unix seconds.
pub(crate) fn select_credentials(stores: &[(&'static str, String)], now: u64) -> Vec<Credential> {
    let mut creds: Vec<Credential> = stores
        .iter()
        .filter_map(|(source, raw)| parse_credential(raw, source))
        .filter(|c| c.expires_at.is_none_or(|t| t > now))
        .collect();
    // Option<u64> orders None first, so descending puts known expiries
    // first (latest at the top) and unknown ones last.
    creds.sort_by_key(|c| std::cmp::Reverse(c.expires_at));
    creds.dedup_by(|a, b| a.token == b.token);
    creds
}

/// Outcome of one usage-API request.
struct Fetch {
    status: Option<u32>,
    body: Vec<u8>,
    headers: String,
}

/// GET the usage endpoint with `token`. The response headers stay in
/// `usage.hdr` so a failing status is inspectable afterwards.
fn fetch_usage(dir: &Path, token: &str) -> Fetch {
    // Headers go through a 0600 curl config file, not argv.
    let cfg_path = dir.join("curl.cfg");
    let cfg = format!(
        "url = \"https://api.anthropic.com/api/oauth/usage\"\n\
         header = \"Authorization: Bearer {token}\"\n\
         header = \"anthropic-beta: oauth-2025-04-20\"\n"
    );
    if write_private(&cfg_path, cfg.as_bytes()).is_err() {
        return Fetch {
            status: None,
            body: Vec::new(),
            headers: String::new(),
        };
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
    Fetch {
        status: http_status(&headers),
        body: out.map(|o| o.stdout).unwrap_or_default(),
        headers,
    }
}

pub(crate) fn refresh_usage() {
    let dir = cache_dir();
    let _ = fs::create_dir_all(&dir);
    refresh_inner(&dir);
    let _ = fs::remove_file(dir.join("usage.lock"));
}

fn refresh_inner(dir: &Path) {
    let creds = select_credentials(&credential_stores(), now_secs());
    if creds.is_empty() {
        record_failure(
            dir,
            "no usable credentials (every store missing, unparseable, or expired)",
            USAGE_BACKOFF.as_secs(),
        );
        return;
    }

    let cache = dir.join("usage.json");
    let mut last_status = None;
    let mut retry_after = None;
    let mut attempts = Vec::with_capacity(creds.len());
    for c in &creds {
        let got = fetch_usage(dir, &c.token);
        attempts.push(format!(
            "{}={}",
            c.source,
            got.status.map_or("none".to_owned(), |s| s.to_string())
        ));
        last_status = got.status;
        match got.status {
            Some(200) => {
                if let Ok(new) = serde_json::from_slice::<Value>(&got.body) {
                    if suspicious_drop(&cache, &new) {
                        // Degraded response under rate limiting: keep the good
                        // cache, refresh its mtime so we don't refetch in a loop.
                        touch(&cache);
                    } else {
                        let tmp = dir.join("usage.json.tmp");
                        if fs::write(&tmp, &got.body).is_ok() {
                            let _ = fs::rename(&tmp, &cache);
                        }
                    }
                    for f in ["usage.backoff", "usage.err", "usage.hdr"] {
                        let _ = fs::remove_file(dir.join(f));
                    }
                    return;
                }
                // 200 with an unparseable body is an outage, not a bad token.
                break;
            }
            // The API rejected this credential (or throttled it, which an
            // expired-but-undated token also produces). Try the next store
            // before giving up — this is the case that used to loop forever
            // on a dead token.
            Some(401 | 403 | 429) => {
                if got.status == Some(429) {
                    retry_after = retry_after_secs(&got.headers);
                }
            }
            // Outage, bad response, network error: another token won't help.
            _ => break,
        }
    }

    // On 429 honor Retry-After (clamped to an hour); anything else backs
    // off the default.
    let delay = match last_status {
        Some(429) => retry_after
            .unwrap_or(USAGE_BACKOFF.as_secs())
            .clamp(1, 3600),
        _ => USAGE_BACKOFF.as_secs(),
    };
    record_failure(
        dir,
        &format!("refresh failed: {}", attempts.join(" ")),
        delay,
    );
}

/// Set the backoff and leave a one-line note for `--usage-status`.
fn record_failure(dir: &Path, msg: &str, delay: u64) {
    let now = now_secs();
    let _ = fs::write(dir.join("usage.backoff"), (now + delay).to_string());
    let _ = fs::write(dir.join("usage.err"), format!("{now} {msg}\n"));
}

/// `ccline --usage-status`: why the usage segments show what they show.
/// Prints cache age, backoff, the last failure, which credential stores are
/// usable (never the tokens), and the cached numbers.
pub(crate) fn usage_status() {
    let dir = cache_dir();
    let now = now_secs();
    println!("cache dir     {}", dir.display());

    let cache = dir.join("usage.json");
    match file_age(&cache) {
        Some(age) => {
            let flag = if age > STALE_AFTER {
                "  (stale: segments marked with ?)"
            } else {
                ""
            };
            println!("cache age     {}{flag}", fmt_age(age));
        }
        None => println!("cache age     missing"),
    }
    match backoff_until(&dir) {
        Some(until) if now < until => println!(
            "backoff       {} remaining",
            fmt_age(Duration::from_secs(until - now))
        ),
        Some(_) => println!("backoff       expired (next render retries)"),
        None => println!("backoff       none"),
    }
    match fs::read_to_string(dir.join("usage.err")) {
        Ok(e) => {
            let e = e.trim();
            let (ts, msg) = e.split_once(' ').unwrap_or((e, ""));
            let ago = ts
                .parse::<u64>()
                .ok()
                .map(|t| fmt_age(Duration::from_secs(now.saturating_sub(t))));
            println!(
                "last error    {msg}  ({} ago)",
                ago.unwrap_or_else(|| "?".into())
            );
        }
        Err(_) => println!("last error    none"),
    }
    if let Ok(h) = fs::read_to_string(dir.join("usage.hdr"))
        && let Some(status) = http_status(&h)
    {
        println!("last response HTTP {status}");
    }

    let stores = credential_stores();
    let selected = select_credentials(&stores, now);
    println!("credentials");
    if stores.is_empty() {
        println!("  none found (keychain / ~/.claude/.credentials.json)");
    }
    for (source, raw) in &stores {
        let state = match parse_credential(raw, source) {
            None => "unparseable or no accessToken".to_owned(),
            Some(c) => match c.expires_at {
                Some(t) if t > now => {
                    format!("expires in {}", fmt_age(Duration::from_secs(t - now)))
                }
                Some(t) => format!("EXPIRED {} ago", fmt_age(Duration::from_secs(now - t))),
                None => "no expiry recorded".to_owned(),
            },
        };
        let used = if selected.first().is_some_and(|c| c.source == *source) {
            "  <- used"
        } else {
            ""
        };
        println!("  {source:<9} {state}{used}");
    }

    let api: Value = fs::read_to_string(&cache)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    if !api.is_null() {
        println!("cached values");
        for (k, label) in [("five_hour", "5h"), ("seven_day", "weekly")] {
            if let Some(p) = bucket_pct(&api[k]) {
                println!("  {label:<9} {p:.0}%");
            }
        }
        for l in api["limits"].as_array().into_iter().flatten() {
            if l["kind"].as_str() == Some("weekly_scoped")
                && let Some(name) = l["scope"]["model"]["display_name"].as_str()
                && let Some(p) = l["percent"].as_f64()
            {
                println!("  {name:<9} {p:.0}%  (weekly, scoped)");
            }
        }
    }
}

/// Human age: `12d3h`, `3h12m`, `45m`, `20s`.
fn fmt_age(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else {
        fmt_countdown(s)
    }
}

/// Status code of the final response in a curl -D header dump (redirects
/// produce several blocks; the last HTTP/ line wins).
pub(crate) fn http_status(headers: &str) -> Option<u32> {
    headers
        .lines()
        .rfind(|l| l.starts_with("HTTP/"))?
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
        .filter_map(|l| l.split_once(':'))
        .rfind(|(k, _)| k.eq_ignore_ascii_case("retry-after"))?
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

    fn store(
        source: &'static str,
        token: &str,
        expires_at_ms: Option<u64>,
    ) -> (&'static str, String) {
        let mut o = serde_json::json!({ "accessToken": token });
        if let Some(t) = expires_at_ms {
            o["expiresAt"] = serde_json::json!(t);
        }
        (
            source,
            serde_json::json!({ "claudeAiOauth": o }).to_string(),
        )
    }

    #[test]
    fn expired_file_token_is_skipped_for_live_keychain_token() {
        // The incident: a leftover .credentials.json whose token died 12
        // days ago, listed before a current Keychain token.
        let now = 1_791_900_000;
        let stores = [
            store("file", "dead", Some((now - 12 * 86_400) * 1000)),
            store("keychain", "live", Some((now + 1800) * 1000)),
        ];
        let got = select_credentials(&stores, now);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, "keychain");
        assert_eq!(got[0].token, "live");
    }

    #[test]
    fn latest_expiry_first_unknown_last_duplicates_collapsed() {
        let now = 1_791_900_000;
        let stores = [
            store("file", "undated", None),
            store("keychain", "soon", Some((now + 60) * 1000)),
            store("file", "soon", Some((now + 60) * 1000)),
            store("file", "later", Some((now + 3600) * 1000)),
        ];
        let got: Vec<_> = select_credentials(&stores, now)
            .into_iter()
            .map(|c| c.token)
            .collect();
        assert_eq!(got, ["later", "soon", "undated"]);
    }

    #[test]
    fn unusable_stores_yield_nothing() {
        let now = 1_791_900_000;
        let stores = [
            ("file", "not json".to_owned()),
            (
                "keychain",
                r#"{"claudeAiOauth":{"accessToken":""}}"#.to_owned(),
            ),
            store("file", "gone", Some((now - 1) * 1000)),
        ];
        assert!(select_credentials(&stores, now).is_empty());
    }

    #[test]
    fn expiry_accepts_seconds_or_milliseconds() {
        let secs = parse_credential(&store("file", "t", Some(1_789_279_200)).1, "file").unwrap();
        let ms = parse_credential(&store("file", "t", Some(1_789_279_200_000)).1, "file").unwrap();
        assert_eq!(secs.expires_at, Some(1_789_279_200));
        assert_eq!(ms.expires_at, Some(1_789_279_200));
    }

    #[test]
    fn status_and_retry_after_from_final_block() {
        let h = "HTTP/1.1 301 Moved\r\nlocation: /x\r\n\r\nHTTP/2 429 \r\nretry-after: 97\r\n\r\n";
        assert_eq!(http_status(h), Some(429));
        assert_eq!(retry_after_secs(h), Some(97));
    }

    #[test]
    fn rfc3339_with_fraction_and_offset() {
        assert_eq!(
            parse_rfc3339("2026-09-13T06:00:00.758527+00:00"),
            Some(1_789_279_200)
        );
        assert_eq!(parse_rfc3339("2026-09-13T06:00:00Z"), Some(1_789_279_200));
        assert_eq!(
            parse_rfc3339("2026-09-13T08:00:00+02:00"),
            Some(1_789_279_200)
        );
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-09-13 06:00:00Z"), None);
        assert_eq!(parse_rfc3339("garbage"), None);
    }

    #[test]
    fn reset_epoch_accepts_numbers() {
        assert_eq!(
            reset_epoch(&serde_json::json!(1_789_279_200)),
            Some(1_789_279_200)
        );
        assert_eq!(
            reset_epoch(&serde_json::json!(1_789_279_200_000u64)),
            Some(1_789_279_200)
        );
        assert_eq!(reset_epoch(&Value::Null), None);
    }

    #[test]
    fn countdown_formats() {
        assert_eq!(fmt_countdown(0), "now");
        assert_eq!(fmt_countdown(45 * 60), "45m");
        assert_eq!(fmt_countdown(3 * 3600 + 12 * 60), "3h12m");
        assert_eq!(fmt_countdown(2 * 86_400 + 4 * 3600 + 59 * 60), "2d4h");
        assert_eq!(fmt_age(Duration::from_secs(20)), "20s");
    }

    #[test]
    fn http_date_retry_after_falls_back() {
        let h = "HTTP/2 429 \r\nRetry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\n\r\n";
        assert_eq!(http_status(h), Some(429));
        assert_eq!(retry_after_secs(h), None);
    }
}
