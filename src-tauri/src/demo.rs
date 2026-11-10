//! Screenshot data.
//!
//! The README needs pictures, and a real screenshot of this machine would show
//! the author's own folders, filenames and open windows. With `FLINT_DEMO=1`
//! the backend answers with a fixed, fictional set of entries instead: the UI
//! renders exactly as it normally does, but nothing on screen belongs to
//! anyone. Icons are pulled from programs that ship with Windows, so even the
//! images are neutral.
//!
//! This is off unless the environment variable is set, and it never touches the
//! on-disk index — `flint.exe` as shipped has no way to reach it.

use crate::model::{Hit, Kind, Seg};
use crate::paths::{Settings, Source};

pub fn enabled() -> bool {
    std::env::var_os("FLINT_DEMO").is_some()
}

struct Demo {
    name: &'static str,
    kind: Kind,
    ext: &'static str,
    hint: &'static str,
    path: &'static str,
    icon: &'static str,
}

fn entries() -> Vec<Demo> {
    vec![
        // Windows already on screen.
        Demo {
            name: "notes.md - Notepad",
            kind: Kind::Window,
            ext: "WIN",
            hint: "notepad.exe",
            path: "hwnd:1",
            icon: concat!(r"C:\Windows\System32", r"\notepad.exe"),
        },
        Demo {
            name: "Calculator",
            kind: Kind::Window,
            ext: "WIN",
            hint: "CalculatorApp.exe",
            path: "hwnd:2",
            icon: concat!(r"C:\Windows\System32", r"\calc.exe"),
        },
        Demo {
            name: "Downloads - File Explorer",
            kind: Kind::Window,
            ext: "WIN",
            hint: "explorer.exe",
            path: "hwnd:3",
            icon: concat!(r"C:\Windows", r"\explorer.exe"),
        },
        Demo {
            name: "Untitled - Notepad",
            kind: Kind::Window,
            ext: "WIN",
            hint: "notepad.exe",
            path: "hwnd:4",
            icon: concat!(r"C:\Windows\System32", r"\notepad.exe"),
        },
        // Applications.
        Demo {
            name: "Notepad",
            kind: Kind::App,
            ext: "APP",
            hint: "",
            path: concat!(r"C:\Windows\System32", r"\notepad.exe"),
            icon: concat!(r"C:\Windows\System32", r"\notepad.exe"),
        },
        Demo {
            name: "File Explorer",
            kind: Kind::App,
            ext: "APP",
            hint: "",
            path: concat!(r"C:\Windows", r"\explorer.exe"),
            icon: concat!(r"C:\Windows", r"\explorer.exe"),
        },
        Demo {
            name: "Calculator",
            kind: Kind::App,
            ext: "APP",
            hint: "",
            path: concat!(r"C:\Windows\System32", r"\calc.exe"),
            icon: concat!(r"C:\Windows\System32", r"\calc.exe"),
        },
        // Files.
        Demo {
            name: "notes.md",
            kind: Kind::File,
            ext: "MD",
            hint: r"~\Documents",
            path: r"~\Documents\notes.md",
            icon: "",
        },
        Demo {
            name: "meeting-notes.txt",
            kind: Kind::File,
            ext: "TXT",
            hint: r"~\Desktop",
            path: r"~\Desktop\meeting-notes.txt",
            icon: "",
        },
        Demo {
            name: "roadmap.pdf",
            kind: Kind::File,
            ext: "PDF",
            hint: r"~\Downloads",
            path: r"~\Downloads\roadmap.pdf",
            icon: "",
        },
    ]
}

/// Split `name` around the first case-insensitive occurrence of `q`.
fn segs_for(name: &str, q: &str) -> Option<Vec<Seg>> {
    let plain = vec![Seg {
        t: name.to_string(),
        h: false,
    }];
    if q.is_empty() {
        return Some(plain);
    }

    let lower = name.to_ascii_lowercase();
    let start = lower.find(q)?;
    let end = start + q.len();

    let mut out = Vec::with_capacity(3);
    if start > 0 {
        out.push(Seg {
            t: name[..start].to_string(),
            h: false,
        });
    }
    out.push(Seg {
        t: name[start..end].to_string(),
        h: true,
    });
    if end < name.len() {
        out.push(Seg {
            t: name[end..].to_string(),
            h: false,
        });
    }
    Some(out)
}

pub fn hits(query: &str, limit: usize) -> Vec<Hit> {
    let q = query.trim().to_ascii_lowercase();
    let mut out: Vec<Hit> = Vec::new();

    for e in entries() {
        let Some(segs) = segs_for(e.name, &q) else {
            continue;
        };
        out.push(Hit {
            path: e.path.to_string(),
            name: e.name.to_string(),
            ext: e.ext.to_string(),
            kind: e.kind.as_str(),
            hint: e.hint.to_string(),
            icon_src: e.icon.to_string(),
            segs,
        });
    }

    // Same shape as the real thing: windows first, then apps, then files.
    out.sort_by_key(|h| match h.kind {
        "window" => 0,
        "app" => 1,
        _ => 2,
    });
    out.truncate(limit);
    out
}

pub fn settings() -> Settings {
    Settings {
        roots: vec![
            r"C:\Projects".to_string(),
            r"D:\Archive".to_string(),
            r"~\Documents".to_string(),
        ],
        max_files: 120_000,
        sources: vec![Source {
            path: r"C:\Projects\flint".to_string(),
            keyword: "flint".to_string(),
            label: "当前项目".to_string(),
        }],
        keymap: "jk".to_string(),
        omp_path: String::new(),
    }
}

pub fn stats() -> serde_json::Value {
    serde_json::json!({ "total": entries().len(), "apps": 4 })
}
