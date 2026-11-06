use std::collections::BTreeMap;

use serde::Deserialize;

use crate::command::Command;
use crate::hotkey::{self, Hotkey};
use crate::model::{InsertMode, OverflowMode};
use crate::rules::{RuleSet, RuleSpec};
use crate::tree::Gaps;

/// Written to disk on first run. Also the source of the built-in defaults.
pub const DEFAULT_CONFIG: &str = r#"# tiler configuration
# Reload with the "reload" command (default Alt+Shift+R).

# Number of workspaces per monitor.
workspaces = 9

# Maximum tiled windows per workspace (0 = unlimited).
max_tiles = 3

# When a workspace already has max_tiles windows, a new window first goes to another
# monitor that has room. If none has:
#   "laptop-stack"   - it goes to the stack monitor (below). A workspace there with more
#                      than max_tiles windows shows them all full screen, stacked
#                      (cycle with focus up/down), and goes back to tiles when the
#                      count drops to max_tiles.
#   "next-workspace" - it goes to the next workspace with room and you are switched there.
overflow = "laptop-stack"

# Which monitor receives the stack: "auto" = the built-in laptop screen, or a device
# name as shown by `tiler --monitors`, e.g. '\\.\DISPLAY2'. If it isn't connected
# (lid closed, desktop PC), each monitor stacks its own overflow.
stack_monitor = "auto"

# How the stack monitor (laptop screen) shows its windows:
#   "tile"  - tiles like any monitor; stacks only when it has more than max_tiles
#   "stack" - always full screen and stacked
laptop_layout = "tile"

# Which tile a new window splits: "largest" (balanced, 4 windows = 2x2 grid)
# or "focused" (the focused tile is halved each time).
insert = "largest"

# Fraction of the split moved by each "resize" command.
resize_step = 0.05

[gaps]
outer = 8
inner = 8

# Hotkey = command.
# Modifiers: alt, ctrl, shift, win. Keys: a-z, 0-9, f1-f24, space, enter, arrows, ...
# Commands: focus|swap|resize <left|right|up|down>, workspace N, send-to-workspace N,
#           toggle-float, close, retile, reload, quit
# Note: Alt+letter hotkeys are global and override app menu accelerators
# (e.g. Alt+H for the Office ribbon). Rebind if that bothers you.
[keys]
"alt+h" = "focus left"
"alt+j" = "focus down"
"alt+k" = "focus up"
"alt+l" = "focus right"
"alt+shift+h" = "swap left"
"alt+shift+j" = "swap down"
"alt+shift+k" = "swap up"
"alt+shift+l" = "swap right"
"alt+ctrl+h" = "resize left"
"alt+ctrl+j" = "resize down"
"alt+ctrl+k" = "resize up"
"alt+ctrl+l" = "resize right"
"alt+1" = "workspace 1"
"alt+2" = "workspace 2"
"alt+3" = "workspace 3"
"alt+4" = "workspace 4"
"alt+5" = "workspace 5"
"alt+6" = "workspace 6"
"alt+7" = "workspace 7"
"alt+8" = "workspace 8"
"alt+9" = "workspace 9"
"alt+shift+1" = "send-to-workspace 1"
"alt+shift+2" = "send-to-workspace 2"
"alt+shift+3" = "send-to-workspace 3"
"alt+shift+4" = "send-to-workspace 4"
"alt+shift+5" = "send-to-workspace 5"
"alt+shift+6" = "send-to-workspace 6"
"alt+shift+7" = "send-to-workspace 7"
"alt+shift+8" = "send-to-workspace 8"
"alt+shift+9" = "send-to-workspace 9"
"alt+shift+space" = "toggle-float"
"alt+shift+c" = "close"
"alt+shift+t" = "retile"
"alt+shift+r" = "reload"
"alt+shift+q" = "quit"

# Rules: the first match wins. Matchers: exe (file name, case-insensitive),
# class (exact), title (regex). Actions: float, tile, ignore, workspace N.
[[rules]]
title = "(?i)^picture.in.picture$"
action = "float"

# [[rules]]
# exe = "Spotify.exe"
# action = "workspace 9"
"#;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GapsConfig {
    #[serde(default)]
    pub outer: i32,
    #[serde(default)]
    pub inner: i32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    workspaces: Option<usize>,
    max_tiles: Option<usize>,
    insert: Option<String>,
    overflow: Option<String>,
    stack_monitor: Option<String>,
    laptop_layout: Option<String>,
    resize_step: Option<f32>,
    gaps: Option<GapsConfig>,
    keys: Option<BTreeMap<String, String>>,
    rules: Option<Vec<RuleSpec>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub workspaces: usize,
    pub max_tiles: usize,
    pub insert: InsertMode,
    pub overflow: OverflowMode,
    /// "auto" or a monitor device name.
    pub stack_monitor: String,
    /// `laptop_layout = "stack"`: the stack monitor always stacks.
    pub laptop_stack: bool,
    pub resize_step: f32,
    pub gaps: Gaps,
    pub keys: BTreeMap<String, String>,
    pub rules: Vec<RuleSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub key: String,
    pub hotkey: Hotkey,
    pub command: Command,
}

impl Default for Config {
    fn default() -> Self {
        Config::parse(DEFAULT_CONFIG).expect("built-in default config must parse")
    }
}

