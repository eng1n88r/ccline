// Easter egg: user-defined codenames for Claude model families.
//
// `ccline codename` (undocumented on purpose) toggles the feature; the map
// lives in a small JSON file under the config dir and survives toggling off.
// When enabled, the model widget shows the codename instead of the display
// name: the current version of a family gets the bare codename
// ("Opus 5.5" → "Clanker"), older ones keep their version ("Opus 4.8" →
// "Clanker 4.8"). The Fable weekly-limit label follows the same map.

use serde_json::{Map, Value, json};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::util::home;

const DEFAULTS: [(&str, &str); 5] = [
    ("haiku", "Toaster"),
    ("sonnet", "Scrappy"),
    ("opus", "Clanker"),
    ("fable", "Omnissiah"),
    ("mythos", "Basilisk"),
];

/// Current version per family; it renders as the bare codename, any other
/// version keeps its number. Bump when a new model generation ships.
const CURRENT: [(&str, &str); 5] = [
    ("haiku", "5.5"),
    ("sonnet", "5.5"),
    ("opus", "5.5"),
    ("fable", "5.2"),
    ("mythos", "5.2"),
];

/// The version token of a display name: "Claude Opus 4.8 (1M)" → "4.8".
fn version_of(name: &str) -> Option<&str> {
    name.split_whitespace()
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))
        .map(|t| t.trim_end_matches(|c: char| !c.is_ascii_digit()))
}

fn config_path() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .or_else(|| env::var_os("APPDATA")) // Windows
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("ccline")
        .join("codenames.json")
}

struct Config {
    enabled: bool,
    names: Map<String, Value>,
}

impl Config {
    fn defaults() -> Self {
        Self {
            enabled: false,
            names: DEFAULTS
                .iter()
                .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                .collect(),
        }
    }

    /// The stored config; defaults (disabled) when the file is missing or
    /// unreadable.
    fn load() -> Self {
        let Some(v) = fs::read_to_string(config_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            return Self::defaults();
        };
        let mut cfg = Self::defaults();
        cfg.enabled = v["enabled"].as_bool().unwrap_or(false);
        if let Some(m) = v["names"].as_object() {
            cfg.names = m.clone();
        }
        cfg
    }

    fn save(&self) -> Result<(), String> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let v = json!({ "enabled": self.enabled, "names": self.names });
        let mut text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        text.push('\n');
        fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Rewrites a model display name using the active map; identity when off.
pub(crate) fn apply(name: &str) -> String {
    let cfg = Config::load();
    if cfg.enabled {
        rewrite(name, &cfg.names)
    } else {
        name.to_string()
    }
}

/// Case-insensitive match of a family key anywhere in `name`; the whole
/// display name becomes the codename, plus the version when it isn't the
/// family's current one. Families without a known current version (custom
/// ones) and names without a version get the bare codename.
fn rewrite(name: &str, map: &Map<String, Value>) -> String {
    let lower = name.to_lowercase();
    // Longest key first so "opus" can't shadow a hypothetical "opus-mini".
    let mut keys: Vec<(String, &str)> = map
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.to_lowercase(), s)))
        .filter(|(k, _)| !k.is_empty())
        .collect();
    keys.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    let Some((family, codename)) = keys.into_iter().find(|(k, _)| lower.contains(k.as_str()))
    else {
        return name.to_string();
    };
    let current = CURRENT.iter().find(|(k, _)| *k == family).map(|(_, v)| *v);
    match (version_of(name), current) {
        (Some(v), Some(cur)) if v != cur => format!("{codename} {v}"),
        _ => codename.to_string(),
    }
}

/// `ccline codename` toggles the feature; subcommands edit the map.
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    let mut cfg = Config::load();
    match arg(0) {
        None => cfg.enabled = !cfg.enabled,
        Some("on") => cfg.enabled = true,
        Some("off") => cfg.enabled = false,
        Some("show" | "status") => return show(&cfg),
        Some("set") => {
            let (Some(family), Some(name)) = (arg(1), arg(2)) else {
                return Err("usage: ccline codename set <family> <name>".into());
            };
            cfg.names
                .insert(family.to_lowercase(), Value::String(name.to_string()));
        }
        Some("unset") => {
            let Some(family) = arg(1) else {
                return Err("usage: ccline codename unset <family>".into());
            };
            cfg.names.remove(&family.to_lowercase());
        }
        Some("reset") => cfg.names = Config::defaults().names,
        Some(other) => return Err(format!("unknown codename command: {other}\n{USAGE}")),
    }
    cfg.save()?;
    show(&cfg)
}

const USAGE: &str = "\
usage: ccline codename                 toggle codenames on/off
       ccline codename on|off          set explicitly
       ccline codename show            print this
       ccline codename set <family> <name>
       ccline codename unset <family>
       ccline codename reset           restore the default names";

fn show(cfg: &Config) -> Result<(), String> {
    let state = if cfg.enabled { "on" } else { "off" };
    println!("codenames {state} ({})", config_path().display());
    // Defaults in their canonical order, then any custom families.
    let known: Vec<&str> = DEFAULTS.iter().map(|(k, _)| *k).collect();
    let ordered = known
        .iter()
        .filter_map(|k| cfg.names.get(*k).map(|v| (k.to_string(), v)))
        .chain(
            cfg.names
                .iter()
                .filter(|(k, _)| !known.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v)),
        );
    for (family, codename) in ordered {
        println!("  {family:<8} → {}", codename.as_str().unwrap_or("?"));
    }
    println!("\n{USAGE}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_whole_name() {
        let m = Config::defaults().names;
        assert_eq!(rewrite("Opus 5.5", &m), "Clanker");
        assert_eq!(rewrite("Fable 5.2", &m), "Omnissiah");
        assert_eq!(rewrite("Claude Haiku 5.5", &m), "Toaster");
        assert_eq!(rewrite("Mythos 5.2", &m), "Basilisk");
        assert_eq!(rewrite("Fable", &m), "Omnissiah");
    }

    #[test]
    fn older_versions_keep_their_number() {
        let m = Config::defaults().names;
        assert_eq!(rewrite("Opus 5", &m), "Clanker 5");
        assert_eq!(rewrite("Opus 4.8", &m), "Clanker 4.8");
        assert_eq!(rewrite("Fable 5.1", &m), "Omnissiah 5.1");
        assert_eq!(rewrite("Claude Haiku 4.5", &m), "Toaster 4.5");
        assert_eq!(rewrite("Sonnet 4.6 (1M context)", &m), "Scrappy 4.6");
    }

    #[test]
    fn unknown_names_pass_through() {
        assert_eq!(rewrite("Gizmo 5.1", &Config::defaults().names), "Gizmo 5.1");
    }

    #[test]
    fn custom_family_and_case_insensitive_keys() {
        let mut m = Map::new();
        m.insert("GIZMO".into(), Value::String("Sparky".into()));
        assert_eq!(rewrite("Gizmo 5.1", &m), "Sparky");
    }
}
