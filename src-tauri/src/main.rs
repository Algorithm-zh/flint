// Flint — a launcher that opens in a blink.
//
// Design notes:
// * One long-lived process. The window is created hidden and never destroyed,
//   so summoning it is a `ShowWindow` away instead of a cold start.
// * The index lives in memory and is rebuilt on a background thread. Search is
//   a lock-free read of an `Arc<Index>` snapshot plus a parallel fuzzy sweep.
// * Nothing on the hot path allocates more than it has to.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icons;
mod index;
mod launch;
mod model;
mod paths;
mod platform;
mod search;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use model::{Entry, Freq, Hit};

pub struct AppState {
    idx: RwLock<Arc<search::Index>>,
    freq: RwLock<FxHashMap<String, Freq>>,
    building: AtomicBool,
    /// Timestamp (ms) of the last time we summoned the palette. Used to ignore
    /// the focus-lost event that Windows fires while the window is still
    /// settling, which would otherwise hide it the instant it appears.
    last_show: AtomicU64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/* ------------------------------------------------------------------ */
/* frequency bookkeeping                                              */
/* ------------------------------------------------------------------ */

fn load_freq() -> FxHashMap<String, Freq> {
    std::fs::read_to_string(paths::freq_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_freq(map: &FxHashMap<String, Freq>) {
    if let Ok(s) = serde_json::to_string(map) {
        let _ = std::fs::write(paths::freq_file(), s);
    }
}

/// Counts with a recency decay, so a long-abandoned favourite stops dominating
/// the empty-query list.
fn counts(map: &FxHashMap<String, Freq>) -> FxHashMap<String, u32> {
    let now = now_secs();
    let mut out = FxHashMap::with_capacity_and_hasher(map.len(), Default::default());
    for (k, f) in map {
        if f.count == 0 {
            continue;
        }
        let age_days = now.saturating_sub(f.last) as f64 / 86_400.0;
        let decay = if age_days < 3.0 {
            1.0
        } else if age_days < 14.0 {
            0.75
        } else if age_days < 45.0 {
            0.5
        } else {
            0.3
        };
        let v = ((f.count as f64) * decay).round().max(1.0) as u32;
        out.insert(k.clone(), v);
    }
    out
}

fn rebuild_with_freq(state: &AppState, freq: &FxHashMap<String, Freq>) {
    let cur = state.idx.read();
    let next = cur.with_freq(counts(freq));
    drop(cur);
    *state.idx.write() = Arc::new(next);
}

fn bump(state: &AppState, path: &str) {
    let snapshot = {
        let mut map = state.freq.write();
        let e = map.entry(path.to_string()).or_default();
        e.count = e.count.saturating_add(1);
        e.last = now_secs();
        map.clone()
    };
    save_freq(&snapshot);
    rebuild_with_freq(state, &snapshot);
}

/* ------------------------------------------------------------------ */
/* indexing                                                           */
/* ------------------------------------------------------------------ */

/// Bump whenever `Entry` changes shape, so stale snapshots are ignored instead
/// of silently producing half-empty results.
const CACHE_VERSION: u32 = 2;

fn cache_load() -> Option<Vec<Entry>> {
    let bytes = std::fs::read(paths::index_cache()).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    let (version, entries): (u32, Vec<Entry>) = bincode::deserialize(&bytes).ok()?;
    if version != CACHE_VERSION {
        return None;
    }
    Some(entries)
}

fn cache_store(entries: &[Entry]) {
    if let Ok(bytes) = bincode::serialize(&(CACHE_VERSION, entries)) {
        let tmp = paths::index_cache().with_extension("tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, paths::index_cache());
        }
    }
}

fn spawn_index_build(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.building.swap(true, Ordering::SeqCst) {
        return;
    }
    drop(state);

    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let settings = paths::load_settings();
        let roots = settings.root_paths();

        // --- phase 1: apps ------------------------------------------------
        // Available in well under a second, so the user can launch something
        // long before the disk walk finishes.
        let apps = index::build_apps();
        {
            let st = app.state::<AppState>();
            let freq = counts(&st.freq.read());
            *st.idx.write() = Arc::new(search::Index::new(Arc::new(apps.clone()), freq));
        }
        let _ = app.emit("flint:indexed", apps.len());
        eprintln!("[flint] apps ready: {} in {:?}", apps.len(), t0.elapsed());

        // --- phase 2: files -----------------------------------------------
        let mut all = apps;
        index::build_files(&roots, settings.max_files.max(2_000), &mut all);
        let n = all.len();
        cache_store(&all);
        eprintln!("[flint] files done: {} total in {:?}", n, t0.elapsed());

        let shared = Arc::new(all);
        {
            let st = app.state::<AppState>();
            let freq = counts(&st.freq.read());
            *st.idx.write() = Arc::new(search::Index::new(shared.clone(), freq));
            st.building.store(false, Ordering::SeqCst);
        }
        let _ = app.emit("flint:indexed", n);

        // Warm the icon cache for the head of the list so the first summon is
        // already showing real icons rather than placeholder glyphs.
        let mut warmed = 0;
        for e in shared.iter() {
            if e.kind != model::Kind::App {
                continue;
            }
            let _ = icons::data_url(&e.icon_src);
            warmed += 1;
            if warmed >= 120 {
                break;
            }
        }
    });
}

/* ------------------------------------------------------------------ */
/* window plumbing                                                    */
/* ------------------------------------------------------------------ */

/// WebView2 keeps keyboard focus on its own child HWND. Raising the parent
/// window is not enough: without moving focus into the controller, the first
/// keys a user types after summoning the palette go nowhere.
///
/// This must not run while the webview is still initialising — calling into the
/// controller mid-creation leaves the view blank — so it is only ever invoked
/// from `show_palette`, after the window has been raised.
fn focus_webview(win: &WebviewWindow) {
    let _ = win.with_webview(|wv| {
        use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
        unsafe {
            let _ = wv
                .controller()
                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
    });
}

fn show_palette(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    app.state::<AppState>()
        .last_show
        .store(now_ms(), Ordering::Relaxed);
    // The UI resets itself on blur, while it is still hidden, so by the time we
    // show the window there is nothing left to paint but the answer.
    let _ = win.show();
    let _ = win.unminimize();
    if let Ok(h) = win.hwnd() {
        platform::force_foreground(h.0 as isize);
    }
    let _ = win.set_focus();
    focus_webview(&win);

    // Windows sometimes hands focus back after the window settles; a second,
    // later nudge catches that case without racing the initial paint.
    let app2 = app.clone();
    let app3 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(120));
        let _ = app2.run_on_main_thread(move || {
            if let Some(w) = app3.get_webview_window("main") {
                if w.is_visible().unwrap_or(false) {
                    focus_webview(&w);
                }
            }
        });
    });
}

