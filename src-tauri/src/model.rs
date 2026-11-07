use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    App,
    File,
    Folder,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::File => "file",
            Kind::Folder => "folder",
        }
    }

    /// Ranking bias. Apps should win ties against files with the same name.
    /// Files sit below zero on purpose: with thousands of them in the index,
    /// an equal-scoring file would otherwise crowd out the app the user meant.
    pub fn bias(self) -> i64 {
        match self {
            Kind::App => 260,
            Kind::Folder => -900,
            Kind::File => -2500,
        }
    }
}

/// One searchable thing (an app entry point or a file on disk).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    /// What the user sees and what gets fuzzy-matched.
    pub name: String,
    /// Extra text that also matches, but is never displayed:
    /// pinyin for Chinese names, the English package id for UWP apps.
    #[serde(default)]
    pub alias: String,
    /// What gets handed to the shell when the row is activated.
    pub path: String,
    /// Parsing name used to resolve the icon (may differ from `path` for UWP).
    pub icon_src: String,
    pub kind: Kind,
    /// Short uppercase badge, e.g. "PNG", "APP".
    pub ext: String,
    /// Right-aligned secondary text.
    pub hint: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Seg {
    pub t: String,
    pub h: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Hit {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub kind: &'static str,
    pub hint: String,
    pub icon_src: String,
    pub segs: Vec<Seg>,
}

/// Persisted launch-frequency record, used for frecency ranking.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Freq {
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub last: u64,
}
