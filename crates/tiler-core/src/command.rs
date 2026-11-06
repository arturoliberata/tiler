use std::fmt;
use std::str::FromStr;

use crate::geom::Direction;

/// Everything the user can ask the tiler to do. Hotkeys (and later IPC) map to these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Focus(Direction),
    Swap(Direction),
    /// Move the nearest divider touching the focused window in this direction.
    Resize(Direction),
    /// 1-based, as typed by the user.
    Workspace(usize),
    SendToWorkspace(usize),
    ToggleFloat,
    Close,
    Retile,
    Reload,
    Quit,
}

impl FromStr for Command {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<String> = s
            .split_whitespace()
            .map(|p| p.to_ascii_lowercase())
            .collect();
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        let dir = |p: Option<&&str>| {
            p.and_then(|d| Direction::parse(d))
                .ok_or_else(|| format!("'{s}': expected a direction (left/right/up/down)"))
        };
        let num = |p: Option<&&str>| {
            p.and_then(|n| n.parse::<usize>().ok())
                .filter(|n| *n >= 1)
                .ok_or_else(|| format!("'{s}': expected a workspace number starting at 1"))
        };
        match parts.first().copied() {
            Some("focus") => Ok(Command::Focus(dir(parts.get(1))?)),
            Some("swap") => Ok(Command::Swap(dir(parts.get(1))?)),
            Some("resize") => Ok(Command::Resize(dir(parts.get(1))?)),
            Some("workspace") => Ok(Command::Workspace(num(parts.get(1))?)),
            Some("send-to-workspace") => Ok(Command::SendToWorkspace(num(parts.get(1))?)),
            Some("toggle-float") => Ok(Command::ToggleFloat),
            Some("close") => Ok(Command::Close),
            Some("retile") => Ok(Command::Retile),
            Some("reload") => Ok(Command::Reload),
            Some("quit") => Ok(Command::Quit),
            _ => Err(format!("unknown command '{s}'")),
        }
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = |d: &Direction| format!("{d:?}").to_ascii_lowercase();
        match self {
            Command::Focus(x) => write!(f, "focus {}", d(x)),
            Command::Swap(x) => write!(f, "swap {}", d(x)),
            Command::Resize(x) => write!(f, "resize {}", d(x)),
            Command::Workspace(n) => write!(f, "workspace {n}"),
            Command::SendToWorkspace(n) => write!(f, "send-to-workspace {n}"),
            Command::ToggleFloat => write!(f, "toggle-float"),
            Command::Close => write!(f, "close"),
            Command::Retile => write!(f, "retile"),
            Command::Reload => write!(f, "reload"),
            Command::Quit => write!(f, "quit"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for s in [
            "focus left",
            "swap down",
            "resize right",
            "workspace 3",
            "send-to-workspace 9",
            "toggle-float",
            "close",
            "retile",
            "reload",
            "quit",
        ] {
            let c: Command = s.parse().unwrap();
            assert_eq!(c.to_string(), s);
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert!("focus sideways".parse::<Command>().is_err());
        assert!("workspace 0".parse::<Command>().is_err());
        assert!("dance".parse::<Command>().is_err());
    }
}
