use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

/// Basename of the project dir (the repo name for git projects).
pub fn repo_name(data: &Value) -> Option<String> {
    let dir = data["workspace"]["project_dir"]
        .as_str()
        .or_else(|| data["workspace"]["current_dir"].as_str())
        .or_else(|| data["cwd"].as_str())?;
    let name = Path::new(dir).file_name()?.to_string_lossy();
    (!name.is_empty()).then(|| name.into_owned())
}

pub fn git_segments(data: &Value, segs: &mut Vec<String>) {
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
