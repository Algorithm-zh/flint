use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use walkdir::WalkDir;

use crate::model::{Entry, Kind};
use crate::paths;

/// Directories that never contain anything a launcher should surface, and that
/// would cost a fortune to walk.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".svn",
    ".hg",
    "target",
    "obj",
    "__pycache__",
    ".venv",
    "venv",
    ".cache",
    ".next",
    ".nuxt",
    "AppData",
    "$RECYCLE.BIN",
    "System Volume Information",
    "Windows",
    "$WinREAgent",
    ".gradle",
    ".cargo",
    ".rustup",
    "site-packages",
    // download-manager scratch space: thousands of hash-named partial files
    "BaiduNetdiskTmp",
    "BaiduNetdiskDownload",
    "Thunder Network",
    "Temp",
    "tmp",
    "temp",
    "Cache",
    "cache",
    "Crashpad",
    "CrashDumps",
];

/// Half-written or machine-named artifacts that nobody ever wants to launch.
const SKIP_EXT: &[&str] = &[
    "tmp", "temp", "crdownload", "part", "partial", "download", "!ut", "bak", "swp", "dmp",
];

const MAX_DEPTH: usize = 6;

/// Windows shell APIs (`SHCreateItemFromParsingName`, `ShellExecute`) reject
/// paths that mix `/` and `\` with E_INVALIDARG, so every path handed to the
/// shell is normalised to backslashes.
pub fn win_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.contains('/') {
        s.replace('/', "\\")
    } else {
        s.into_owned()
    }
}

/// Shorthands people actually type. Fuzzy matching alone can't bridge
/// "vscode" → "Visual Studio Code"; an explicit alias can.
const APP_ALIASES: &[(&str, &str)] = &[
    ("visual studio code", "vscode code vs code"),
    ("visual studio", "vs"),
    ("google chrome", "chrome"),
    ("microsoft edge", "edge browser"),
    ("windows terminal", "wt terminal"),
    ("task manager", "taskmgr"),
    ("command prompt", "cmd"),
    ("windows powershell", "powershell ps"),
    ("registry editor", "regedit"),
    ("snipping tool", "screenshot snip"),
    ("control panel", "controlpanel"),
    ("file explorer", "explorer files"),
    ("windows explorer", "explorer files"),
    ("calculator", "calc"),
    ("notepad", "txt"),
    ("paint", "mspaint draw"),
    ("microsoft store", "store"),
    ("media player", "video music"),
    ("character map", "charmap"),
    ("system information", "msinfo32"),
    ("event viewer", "eventvwr"),
    ("services", "services.msc"),
    ("disk management", "diskmgmt"),
    ("device manager", "devmgmt"),
    ("resource monitor", "resmon"),
    ("performance monitor", "perfmon"),
    ("word", "docx doc"),
    ("excel", "xlsx sheet"),
    ("powerpoint", "pptx slides"),
    ("obsidian", "notes"),
    ("wechat", "weixin"),
    ("qq", "tencent"),
    ("netease cloud music", "music"),
];

/// First letters of each word: "Visual Studio Code" -> "vsc".
/// ASCII-only on purpose, so Chinese names don't produce noise.
fn initials_alias(name: &str) -> String {
    let mut out = String::new();
    let mut at_boundary = true;
    for ch in name.chars() {
        let is_sep = ch.is_whitespace() || matches!(ch, '-' | '_' | '(' | '[' | '/');
        if is_sep {
            at_boundary = true;
            continue;
        }
        if at_boundary && ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        }
        at_boundary = false;
    }
    if out.chars().count() >= 2 {
        out
    } else {
        String::new()
    }
}

fn app_alias(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut parts: Vec<String> = Vec::new();

    for (key, extra) in APP_ALIASES {
        if lower == *key || lower.starts_with(key) {
            parts.push((*extra).to_string());
            break;
        }
    }
    let ini = initials_alias(name);
    if !ini.is_empty() {
        parts.push(ini);
    }
    parts.join(" ")
}

