// ccline — fast statusline for Claude Code, replacing ccstatusline.
//
// Render path is pure-local (stdin JSON + cache files + git); anything slow
// (usage API, `claude agents`) is refreshed by detached child processes so a
// render never blocks on the network or on Node startup.
//
// Widgets: model · version · session-name · context · 5h/weekly/Fable usage ·
// git branch · git changes. Colors use the 16-color ANSI palette so the line
// follows the active terminal theme (omarchy themes restyle it automatically).

use serde_json::Value;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

const USAGE_TTL: Duration = Duration::from_secs(60);
const SESSION_NAME_TTL: Duration = Duration::from_secs(60);
const LOCK_MAX_AGE: Duration = Duration::from_secs(60);

fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--refresh-usage") => refresh_usage(),
        Some("--refresh-session-name") => refresh_session_name(args.get(2).map(String::as_str)),
        Some("update" | "--update") => {
            if let Err(e) = update() {
                eprintln!("update failed: {e}");
                std::process::exit(1);
            }
        }
        Some("--version" | "-V") => println!("ccline {}", env!("CARGO_PKG_VERSION")),
        _ => render(),
    }
}

// ---------------------------------------------------------------- rendering

fn render() {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let data: Value = serde_json::from_str(&input).unwrap_or(Value::Null);

    let mut segs: Vec<String> = Vec::with_capacity(9);

    if let Some(model) = data["model"]["display_name"].as_str() {
        segs.push(format!("\x1b[1;36m{model}\x1b[0m"));
    }
    if let Some(v) = data["version"].as_str() {
        segs.push(dim(&format!("v{v}")));
    }
    if let Some(name) = session_name(&data) {
        segs.push(format!("\x1b[35m{name}\x1b[0m"));
    }
    if let Some(seg) = context_segment(&data) {
        segs.push(seg);
    }
    usage_segments(&data, &mut segs);
    if let Some(repo) = repo_name(&data) {
        segs.push(format!("\x1b[1;34m{repo}\x1b[0m"));
    }
    git_segments(&data, &mut segs);

    // Single row; Claude Code trims each line, so leading padding is moot.
    println!("{}", segs.join(&dim(" │ ")));
}

fn dim(s: &str) -> String {
    format!("\x1b[2m{s}\x1b[0m")
}

/// Green under 50%, yellow under 80%, red above.
fn pct_color(pct: f64) -> &'static str {
    if pct < 50.0 {
        "\x1b[32m"
    } else if pct < 80.0 {
        "\x1b[33m"
    } else {
        "\x1b[31m"
    }
}

fn fmt_tokens(n: f64) -> String {
    if n >= 1000.0 {
        format!("{:.1}k", n / 1000.0)
    } else {
        format!("{n:.0}")
    }
}

// ------------------------------------------------------------- session name

