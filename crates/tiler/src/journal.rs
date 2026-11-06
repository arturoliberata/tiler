//! Crash safety: every window the tiler hides is recorded on disk and in a global,
//! so a panic, Ctrl+C, console close or the next start can show it again.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::{window, Hwnd};

static HIDDEN: Mutex<Vec<isize>> = Mutex::new(Vec::new());

pub struct Journal {
    path: PathBuf,
}

impl Journal {
    pub fn new(path: PathBuf) -> Journal {
        Journal { path }
    }

    pub fn save(&self, hidden: &HashSet<Hwnd>) {
        let list: Vec<isize> = hidden.iter().map(|h| h.0).collect();
        if let Ok(mut g) = HIDDEN.lock() {
            g.clone_from(&list);
        }
        if let Err(e) = std::fs::write(&self.path, serde_json::to_string(&list).unwrap_or_default())
        {
            tracing::warn!("could not write journal {}: {e}", self.path.display());
        }
    }

    pub fn load(&self) -> Vec<Hwnd> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<isize>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .map(Hwnd)
            .collect()
    }

    /// Show every window a previous run left hidden. Returns how many were restored.
    pub fn restore_previous(&self) -> usize {
        let list = self.load();
        let n = list
            .iter()
            .filter(|h| window::is_window(**h))
            .inspect(|h| window::restore_minimized(**h))
            .count();
        self.save(&HashSet::new());
        n
    }
}

/// Emergency path for panic and console handlers. Must not allocate much or block.
pub fn restore_from_global() {
    let list = match HIDDEN.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    };
    for h in list {
        window::restore_minimized(Hwnd(h));
    }
}
