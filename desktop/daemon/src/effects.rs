//! Things the daemon does to the desktop on the phone's behalf (open a link, ring). Behind
//! a trait so tests can record them instead of opening a browser or making noise.

use std::sync::{Arc, Mutex};

pub trait Effects: Send + Sync {
    /// Open a web link (already checked with `is_web_link`) in the default browser.
    fn open_url(&self, url: &str) -> bool;
}

/// The real desktop.
pub struct System;

impl Effects for System {
    fn open_url(&self, url: &str) -> bool {
        crate::open::open_url(url)
    }
}

/// For tests: remembers what would have happened, e.g. `"open https://…"`.
#[derive(Default)]
pub struct Recorded {
    pub log: Mutex<Vec<String>>,
}

impl Recorded {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn note(&self, s: String) {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).push(s);
    }
}

impl Effects for Recorded {
    fn open_url(&self, url: &str) -> bool {
        self.note(format!("open {url}"));
        true
    }
}