/// Shares the cache file used by the old session-name.cjs script.
fn session_name(data: &Value) -> Option<String> {
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

fn refresh_session_name(sid: Option<&str>) {
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

// ----------------------------------------------------------------- context

fn context_segment(data: &Value) -> Option<String> {
    let cw = &data["context_window"];
    let used = current_usage_tokens(&cw["current_usage"])
        .or_else(|| cw["context_length_tokens"].as_f64())
        .or_else(|| transcript_context_tokens(data))?;
    let window = cw["context_window_size"].as_f64().unwrap_or(200_000.0);
    let pct = (used / window * 100.0).clamp(0.0, 100.0);
    Some(format!(
        "{}{} {:.0}%\x1b[0m",
        pct_color(pct),
        fmt_tokens(used),
        pct
    ))
}

fn current_usage_tokens(usage: &Value) -> Option<f64> {
    let get = |keys: [&str; 2]| keys.iter().find_map(|k| usage[*k].as_f64()).unwrap_or(0.0);
    if !usage.is_object() {
        return None;
    }
    let total = get(["input", "input_tokens"])
        + get(["creation", "cache_creation_input_tokens"])
        + get(["read", "cache_read_input_tokens"]);
    (total > 0.0).then_some(total)
}

/// Fallback for older payloads: last usage entry in the transcript tail.
fn transcript_context_tokens(data: &Value) -> Option<f64> {
    let path = data["transcript_path"].as_str()?;
    let content = read_tail(Path::new(path), 256 * 1024)?;
    for line in content.lines().rev() {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        if let Some(t) = current_usage_tokens(&entry["message"]["usage"]) {
            return Some(t);
        }
    }
    None
}

fn read_tail(path: &Path, max: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len > max {
        f.seek(SeekFrom::Start(len - max)).ok()?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

// ------------------------------------------------------------------- usage

fn usage_segments(data: &Value, segs: &mut Vec<String>) {
    let api = load_usage_cache();

    let session = data["rate_limits"]["five_hour"]["utilization"]
        .as_f64()
        .or_else(|| api["five_hour"]["utilization"].as_f64());
    let weekly = data["rate_limits"]["seven_day"]["utilization"]
        .as_f64()
        .or_else(|| api["seven_day"]["utilization"].as_f64());
    let fable = scoped_limit_pct(&api, "fable");

    for (label, pct) in [("5h", session), ("wk", weekly), ("Fable", fable)] {
        if let Some(p) = pct {
            segs.push(format!("{} {}{:.0}%\x1b[0m", dim(label), pct_color(p), p));
        }
    }
}

/// Percent for a `weekly_scoped` limit whose model display name matches.
fn scoped_limit_pct(api: &Value, model: &str) -> Option<f64> {
    api["limits"].as_array()?.iter().find_map(|l| {
        let scoped = l["kind"].as_str() == Some("weekly_scoped");
        let name = l["scope"]["model"]["display_name"].as_str().unwrap_or("");
        (scoped && name.to_lowercase().contains(model))
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

fn home() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Returns the cached raw usage-API response, kicking off a detached refresh
/// when the cache is stale. Never blocks on the network.
fn load_usage_cache() -> Value {
    let dir = cache_dir();
    let cache = dir.join("usage.json");
    let stale = file_age(&cache).is_none_or(|age| age > USAGE_TTL);
    if stale && acquire_lock(&dir.join("usage.lock")) {
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
    if cfg!(target_os = "macos") {
        if let Ok(out) = Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output()
        {
            if out.status.success() {
                return String::from_utf8_lossy(&out.stdout).into_owned();
            }
        }
    }
    String::new()
}

fn refresh_usage() {
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

    let out = Command::new("curl")
        .args(["-sf", "-m", "8", "-K"])
        .arg(&cfg_path)
        .output();
    let _ = fs::remove_file(&cfg_path);

    if let Ok(out) = out {
        if out.status.success() && serde_json::from_slice::<Value>(&out.stdout).is_ok() {
            let tmp = dir.join("usage.json.tmp");
            if fs::write(&tmp, &out.stdout).is_ok() {
                let _ = fs::rename(&tmp, dir.join("usage.json"));
            }
        }
    }
    let _ = fs::remove_file(&lock);
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

// --------------------------------------------------------------------- git

/// Basename of the project dir (the repo name for git projects).
fn repo_name(data: &Value) -> Option<String> {
    let dir = data["workspace"]["project_dir"]
        .as_str()
        .or_else(|| data["workspace"]["current_dir"].as_str())
        .or_else(|| data["cwd"].as_str())?;
    let name = Path::new(dir).file_name()?.to_string_lossy();
    (!name.is_empty()).then(|| name.into_owned())
}

fn git_segments(data: &Value, segs: &mut Vec<String>) {
    let dir = data["workspace"]["current_dir"]
        .as_str()
        .or_else(|| data["cwd"].as_str())
        .unwrap_or(".");
    let Some(branch) = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
        return;
    };
    segs.push(format!("\x1b[34m\u{e0a0} {}\x1b[0m", branch.trim()));

    if let Some(stat) = git(dir, &["diff", "HEAD", "--shortstat"]) {
        let ins = parse_stat(&stat, "insertion");
        let del = parse_stat(&stat, "deletion");
        if ins + del > 0 {
            segs.push(format!("\x1b[32m+{ins}\x1b[0m \x1b[31m-{del}\x1b[0m"));
        }
    }
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Pulls the number before e.g. "insertion" out of `git diff --shortstat`.
fn parse_stat(stat: &str, word: &str) -> u64 {
    stat.find(word)
        .map(|i| {
            stat[..i]
                .rsplit(|c: char| !c.is_ascii_digit())
                .find(|s| !s.is_empty())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

// ------------------------------------------------------------------ update

const REPO: &str = "eng1n88r/ccline";

/// Release asset target triple for the running platform, matching the CI matrix.
fn release_target() -> Option<&'static str> {
    Some(match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    })
}

/// `ccline update`: download the latest release binary and replace this
/// executable in place.
fn update() -> Result<(), String> {
    let current = env!("CARGO_PKG_VERSION");
    let tmp = env::temp_dir().join(format!("ccline-update-{}", std::process::id()));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let _cleanup = Cleanup(tmp.clone());

    // Latest version from the /releases/latest redirect (no API token needed).
    let effective = curl(
        &tmp,
        &[
            "-o",
            &tmp.join("headers").to_string_lossy(),
            "-w",
            "%{url_effective}",
            "-I",
            &format!("https://github.com/{REPO}/releases/latest"),
        ],
    )?;
    let latest = effective
        .trim()
        .rsplit("/tag/v")
        .next()
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .ok_or_else(|| format!("cannot parse latest version from {effective:?}"))?
        .to_string();
    if ver_key(&latest) <= ver_key(current) {
        println!("ccline {current} is already the latest release (v{latest})");
        return Ok(());
    }

    let target = release_target().ok_or("no prebuilt release for this platform")?;
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let url =
        format!("https://github.com/{REPO}/releases/latest/download/ccline-{target}.{ext}");
    println!("downloading {url}");
    let archive = tmp.join(format!("ccline.{ext}"));
    curl(&tmp, &["-o", &archive.to_string_lossy(), &url])?;

    // bsdtar (macOS, Windows 10+) and GNU tar both auto-detect the format.
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&tmp)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        return Err("tar extraction failed".into());
    }

    let new_bin = tmp.join(if cfg!(windows) { "ccline.exe" } else { "ccline" });
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    replace_exe(&new_bin, &exe)?;
    println!("updated ccline {current} -> {latest} at {}", exe.display());
    Ok(())
}

/// "1.2.10" -> (1, 2, 10), for ordered version comparison.
fn ver_key(v: &str) -> (u64, u64, u64) {
    let mut parts = v.split('.').map(|p| p.parse().unwrap_or(0));
    let mut next = || parts.next().unwrap_or(0);
    (next(), next(), next())
}

/// Replace `exe` with `new_bin`, handling the running-executable case.
fn replace_exe(new_bin: &Path, exe: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(new_bin, fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    // Stage next to the destination so the final rename is atomic and never
    // crosses filesystems; on Windows the running exe is moved aside first.
    let staged = exe.with_extension("new");
    fs::copy(new_bin, &staged).map_err(|e| format!("staging {}: {e}", staged.display()))?;
    if cfg!(windows) {
        let old = exe.with_extension("old");
        let _ = fs::remove_file(&old);
        fs::rename(exe, &old).map_err(|e| e.to_string())?;
    }
    fs::rename(&staged, exe).map_err(|e| e.to_string())
}

fn curl(tmp: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "60"])
        .args(args)
        .current_dir(tmp)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Removes its directory on drop, so temp files go away on every exit path.
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// ----------------------------------------------------------------- helpers

fn file_age(path: &Path) -> Option<Duration> {
    let mtime = fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(mtime).ok()
}

fn spawn_self(args: &[&str]) {
    if let Ok(exe) = env::current_exe() {
        let _ = Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}
