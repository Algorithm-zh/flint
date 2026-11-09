//! oh-my-pi (`omp`) integration.
//!
//! omp is a terminal agent, and Flint is a launcher, so the tempting mistake is
//! to host it in-process. We don't: a 250 MB agent must not live inside the
//! palette, or summoning it would cost the thing Flint is actually for.
//!
//! Two light integrations instead:
//!
//!  * `ask` spawns `omp -p` with `--mode json` and streams the deltas back to
//!    the palette. Flint's own cost is one `CreateProcess` and a reader thread;
//!    the agent runs and dies in the background.
//!  * `open_tui` hands a folder to a terminal window, so `omp` runs where it
//!    was designed to run.
//!
//! Nothing is cached, nothing is kept warm. If omp is not installed every entry
//! point simply reports that.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};

/// Where omp usually ends up on a machine that installed it by hand.
const KNOWN: &[&str] = &[
    r"D:\omp-windows-x64.exe",
    r"C:\omp\omp.exe",
    r"C:\Users\Public\omp\omp.exe",
];

/// Find the executable. Explicit setting first, then PATH, then known spots.
pub fn locate(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Some(p) = on_path("omp.exe").or_else(|| on_path("omp")) {
        return Some(p);
    }

    for c in KNOWN {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }

    // A couple of per-user install locations, checked last because they are
    // guesses rather than conventions.
    if let Some(base) = dirs::data_local_dir() {
        for rel in [r"omp\omp.exe", r"Programs\omp\omp.exe"] {
            let p = base.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Deserialize)]
struct Frame {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "assistantMessageEvent")]
    event: Option<AssistantEvent>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct AssistantEvent {
    #[serde(rename = "type")]
    kind: String,
    delta: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    /// Only the assistant's message_end carries the answer; the user's own
    /// echo fires the same event type and must be ignored.
    role: Option<String>,
    content: Option<Vec<Block>>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// Spawn a one-shot question and stream the answer into the palette.
///
/// The child is returned so the caller can keep it for cancellation; the reader
/// thread owns stdout and finishes the job.
pub fn ask_streaming(
    app: AppHandle,
    exe: &Path,
    prompt: String,
    cwd: &str,
) -> Result<Child, String> {
    let mut cmd = Command::new(exe);
    cmd.arg("--cwd")
        .arg(cwd)
        .arg("--allow-home")
        .arg("--mode")
        .arg("json")
        // Epic asks should not accumulate in the user's session history.
        .arg("--no-session")
        .arg("-p")
        .arg(&prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console flash: this runs behind the palette.
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd.spawn().map_err(|e| format!("启动 omp 失败：{e}"))?;
    let stdout = child.stdout.take().ok_or("omp 没有输出")?;
    let stderr = child.stderr.take();

    let app_err = app.clone();

    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut finished = false;

        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(frame) = serde_json::from_str::<Frame>(&line) else {
                continue;
            };

            match frame.kind.as_str() {
                "message_update" => {
                    if let Some(ev) = frame.event {
                        if ev.kind == "text_delta" {
                            if let Some(d) = ev.delta {
                                let _ = app.emit("flint:omp-delta", d);
                            }
                        }
                    }
                }
                "message_end" => {
                    // The final text is authoritative; streamed deltas can be
                    // partial if the model was interrupted mid-flush. Only the
                    // assistant message counts — the user's prompt echoes here
                    // too.
                    if let Some(m) = frame.message {
                        if m.role.as_deref() == Some("assistant") {
                            if let Some(blocks) = m.content {
                                let text: String = blocks
                                    .iter()
                                    .filter(|b| b.kind == "text")
                                    .filter_map(|b| b.text.clone())
                                    .collect();
                                if !text.is_empty() {
                                    let _ = app.emit("flint:omp-final", text);
                                }
                            }
                        }
                    }
                }
                "agent_end" => {
                    finished = true;
                    break;
                }
                _ => {}
            }
        }

        let _ = finished;
        let _ = app.emit("flint:omp-done", ());
    });

    // stderr is only interesting when the run fails; surface it as the answer
    // instead of hiding the reason in a console nobody sees.
    if let Some(err) = stderr {
        let app2 = app_err;
        std::thread::spawn(move || {
            let reader = BufReader::new(err);
            let mut buf = String::new();
            for line in reader.lines().flatten() {
                if buf.len() < 2000 {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
            if !buf.trim().is_empty() {
                let _ = app2.emit("flint:omp-error", buf);
            }
        });
    }

    Ok(child)
}

/* ------------------------------------------------------------------ */
/* terminal hand-off                                                   */
/* ------------------------------------------------------------------ */

/// Open a terminal in `dir` running the omp TUI.
pub fn open_tui(exe: &Path, dir: &str) -> Result<(), String> {
    // Windows Terminal keeps the TUI inside a proper console; the cmd fallback
    // covers machines where wt.exe is not installed.
    let mut wt = Command::new("wt.exe");
    wt.arg("-d").arg(dir).arg(exe).arg("--cwd").arg(dir);
    wt.arg("--allow-home");
    detach(&mut wt);
    if wt.spawn().is_ok() {
        return Ok(());
    }

    let mut fallback = Command::new("cmd.exe");
    fallback
        .arg("/c")
        .arg("start")
        .arg("")
        .arg("/D")
        .arg(dir)
        .arg(exe)
        .arg("--cwd")
        .arg(dir)
        .arg("--allow-home");
    detach(&mut fallback);
    fallback
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开终端失败：{e}"))
}

fn detach(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
}
