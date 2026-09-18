// Easter egg: user-defined codenames for Claude model families.
//
// `ccline codename` (undocumented on purpose) toggles the feature; the map
// lives in a small JSON file under the config dir and survives toggling off.
// When enabled, the model widget shows the codename instead of the display
// name ("Opus 5" → "Clanker"). The Fable weekly-limit label follows the
// same map.

use serde_json::{Map, Value, json};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::util::home;

const DEFAULTS: [(&str, &str); 4] = [
    ("haiku", "Toaster"),
    ("sonnet", "Scrappy"),
    ("opus", "Clanker"),
    ("fable", "Omnissiah"),
];

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
/// display name becomes the codename (no version suffix).
fn rewrite(name: &str, map: &Map<String, Value>) -> String {
    let lower = name.to_lowercase();
    // Longest key first so "opus" can't shadow a hypothetical "opus-mini".
    let mut keys: Vec<(String, &str)> = map
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.to_lowercase(), s)))
        .filter(|(k, _)| !k.is_empty())
        .collect();
    keys.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    keys.into_iter()
        .find(|(k, _)| lower.contains(k.as_str()))
        .map_or_else(|| name.to_string(), |(_, codename)| codename.to_string())
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
        assert_eq!(rewrite("Opus 5", &m), "Clanker");
        assert_eq!(rewrite("Fable 5.1", &m), "Omnissiah");
        assert_eq!(rewrite("Claude Haiku 4.5", &m), "Toaster");
        assert_eq!(rewrite("Fable", &m), "Omnissiah");
    }

    #[test]
    fn unknown_names_pass_through() {
        assert_eq!(
            rewrite("Mythos 5.1", &Config::defaults().names),
            "Mythos 5.1"
        );
    }

    #[test]
    fn custom_family_and_case_insensitive_keys() {
        let mut m = Map::new();
        m.insert("MYTHOS".into(), Value::String("Basilisk".into()));
        assert_eq!(rewrite("Mythos 5.1", &m), "Basilisk");
    }
}