/// Pinyin spellings for a Chinese name: full syllables plus initials, so both
/// `jisuanqi` and `jsq` find 计算器.
fn pinyin_alias(name: &str) -> String {
    use pinyin::ToPinyin;

    let has_cjk = name.chars().any(|c| (0x3400..=0x9FFF).contains(&(c as u32)));
    if !has_cjk {
        return String::new();
    }

    let mut full = String::new();
    let mut initials = String::new();
    for item in name.to_pinyin() {
        let Some(py) = item else { continue };
        let s = py.plain();
        if s.is_empty() {
            continue;
        }
        full.push_str(s);
        full.push(' ');
        if let Some(c) = s.chars().next() {
            initials.push(c);
        }
    }
    if initials.is_empty() {
        full
    } else {
        format!("{}{}", full, initials)
    }
}

/// `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App` -> `Microsoft Windows Calculator`
/// so English words in a package id are searchable without the hash suffix.
fn aumid_alias(id: &str) -> String {
    let base = id.split('!').next().unwrap_or(id);
    let base = base.split('_').next().unwrap_or(base);
    // Split CamelCase into words: WindowsCalculator -> Windows Calculator
    let mut out = String::with_capacity(base.len() + 8);
    let mut prev_lower = false;
    for ch in base.chars() {
        if ch == '.' || ch == '-' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = ch.is_lowercase() || ch.is_numeric();
        out.push(ch);
    }
    out
}

pub fn default_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = dirs::desktop_dir() {
        v.push(p);
    }
    if let Some(p) = dirs::download_dir() {
        v.push(p);
    }
    if let Some(p) = dirs::document_dir() {
        v.push(p);
    }
    v
}

/// Applications only — start-menu shortcuts plus everything the shell knows
/// about. Fast enough to run before the user notices.
pub fn build_apps() -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(512);
    let mut seen: HashSet<String> = HashSet::with_capacity(512);

    // Start-menu shortcuts first: they carry real file paths, which make for
    // better hints and more reliable launches than AUMIDs.
    for e in start_menu_lnks() {
        if seen.insert(e.name.to_lowercase()) {
            out.push(e);
        }
    }
    // Everything registered with the shell (covers UWP / Store apps).
    for e in shell_apps() {
        if seen.insert(e.name.to_lowercase()) {
            out.push(e);
        }
    }
    out
}

/// Files and folders under the configured roots.
pub fn build_files(roots: &[PathBuf], budget: usize, out: &mut Vec<Entry>) {
    let per_root = (budget / roots.len().max(1)).max(2_000);
    for r in roots {
        scan_root(r, per_root, out);
    }
}

/// Build the full entry list from every source.
#[allow(dead_code)]
pub fn build_all(roots: &[PathBuf], max_files: usize) -> Vec<Entry> {
    let mut out = build_apps();
    build_files(roots, max_files, &mut out);
    out
}