fn toggle_palette(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    match win.is_visible() {
        Ok(true) => {
            let _ = win.hide();
        }
        _ => show_palette(app),
    }
}

/* ------------------------------------------------------------------ */
/* commands                                                           */
/* ------------------------------------------------------------------ */

#[tauri::command]
async fn search(state: State<'_, AppState>, q: String, limit: Option<usize>) -> Result<Vec<Hit>, String> {
    let idx = state.idx.read().clone();
    let lim = limit.unwrap_or(40).clamp(1, 200);
    Ok(idx.search(&q, lim))
}

#[tauri::command]
async fn launch(app: AppHandle, path: String) -> Result<(), String> {
    let result = launch::open(&path);
    if result.is_ok() {
        let state = app.state::<AppState>();
        bump(&state, &path);
    }
    result
}

#[tauri::command]
async fn reveal(path: String) -> Result<(), String> {
    launch::reveal(&path)
}

#[tauri::command]
fn dismiss(window: WebviewWindow) {
    let _ = window.hide();
}

#[tauri::command]
async fn icons(sources: Vec<String>) -> Result<Vec<Option<String>>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        sources
            .iter()
            .map(|s| crate::icons::data_url(s))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn reindex(app: AppHandle) -> bool {
    spawn_index_build(app);
    true
}

#[tauri::command]
fn stats(state: State<'_, AppState>) -> serde_json::Value {
    let idx = state.idx.read();
    let apps = idx
        .entries
        .iter()
        .filter(|e| e.kind == model::Kind::App)
        .count();
    serde_json::json!({ "total": idx.len(), "apps": apps })
}

