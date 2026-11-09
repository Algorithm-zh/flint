use std::path::PathBuf;

/// `%LOCALAPPDATA%\Flint`
pub fn data_dir() -> PathBuf {
    let mut p = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push("Flint");
    let _ = std::fs::create_dir_all(&p);
    p
}

pub fn index_cache() -> PathBuf {
    data_dir().join("index.bin")
}

pub fn freq_file() -> PathBuf {
    data_dir().join("freq.json")
}

pub fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn icon_dir() -> PathBuf {
    let p = data_dir().join("icons");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Collapse `C:\Users\bob\Desktop` to `~\Desktop` for display.
pub fn shorten(p: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    if !home.is_empty() {
        if let Some(rest) = p.strip_prefix(&home) {
            let rest = rest.trim_start_matches(['\\', '/']);
            return if rest.is_empty() {
                "~".to_string()
            } else {
                format!("~\\{}", rest)
            };
        }
    }
    p.to_string()
}

/* ------------------------------------------------------------------ */
/* settings                                                           */
/* ------------------------------------------------------------------ */

/// A folder the user added by hand, reachable by typing its keyword.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Source {
    pub path: String,
    /// What you type to jump straight here.
    #[serde(default)]
    pub keyword: String,
    /// Display name; falls back to the folder name.
    #[serde(default)]
    pub label: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Settings {
    /// Folders to walk for files. Empty means "use the defaults".
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default = "default_budget")]
    pub max_files: usize,
    /// Hand-added shortcuts: a folder plus the keyword that summons it.
    #[serde(default)]
    pub sources: Vec<Source>,
    /// How INSERT mode hands over to NORMAL mode.
    /// `"jk"` — type j then k quickly; `"esc"` — press Esc.
    #[serde(default = "default_keymap")]
    pub keymap: String,
    /// Where the omp executable lives. Empty means "search PATH and the usual
    /// install locations".
    #[serde(default)]
    pub omp_path: String,
}

fn default_keymap() -> String {
    "jk".to_string()
}

fn default_budget() -> usize {
    120_000
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            roots: crate::index::default_roots()
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            max_files: default_budget(),
            sources: Vec::new(),
            keymap: default_keymap(),
            omp_path: String::new(),
        }
    }
}

impl Settings {
    pub fn root_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .roots
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .collect();

        // Hand-added sources are indexed too — that's the whole point of
        // adding them; the keyword is just a shortcut on top.
        for s in &self.sources {
            let p = PathBuf::from(&s.path);
            if p.is_dir() && !v.iter().any(|x| x == &p) {
                v.push(p);
            }
        }

        if v.is_empty() {
            crate::index::default_roots()
        } else {
            v
        }
    }
}

pub fn load_settings() -> Settings {
    let f = settings_file();
    if let Ok(s) = std::fs::read_to_string(&f) {
        if let Ok(v) = serde_json::from_str::<Settings>(&s) {
            return v;
        }
    }
    let def = Settings::default();
    if let Ok(s) = serde_json::to_string_pretty(&def) {
        let _ = std::fs::write(&f, s);
    }
    def
}

pub fn write_settings(s: &Settings) -> bool {
    match serde_json::to_string_pretty(s) {
        Ok(body) => std::fs::write(settings_file(), body).is_ok(),
        Err(_) => false,
    }
}
