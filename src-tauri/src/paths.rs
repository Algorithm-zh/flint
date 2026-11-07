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

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Settings {
    /// Folders to walk for files. Empty means "use the defaults".
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default = "default_budget")]
    pub max_files: usize,
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
        }
    }
}

impl Settings {
    pub fn root_paths(&self) -> Vec<PathBuf> {
        let v: Vec<PathBuf> = self
            .roots
            .iter()
            .map(|s| PathBuf::from(s))
            .filter(|p| p.is_dir())
            .collect();
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
