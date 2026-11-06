//! Per-application rules: first matching rule wins.

use regex::Regex;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RuleSpec {
    /// Executable file name, case-insensitive, e.g. "Spotify.exe".
    pub exe: Option<String>,
    /// Exact window class name.
    pub class: Option<String>,
    /// Regex matched against the window title.
    pub title: Option<String>,
    /// "float", "tile", "ignore" or "workspace N".
    pub action: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleAction {
    /// Manage, but don't tile.
    Float,
    /// Tile even if the heuristics would float it.
    Tile,
    /// Don't manage at all.
    Ignore,
    /// Tile on this workspace (0-based) of the window's monitor.
    Workspace(usize),
}

#[derive(Debug)]
struct Rule {
    exe: Option<String>,
    class: Option<String>,
    title: Option<Regex>,
    action: RuleAction,
}

pub struct WindowInfo<'a> {
    pub exe: &'a str,
    pub class: &'a str,
    pub title: &'a str,
}

#[derive(Debug, Default)]
pub struct RuleSet(Vec<Rule>);

impl RuleSet {
    pub fn compile(specs: &[RuleSpec]) -> Result<RuleSet, String> {
        specs
            .iter()
            .map(|s| {
                if s.exe.is_none() && s.class.is_none() && s.title.is_none() {
                    return Err(format!(
                        "rule with action '{}' has no exe/class/title matcher",
                        s.action
                    ));
                }
                let title = s
                    .title
                    .as_deref()
                    .map(Regex::new)
                    .transpose()
                    .map_err(|e| format!("bad title regex: {e}"))?;
                Ok(Rule {
                    exe: s.exe.as_ref().map(|e| e.to_ascii_lowercase()),
                    class: s.class.clone(),
                    title,
                    action: parse_action(&s.action)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(RuleSet)
    }

    pub fn evaluate(&self, w: &WindowInfo) -> Option<RuleAction> {
        let exe = w.exe.to_ascii_lowercase();
        self.0
            .iter()
            .find(|r| {
                r.exe.as_ref().is_none_or(|e| *e == exe)
                    && r.class.as_ref().is_none_or(|c| c == w.class)
                    && r.title.as_ref().is_none_or(|t| t.is_match(w.title))
            })
            .map(|r| r.action)
    }
}

fn parse_action(s: &str) -> Result<RuleAction, String> {
    let lower = s.trim().to_ascii_lowercase();
    match lower.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["float"] => Ok(RuleAction::Float),
        ["tile"] => Ok(RuleAction::Tile),
        ["ignore"] => Ok(RuleAction::Ignore),
        ["workspace", n] => n
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .map(|n| RuleAction::Workspace(n - 1))
            .ok_or_else(|| format!("bad workspace number in rule action '{s}'")),
        _ => Err(format!("unknown rule action '{s}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(exe: Option<&str>, class: Option<&str>, title: Option<&str>, action: &str) -> RuleSpec {
        RuleSpec {
            exe: exe.map(Into::into),
            class: class.map(Into::into),
            title: title.map(Into::into),
            action: action.into(),
        }
    }

    #[test]
    fn first_match_wins_and_exe_is_case_insensitive() {
        let rs = RuleSet::compile(&[
            spec(Some("Spotify.exe"), None, None, "workspace 9"),
            spec(None, None, Some("(?i)picture.in.picture"), "float"),
            spec(Some("spotify.exe"), None, None, "ignore"),
        ])
        .unwrap();
        let w = |exe, title| WindowInfo {
            exe,
            class: "Chrome_WidgetWin_1",
            title,
        };
        assert_eq!(
            rs.evaluate(&w("SPOTIFY.EXE", "x")),
            Some(RuleAction::Workspace(8))
        );
        assert_eq!(
            rs.evaluate(&w("chrome.exe", "Picture in picture")),
            Some(RuleAction::Float)
        );
        assert_eq!(rs.evaluate(&w("chrome.exe", "Inbox")), None);
    }

    #[test]
    fn all_matchers_must_match() {
        let rs = RuleSet::compile(&[spec(Some("a.exe"), Some("Cls"), None, "float")]).unwrap();
        assert_eq!(
            rs.evaluate(&WindowInfo {
                exe: "a.exe",
                class: "Other",
                title: ""
            }),
            None
        );
        assert_eq!(
            rs.evaluate(&WindowInfo {
                exe: "a.exe",
                class: "Cls",
                title: ""
            }),
            Some(RuleAction::Float)
        );
    }

    #[test]
    fn rejects_bad_rules() {
        assert!(RuleSet::compile(&[spec(None, None, None, "float")]).is_err());
        assert!(RuleSet::compile(&[spec(Some("a"), None, None, "explode")]).is_err());
        assert!(RuleSet::compile(&[spec(None, None, Some("("), "float")]).is_err());
        assert!(RuleSet::compile(&[spec(Some("a"), None, None, "workspace 0")]).is_err());
    }
}
