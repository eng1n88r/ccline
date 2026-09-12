use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::style::{fmt_tokens, pct_color};

pub(crate) fn context_segment(data: &Value) -> Option<String> {
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