/* ------------------------------------------------------------------ */
/* autostart                                                          */
/* ------------------------------------------------------------------ */

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn quiet(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

fn is_autostart() -> bool {
    let mut cmd = std::process::Command::new("reg");
    cmd.args(["query", RUN_KEY, "/v", "Flint"]);
    matches!(quiet(&mut cmd).output(), Ok(o) if o.status.success())
}

fn set_autostart(enable: bool) -> bool {
    let mut cmd = std::process::Command::new("reg");
    if enable {
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        cmd.args([
            "add",
            RUN_KEY,
            "/v",
            "Flint",
            "/t",
            "REG_SZ",
            "/d",
            &format!("\"{}\"", exe),
            "/f",
        ]);
    } else {
        cmd.args(["delete", RUN_KEY, "/v", "Flint", "/f"]);
    }
    match quiet(&mut cmd).output() {
        Ok(o) => o.status.success() || !enable,
        Err(_) => false,
    }
}

/* ------------------------------------------------------------------ */
/* entry point                                                        */
/* ------------------------------------------------------------------ */

fn main() {
    let state = AppState {
        idx: RwLock::new(Arc::new(search::Index::empty())),
        freq: RwLock::new(load_freq()),
        building: AtomicBool::new(false),
        last_show: AtomicU64::new(0),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_palette(app);
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle_palette(app);
                    }
                })
                .build(),
        )
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            search, launch, reveal, dismiss, icons, reindex, stats
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // --- window ---------------------------------------------------
            let win = app.get_webview_window("main").expect("main window");
            if let Ok(h) = win.hwnd() {
                platform::hide_from_switcher(h.0 as isize);
            }

            // --- hotkey ---------------------------------------------------
            let shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
            if let Err(e) = app.global_shortcut().register(shortcut) {
                eprintln!("[flint] failed to register Alt+Space: {e}");
            }

            // --- tray -----------------------------------------------------
            let show_i = MenuItem::with_id(app, "show", "打开 Flint", true, None::<&str>)?;
            let auto_i = CheckMenuItem::with_id(
                app,
                "autostart",
                "开机自动启动",
                true,
                is_autostart(),
                None::<&str>,
            )?;
            let re_i = MenuItem::with_id(app, "reindex", "重建索引", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &auto_i, &re_i, &quit_i])?;

            let auto_handle = auto_i.clone();
            let mut tray = TrayIconBuilder::with_id("flint-tray")
                .tooltip("Flint — Alt+Space")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, ev| match ev.id.as_ref() {
                    "show" => show_palette(app),
                    "autostart" => {
                        let next = !is_autostart();
                        if set_autostart(next) {
                            let _ = auto_handle.set_checked(next);
                        }
                    }
                    "reindex" => spawn_index_build(app.clone()),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = ev
                    {
                        show_palette(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            // --- instant start from the on-disk snapshot ------------------
            if let Some(entries) = cache_load() {
                let n = entries.len();
                let st = app.state::<AppState>();
                let freq = counts(&st.freq.read());
                *st.idx.write() = Arc::new(search::Index::new(Arc::new(entries), freq));
                let _ = app.emit("flint:indexed", n);
            }

            // --- refresh in the background --------------------------------
            spawn_index_build(handle.clone());

            // Dev affordance: `FLINT_SHOW=1` opens the palette shortly after
            // launch instead of waiting for the hotkey.
            if std::env::var_os("FLINT_SHOW").is_some() {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1200));
                    show_palette(&h);
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Any time the window becomes foreground — hotkey, tray, alt-tab —
            // focus has to end up inside the webview or the user's first
            // keystroke is swallowed.
            //
            // Two hazards, both handled here:
            //  * calling into the controller while the view is still being
            //    created blanks it, so the call is deferred;
            //  * MoveFocus itself re-raises this event, so without the
            //    debounce below it loops forever and the caret never settles.
            WindowEvent::Focused(true) => {
                static LAST: AtomicU64 = AtomicU64::new(0);
                let now = now_ms();
                if now.saturating_sub(LAST.load(Ordering::Relaxed)) < 800 {
                    return;
                }
                LAST.store(now, Ordering::Relaxed);

                let app = window.app_handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    let inner = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        if let Some(w) = inner.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                focus_webview(&w);
                            }
                        }
                    });
                });
            }
            // Clicking away dismisses the palette, like any good launcher.
            WindowEvent::Focused(false) => {
                static PINNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if *PINNED.get_or_init(|| std::env::var_os("FLINT_PIN").is_some()) {
                    return;
                }
                let state = window.app_handle().state::<AppState>();
                // Ignore the focus churn that happens while the window is
                // still being shown / gaining foreground rights.
                if now_ms().saturating_sub(state.last_show.load(Ordering::Relaxed)) < 400 {
                    return;
                }
                let _ = window.hide();
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("failed to start Flint");
}