/// `Get-StartApps` is the cheapest reliable way to enumerate every app the
/// shell knows about, including UWP. It costs one process spawn, so the result
/// gets cached on disk.
fn shell_apps() -> Vec<Entry> {
    let script = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
                  Get-StartApps | Select-Object Name,AppID | ConvertTo-Json -Compress";

    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let out = match cmd.output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let json: Value = match serde_json::from_str(text.trim()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let items: Vec<&Value> = match &json {
        Value::Array(a) => a.iter().collect(),
        v @ Value::Object(_) => vec![v],
        _ => return Vec::new(),
    };

    let mut v = Vec::with_capacity(items.len());
    for it in items {
        let name = it.get("Name").and_then(|x| x.as_str()).unwrap_or("").trim();
        let id = it.get("AppID").and_then(|x| x.as_str()).unwrap_or("").trim();
        if name.is_empty() || id.is_empty() {
            continue;
        }
        // Skip the shell's own scaffolding entries.
        if name.eq_ignore_ascii_case("Program Manager") {
            continue;
        }
        let parse = format!("shell:AppsFolder\\{id}");

        let mut alias = pinyin_alias(name);
        let eng = aumid_alias(id);
        if !eng.is_empty() {
            if !alias.is_empty() {
                alias.push(' ');
            }
            alias.push_str(&eng);
        }
        let extra = app_alias(name);
        if !extra.is_empty() {
            if !alias.is_empty() {
                alias.push(' ');
            }
            alias.push_str(&extra);
        }

        v.push(Entry {
            name: name.to_string(),
            alias,
            path: parse.clone(),
            icon_src: parse,
            kind: Kind::App,
            ext: "APP".into(),
            hint: String::new(),
        });
    }
    v
}

fn start_menu_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let tail = ["Microsoft", "Windows", "Start Menu", "Programs"];
    if let Ok(appdata) = std::env::var("APPDATA") {
        let mut p = PathBuf::from(appdata);
        for t in tail {
            p.push(t);
        }
        v.push(p);
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        let mut p = PathBuf::from(pd);
        for t in tail {
            p.push(t);
        }
        v.push(p);
    }
    v.into_iter().filter(|p| p.is_dir()).collect()
}

fn start_menu_lnks() -> Vec<Entry> {
    let mut v = Vec::new();
    for dir in start_menu_dirs() {
        for entry in WalkDir::new(&dir)
            .max_depth(5)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext != "lnk" && ext != "url" {
                continue;
            }
            let stem = match p.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.trim(),
                None => continue,
            };
            if stem.is_empty() || stem.starts_with("Uninstall") || stem.starts_with("卸载") {
                continue;
            }
            // Group folder, e.g. "Accessories", makes a useful hint.
            let hint = p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            let full = win_path(p);
            let mut alias = pinyin_alias(stem);
            let extra = app_alias(stem);
            if !extra.is_empty() {
                if !alias.is_empty() {
                    alias.push(' ');
                }
                alias.push_str(&extra);
            }
            v.push(Entry {
                name: stem.to_string(),
                alias,
                path: full.clone(),
                icon_src: full,
                kind: Kind::App,
                ext: "APP".into(),
                hint: if hint.eq_ignore_ascii_case("Programs") {
                    String::new()
                } else {
                    hint
                },
            });
        }
    }
    v
}

fn scan_root(root: &Path, budget: usize, out: &mut Vec<Entry>) {
    if !root.is_dir() {
        return;
    }
    let mut added = 0usize;

    for entry in WalkDir::new(root)
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = match e.file_name().to_str() {
                Some(s) => s,
                None => return false,
            };
            if name.starts_with('.') || name.starts_with('~') {
                return false;
            }
            if e.file_type().is_dir() && SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(name)) {
                return false;
            }
            true
        })
        .filter_map(|e| e.ok())
    {
        if added >= budget {
            break;
        }
        let p = entry.path();
        if entry.depth() == 0 {
            continue;
        }

        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };

        let is_dir = entry.file_type().is_dir();

        let ext_raw = if is_dir {
            String::new()
        } else {
            p.extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default()
        };

        if !is_dir && SKIP_EXT.iter().any(|s| *s == ext_raw) {
            continue;
        }
        // `...node` / `.node` style sidecar files from Node tooling are noise.
        if !is_dir && ext_raw == "node" {
            continue;
        }

        let kind = if is_dir { Kind::Folder } else { Kind::File };

        let ext = if is_dir {
            "DIR".to_string()
        } else if ext_raw.is_empty() {
            "FILE".to_string()
        } else {
            ext_raw.to_ascii_uppercase()
        };

        // Hint: nearest parent, shortened to keep the row tidy.
        let hint = p
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let hint = if hint.is_empty() {
            paths::shorten(&win_path(root))
        } else {
            hint
        };

        let full = win_path(p);
        out.push(Entry {
            name: name.to_string(),
            alias: String::new(),
            path: full.clone(),
            icon_src: full,
            kind,
            ext,
            hint,
        });
        added += 1;
    }
}
