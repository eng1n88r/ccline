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
use std::io::Read;
use style::dim;

mod context;
mod git;
mod session;
mod style;
mod update;
mod usage;
mod util;

fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--refresh-usage") => usage::refresh_usage(),
        Some("--refresh-session-name") => {
            session::refresh_session_name(args.get(2).map(String::as_str))
        }
        Some("update" | "--update") => {
            if let Err(e) = update::run() {
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
    if let Some(name) = session::session_name(&data) {
        segs.push(format!("\x1b[35m{name}\x1b[0m"));
    }
    if let Some(seg) = context::context_segment(&data) {
        segs.push(seg);
    }
    usage::usage_segments(&data, &mut segs);
    if let Some(repo) = git::repo_name(&data) {
        segs.push(format!("\x1b[1;34m{repo}\x1b[0m"));
    }
    git::git_segments(&data, &mut segs);

    // Single row; Claude Code trims each line, so leading padding is moot.
    println!("{}", segs.join(&dim(" │ ")));
}