impl Config {
    /// Parse a config. Missing sections fall back to the defaults; a section that is
    /// present (e.g. `[keys]`) replaces the default section entirely.
    pub fn parse(text: &str) -> Result<Config, String> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        // The default config is parsed without a base (it *is* the base).
        let base = (text != DEFAULT_CONFIG).then(Config::default);
        let cfg = Config {
            workspaces: raw
                .workspaces
                .or(base.as_ref().map(|b| b.workspaces))
                .unwrap_or(9),
            max_tiles: raw
                .max_tiles
                .or(base.as_ref().map(|b| b.max_tiles))
                .unwrap_or(0),
            insert: match raw.insert.as_deref() {
                Some("largest") => InsertMode::Largest,
                Some("focused") => InsertMode::Focused,
                Some(other) => {
                    return Err(format!(
                        "insert must be \"largest\" or \"focused\", not \"{other}\""
                    ))
                }
                None => base.as_ref().map_or(InsertMode::Focused, |b| b.insert),
            },
            overflow: match raw.overflow.as_deref() {
                Some("laptop-stack") => OverflowMode::Stack,
                Some("next-workspace") => OverflowMode::NextWorkspace,
                Some(other) => {
                    return Err(format!(
                        "overflow must be \"laptop-stack\" or \"next-workspace\", not \"{other}\""
                    ))
                }
                None => base
                    .as_ref()
                    .map_or(OverflowMode::NextWorkspace, |b| b.overflow),
            },
            stack_monitor: raw
                .stack_monitor
                .or(base.as_ref().map(|b| b.stack_monitor.clone()))
                .unwrap_or_else(|| "auto".into()),
            laptop_stack: match raw.laptop_layout.as_deref() {
                Some("stack") => true,
                Some("tile") => false,
                Some(other) => {
                    return Err(format!(
                        "laptop_layout must be \"tile\" or \"stack\", not \"{other}\""
                    ))
                }
                None => base.as_ref().is_some_and(|b| b.laptop_stack),
            },
            resize_step: raw
                .resize_step
                .or(base.as_ref().map(|b| b.resize_step))
                .unwrap_or(0.05),
            gaps: raw
                .gaps
                .map(|g| Gaps {
                    outer: g.outer,
                    inner: g.inner,
                })
                .or(base.as_ref().map(|b| b.gaps))
                .unwrap_or_default(),
            keys: raw
                .keys
                .or(base.as_ref().map(|b| b.keys.clone()))
                .unwrap_or_default(),
            rules: raw
                .rules
                .or(base.as_ref().map(|b| b.rules.clone()))
                .unwrap_or_default(),
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        if !(1..=20).contains(&self.workspaces) {
            return Err("workspaces must be between 1 and 20".into());
        }
        if self.max_tiles > 64 {
            return Err("max_tiles must be between 0 (unlimited) and 64".into());
        }
        if !(0.01..=0.5).contains(&self.resize_step) {
            return Err("resize_step must be between 0.01 and 0.5".into());
        }
        if self.gaps.outer < 0 || self.gaps.inner < 0 {
            return Err("gaps must not be negative".into());
        }
        self.bindings()?;
        RuleSet::compile(&self.rules)?;
        Ok(())
    }

    pub fn bindings(&self) -> Result<Vec<Binding>, String> {
        let mut out: Vec<Binding> = Vec::new();
        for (key, cmd) in &self.keys {
            let hotkey = hotkey::parse(key)?;
            let command: Command = cmd.parse()?;
            if let Some(dup) = out.iter().find(|b| b.hotkey == hotkey) {
                return Err(format!("'{key}' and '{}' are the same hotkey", dup.key));
            }
            out.push(Binding {
                key: key.clone(),
                hotkey,
                command,
            });
        }
        Ok(out)
    }

    pub fn rule_set(&self) -> RuleSet {
        RuleSet::compile(&self.rules).expect("validated at parse time")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Direction;

    #[test]
    fn default_parses_with_expected_values() {
        let c = Config::default();
        assert_eq!(c.workspaces, 9);
        assert_eq!(c.max_tiles, 3);
        assert_eq!(c.overflow, OverflowMode::Stack);
        assert_eq!(c.stack_monitor, "auto");
        assert!(!c.laptop_stack);
        assert!(
            Config::parse("laptop_layout = \"stack\"")
                .unwrap()
                .laptop_stack
        );
        assert!(Config::parse("laptop_layout = \"grid\"").is_err());
        assert_eq!(c.insert, InsertMode::Largest);
        assert_eq!(c.gaps, Gaps { outer: 8, inner: 8 });
        let b = c.bindings().unwrap();
        assert!(b
            .iter()
            .any(|b| b.key == "alt+h" && b.command == Command::Focus(Direction::Left)));
        assert_eq!(b.len(), 35);
    }

    #[test]
    fn missing_sections_fall_back_to_defaults() {
        let c = Config::parse("[gaps]\nouter = 0\ninner = 2\n").unwrap();
        assert_eq!(c.gaps, Gaps { outer: 0, inner: 2 });
        assert_eq!(c.keys, Config::default().keys);
    }

    #[test]
    fn present_section_replaces_default() {
        let c = Config::parse("[keys]\n\"win+q\" = \"quit\"\n").unwrap();
        assert_eq!(c.keys.len(), 1);
    }

    #[test]
    fn rejects_invalid() {
        assert!(Config::parse("workspaces = 0").is_err());
        assert!(Config::parse("insert = \"random\"").is_err());
        assert!(Config::parse("max_tiles = 100").is_err());
        assert!(Config::parse("overflow = \"bin\"").is_err());
        assert_eq!(
            Config::parse("stack_monitor = '\\\\.\\DISPLAY2'")
                .unwrap()
                .stack_monitor,
            r"\\.\DISPLAY2"
        );
        assert!(Config::parse("[keys]\n\"alt+h\" = \"fly\"").is_err());
        assert!(Config::parse("[keys]\n\"alt+h\" = \"quit\"\n\"Alt+H\" = \"close\"").is_err());
        assert!(Config::parse("unknown_field = 1").is_err());
        assert!(Config::parse("[[rules]]\naction = \"float\"").is_err());
    }
}
