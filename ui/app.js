/* ============================================================
   Flint — UI layer
   Deliberately framework-free: no hydration, no vdom, no build
   step. The Rust side owns all the data; this file only paints
   ~40 rows and keeps the selection in sync.
   ============================================================ */
const core = window.__TAURI__.core;
const event = window.__TAURI__.event;
const invoke = core.invoke;

const $q = document.getElementById("q");
const $list = document.getElementById("list");
const $meta = document.getElementById("meta");
const $frame = document.getElementById("frame");
const $mode = document.getElementById("mode");
const $keys = document.getElementById("keys");

let hits = [];
let sel = 0;
let seq = 0;
let composing = false;
let indexed = false;

/* ---------- vim modes ----------
   INSERT is the default: type to search, arrows to move.
   Esc (with a non-empty query) drops into NORMAL, where j/k navigate and the
   input stops accepting characters. Esc again dismisses the palette.
   Ctrl+J / Ctrl+K work in both modes for people who don't want to think
   about modes at all. */
let mode = "insert";
let lastG = 0;

const KEYHINTS = {
  insert: `<span><kbd>↑</kbd><kbd>↓</kbd>选择</span>
           <span><kbd>↵</kbd>打开</span>
           <span><kbd>tab</kbd>动作</span>
           <span><kbd>esc</kbd>普通模式</span>`,
  normal: `<span><kbd>j</kbd><kbd>k</kbd>选择</span>
           <span><kbd>↵</kbd>打开</span>
           <span><kbd>tab</kbd>动作</span>
           <span><kbd>i</kbd>编辑</span>
           <span><kbd>esc</kbd>收起</span>`,
};

function setMode(next) {
  if (mode === next) return;
  mode = next;
  document.documentElement.dataset.mode = next;

  if (next === "normal") {
    $q.readOnly = true;
    $q.blur();
  } else {
    $q.readOnly = false;
    $q.focus();
    const v = $q.value;
    $q.value = "";
    $q.value = v; // caret to end
  }

  $mode.textContent = next === "normal" ? "NORMAL" : "INSERT";
  $keys.innerHTML = KEYHINTS[next];
  paintSelection();
}

function dismiss() {
  invoke("dismiss").catch(() => {});
}

/* icon source -> data URL */
const iconCache = new Map();

/* ---------- helpers ---------- */
const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[c]));

const SEG_SVG =
  '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" aria-hidden="true">' +
  '<path d="M6.1 2.6h3.8c2.2 0 3.5 1.2 3.5 3.4s-1.3 3.4-3.5 3.4H8v4h-1.9V2.6Z" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round"/>' +
  "</svg>";

const COMMAND_SVG =
  '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" aria-hidden="true">' +
  '<path d="M4 5.2 7 8l-3 2.8" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"/>' +
  '<path d="M8.4 11.4h3.6" stroke="currentColor" stroke-width="1.35" stroke-linecap="round"/>' +
  "</svg>";

const FOLDER_SVG =
  '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" aria-hidden="true">' +
  '<path d="M2.1 4.4c0-.6.5-1.1 1.1-1.1h2.6l1.4 1.5h5.6c.6 0 1.1.5 1.1 1.1v5.7c0 .6-.5 1.1-1.1 1.1H3.2c-.6 0-1.1-.5-1.1-1.1V4.4Z" ' +
  'stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/></svg>';

const FILE_SVG =
  '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" aria-hidden="true">' +
  '<path d="M4 2.4h5.2L12.6 5.8v7.8H4V2.4Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>' +
  '<path d="M9.1 2.6v3.3h3.3" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/></svg>';

const ACTION_SVG =
  '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" aria-hidden="true">' +
  '<path d="M5.6 3.4v9.2l7-4.6-7-4.6Z" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>' +
  "</svg>";

function glyphOf(h) {
  if (h.kind === "command") return COMMAND_SVG;
  if (h.kind === "action") return ACTION_SVG;
  if (h.kind === "folder" || h.kind === "bookmark") return FOLDER_SVG;
  if (h.kind === "file") return FILE_SVG;
  return SEG_SVG;
}

/* ---------- rendering ---------- */
function render() {
  if (!hits.length) {
    const term = $q.value.trim();
    $list.replaceChildren();
    const box = document.createElement("div");
    box.className = "empty";
    box.innerHTML = term
      ? `<div class="big">没有找到 <b>${esc(term)}</b></div><div>换个更短的关键词试试</div>`
      : `<div class="big">${indexed ? "还没有可搜索的内容" : "正在建立索引…"}</div>
         <div>${indexed ? "检查一下索引目录设置" : "首次启动需要几秒钟"}</div>`;
    $list.append(box);
    return;
  }

  const frag = document.createDocumentFragment();

  for (let i = 0; i < hits.length; i++) {
    const h = hits[i];
    const row = document.createElement("div");
    row.className = "row" + (i === sel ? " sel" : "");
    row.dataset.i = String(i);
    row.dataset.kind = h.kind;

    const cached = iconCache.get(h.icon_src);

    const ico = document.createElement("div");
    ico.className = "ico" + (cached ? " has-img" : "");
    if (cached) ico.style.backgroundImage = `url("${cached}")`;
    ico.innerHTML = `<span class="glyph">${glyphOf(h)}</span>`;

    const body = document.createElement("div");
    body.className = "body";

    const name = document.createElement("div");
    name.className = "name";
    name.innerHTML = h.segs
      .map((s) => (s.h ? `<b>${esc(s.t)}</b>` : esc(s.t)))
      .join("");

    body.append(name);
    if (h.hint) {
      const hint = document.createElement("div");
      hint.className = "hint";
      hint.textContent = h.hint;
      body.append(hint);
    }

    const badge = document.createElement("div");
    badge.className = "badge";
    badge.textContent = h.ext;

    row.append(ico, body, badge);
    frag.append(row);
  }

  $list.replaceChildren(frag);
  $list.scrollTop = 0;
  sel = 0;
}

function move(delta) {
  if (!hits.length) return;
  const next = sel + delta;
  if (next < 0) sel = hits.length - 1;
  else if (next >= hits.length) sel = 0;
  else sel = next;
  paintSelection();
}

function paintSelection() {
  const rows = $list.children;
  for (let i = 0; i < rows.length; i++) {
    const el = rows[i];
    if (!el.classList) continue;
    el.classList.toggle("sel", Number(el.dataset.i) === sel);
  }
  const cur = rows[sel];
  if (cur && cur.scrollIntoView) cur.scrollIntoView({ block: "nearest" });
}

/* ---------- icons ---------- */
function paintIcons() {
  const nodes = $list.querySelectorAll(".row");
  for (let i = 0; i < nodes.length; i++) {
    const idx = Number(nodes[i].dataset.i);
    const h = hits[idx];
    if (!h) continue;
    const url = iconCache.get(h.icon_src);
    if (!url) continue;
    const ico = nodes[i].querySelector(".ico");
    if (ico && !ico.classList.contains("has-img")) {
      ico.classList.add("has-img");
      ico.style.backgroundImage = `url("${url}")`;
    }
  }
}

let iconBatch = 0;
async function loadIcons() {
  const mine = ++iconBatch;
  const upTo = Math.min(hits.length, 24);
  const need = [];
  for (let i = 0; i < upTo; i++) {
    const src = hits[i].icon_src;
    if (src && !iconCache.has(src)) need.push(src);
  }
  if (!need.length) {
    paintIcons();
    return;
  }
  try {
    const urls = await invoke("icons", { sources: need });
    if (mine !== iconBatch) return;
    for (let i = 0; i < need.length; i++) {
      if (urls[i]) iconCache.set(need[i], urls[i]);
    }
    paintIcons();
  } catch (_) {
    /* icons are cosmetic — never block on them */
  }
}

/* ---------- built-in commands ----------
   Reachable by typing `>` — a discoverable surface for actions that have no
   business being a global hotkey. */

const COMMANDS = [
  {
    id: "settings",
    name: "设置",
    desc: "索引目录与快捷路径",
    keepOpen: true,
    run: () => openSettings(),
  },
  {
    id: "reindex",
    name: "重建索引",
    desc: "重新扫描所有索引目录",
    keepOpen: true,
    run: () => {
      invoke("reindex");
      meta("正在重建索引…");
    },
  },
  {
    id: "autostart",
    name: "开机自启",
    desc: "切换开机自动启动",
    keepOpen: true,
    run: async () => {
      const on = await invoke("get_autostart");
      const next = !on;
      await invoke("set_autostart_cmd", { enable: next });
      meta(next ? "已设为开机自动启动" : "已取消开机自动启动");
    },
  },
  {
    id: "quit",
    name: "退出 Flint",
    desc: "关闭后台进程",
    run: () => invoke("quit"),
  },
];

let commandMode = false;

/* ---------- action panel ----------
   Tab on any result expands the things you can do with it. This is where a
   launcher stops being a list of shortcuts and starts being a command surface. */

let actionMode = false;
let actionTarget = null;
let savedHits = null;

function actionsFor(h) {
  const out = [];
  const virtual = h.path.startsWith("shell:") || h.path.startsWith("hwnd:");

  if (h.kind === "window") {
    out.push({
      name: "切换到此窗口",
      run: () => invoke("launch", { path: h.path }),
    });
    return out;
  }

  out.push({ name: "打开", run: () => invoke("launch", { path: h.path }) });
  if (!virtual) {
    out.push({
      name: "打开所在位置",
      run: () => invoke("reveal", { path: h.path }),
    });
    out.push({
      name: "复制路径",
      run: () => invoke("copy_text", { text: h.path }),
    });
  }

  const isExe = h.kind === "app" || /\.(exe|bat|cmd|ps1)$/i.test(h.name);
  if (isExe && !virtual) {
    out.push({
      name: "以管理员身份运行",
      run: () => invoke("run_as_admin", { path: h.path }),
    });
  }

  if (h.kind === "folder" || h.kind === "bookmark") {
    out.push({
      name: "在此处打开终端",
      run: () => invoke("open_terminal", { path: h.path }),
    });
  }
  return out;
}

function openActions() {
  if (actionMode) {
    closeActions();
    return;
  }
  const h = hits[sel];
  if (!h || h.kind === "command" || h.kind === "action") return;

  const acts = actionsFor(h);
  if (!acts.length) return;

  actionTarget = h;
  savedHits = hits;
  actionMode = true;

  // The panel is navigated with j/k, so the query box must stop eating keys.
  setMode("normal");

  hits = acts.map((a) => ({
    name: a.name,
    hint: "",
    ext: "ACT",
    kind: "action",
    icon_src: "",
    segs: [{ t: a.name, h: false }],
    _act: a,
  }));
  sel = 0;
  render();
  meta(actionTarget.name);
  $keys.innerHTML = `<span><kbd>j</kbd><kbd>k</kbd>选择</span><span><kbd>↵</kbd>执行</span><span><kbd>esc</kbd>返回</span>`;
}

function closeActions() {
  actionMode = false;
  actionTarget = null;
  if (savedHits) {
    hits = savedHits;
    savedHits = null;
  }
  sel = 0;
  setMode("insert");
  render();
  meta("");
  $keys.innerHTML = KEYHINTS.insert;
}

function renderCommands(raw) {
  const q = raw.toLowerCase();
  const list = COMMANDS.filter(
    (c) => !q || c.name.toLowerCase().includes(q) || c.id.includes(q)
  );

  hits = list.map((c) => ({
    name: c.name,
    hint: c.desc,
    ext: "CMD",
    kind: "command",
    icon_src: "",
    segs: [{ t: c.name, h: false }],
    _cmd: c,
  }));
  sel = 0;
  render();
  meta(list.length ? `${list.length} 个命令` : "没有匹配的命令");
}

/* ---------- search ---------- */
let rafPending = null;

function schedule() {
  if (rafPending) cancelAnimationFrame(rafPending);
  rafPending = requestAnimationFrame(() => {
    rafPending = null;
    run($q.value);
  });
}

async function run(text) {
  const raw = text.trim();

  // `>` prefix switches to command mode — no IPC, purely local.
  if (raw.startsWith(">")) {
    commandMode = true;
    renderCommands(raw.slice(1).trim());
    return;
  }
  commandMode = false;

  const mine = ++seq;
  const t0 = performance.now();
  let res;
  try {
    res = await invoke("search", { q: text, limit: 40 });
  } catch (e) {
    meta("搜索出错：" + e);
    return;
  }
  if (mine !== seq) return; // a newer query already won
  const dt = performance.now() - t0;

  hits = res;
  render();
  loadIcons();

  if (!res.length) {
    meta(
      text.trim()
        ? "没有匹配项"
        : indexed
        ? "索引为空"
        : "正在建立索引…"
    );
  } else {
    const suffix = dt < 1 ? "<1" : dt.toFixed(dt < 10 ? 1 : 0);
    meta(`${res.length} 个结果 · ${suffix} ms`);
  }
}

function meta(t) {
  $meta.textContent = t;
}

/* ---------- actions ---------- */
async function activate(revealIt) {
  const h = hits[sel];
  if (!h) return;

  // Commands are handled locally, not by the shell.
  if (h._cmd) {
    const cmd = h._cmd;
    if (cmd.keepOpen) {
      // Settings, reindex, autostart: the palette stays up to show the result.
      try {
        cmd.run();
      } catch (_) {}
    } else {
      dismiss();
      setTimeout(() => {
        try {
          cmd.run();
        } catch (_) {}
      }, 80);
    }
    return;
  }

  // Actions from the Tab panel.
  if (h._act) {
    const act = h._act;
    dismiss();
    setTimeout(() => {
      try {
        act.run();
      } catch (_) {}
    }, 80);
    return;
  }

  // Hide first: the palette vanishing before the target window appears feels
  // instantaneous, while hiding after it feels sluggish.
  dismiss();
  try {
    await invoke(revealIt ? "reveal" : "launch", { path: h.path });
  } catch (e) {
    meta("打开失败：" + e);
  }
}

/* ---------- settings ----------
   The settings view reuses the same window: the search chrome is hidden by a
   data-view attribute rather than being torn down, so returning is instant. */

const $rootRows = document.getElementById("root-rows");
const $srcRows = document.getElementById("src-rows");
const $addRoot = document.getElementById("add-root");
const $addSrc = document.getElementById("add-src");
const $setSave = document.getElementById("set-save");
const $setMsg = document.getElementById("set-msg");
const $setBack = document.getElementById("set-back");

let settingsOpen = false;
let draft = null;
let suppressBlurReset = false;

function setMsg(t) {
  $setMsg.textContent = t;
}

async function openSettings() {
  setMode("insert");
  try {
    const s = await invoke("get_settings");
    draft = {
      roots: [...(s.roots || [])],
      sources: [...(s.sources || [])],
      max_files: s.max_files || 120000,
    };
  } catch (_) {
    meta("无法读取设置");
    return;
  }
  settingsOpen = true;
  document.documentElement.dataset.view = "settings";
  renderSettings();
  setMsg("");
}

function closeSettings() {
  settingsOpen = false;
  document.documentElement.dataset.view = "search";
  $q.focus();
}

function renderSettings() {
  $rootRows.replaceChildren();
  draft.roots.forEach((p, i) => {
    const row = document.createElement("div");
    row.className = "srow";

    const path = document.createElement("span");
    path.className = "spath";
    path.textContent = p;
    path.title = p;

    const del = document.createElement("button");
    del.className = "del";
    del.textContent = "×";
    del.title = "移除";
    del.addEventListener("click", () => {
      draft.roots.splice(i, 1);
      renderSettings();
    });

    row.append(path, del);
    $rootRows.append(row);
  });

  $srcRows.replaceChildren();
  draft.sources.forEach((s, i) => {
    const row = document.createElement("div");
    row.className = "srow";

    const key = document.createElement("input");
    key.className = "skey";
    key.placeholder = "关键词";
    key.value = s.keyword || "";
    key.spellcheck = false;
    key.addEventListener("input", () => {
      s.keyword = key.value.trim().toLowerCase();
    });

    const path = document.createElement("span");
    path.className = "spath";
    path.textContent = s.path;
    path.title = s.path;

    const del = document.createElement("button");
    del.className = "del";
    del.textContent = "×";
    del.title = "移除";
    del.addEventListener("click", () => {
      draft.sources.splice(i, 1);
      renderSettings();
    });

    row.append(key, path, del);
    $srcRows.append(row);
  });
}

async function pick() {
  // A native folder dialog is its own top-level window, so the palette loses
  // focus while it's up. Suppress the blur-driven reset from the moment the
  // dialog opens until well after focus has been handed back.
  suppressBlurReset = true;
  let path = null;
  try {
    path = await invoke("pick_folder");
  } catch (e) {
    setMsg("打开选择器失败：" + e);
  } finally {
    setTimeout(async () => {
      try {
        await invoke("refocus");
      } catch (_) {}
      // Hold the suppression a little longer: refocus itself causes one more
      // focus transition, and a reset there would wipe the panel mid-edit.
      setTimeout(() => {
        suppressBlurReset = false;
      }, 320);
    }, 60);
  }
  return path;
}

$addRoot.addEventListener("click", async () => {
  const p = await pick();
  if (!p) return;
  if (!draft.roots.includes(p)) draft.roots.push(p);
  renderSettings();
});

$addSrc.addEventListener("click", async () => {
  const p = await pick();
  if (!p) return;
  if (draft.sources.some((s) => s.path === p)) {
    setMsg("这个路径已经在列表里了");
    return;
  }
  draft.sources.push({ path: p, keyword: "", label: "" });
  renderSettings();
  setMsg("给它起个关键词，之后输入就能直达");
  const last = $srcRows.lastElementChild;
  const box = last && last.querySelector(".skey");
  if (box) box.focus();
});

$setSave.addEventListener("click", async () => {
  // A shortcut without a keyword is unreachable — skip those quietly.
  const sources = draft.sources.filter((s) => (s.keyword || "").trim());
  const dropped = draft.sources.length - sources.length;

  setMsg("保存中…");
  let ok = false;
  try {
    ok = await invoke("save_settings", {
      settings: {
        roots: draft.roots,
        max_files: draft.max_files,
        sources,
      },
    });
  } catch (e) {
    setMsg("保存失败：" + e);
    return;
  }

  if (!ok) {
    setMsg("保存失败");
    return;
  }
  setMsg(dropped > 0 ? `已保存（跳过 ${dropped} 条没有关键词）· 重建索引用…` : "已保存，正在重建索引…");
  setTimeout(closeSettings, 700);
});

$setBack.addEventListener("click", closeSettings);

/* ---------- events ---------- */
$q.addEventListener("input", () => {
  if (composing) return;
  schedule();
});

$q.addEventListener("compositionstart", () => {
  composing = true;
});
$q.addEventListener("compositionend", () => {
  composing = false;
  schedule();
});

$list.addEventListener("mousemove", (e) => {
  const row = e.target.closest?.(".row");
  if (!row) return;
  const i = Number(row.dataset.i);
  if (i !== sel) {
    sel = i;
    paintSelection();
  }
});

$list.addEventListener("mousedown", (e) => {
  const row = e.target.closest?.(".row");
  if (!row) return;
  e.preventDefault();
  sel = Number(row.dataset.i);
  activate(e.ctrlKey || e.metaKey);
});

document.addEventListener("keydown", (e) => {
  if (e.isComposing) return;

  // Settings view owns the keyboard while it's up; only Esc gets through.
  if (settingsOpen) {
    if (e.key === "Escape") {
      e.preventDefault();
      closeSettings();
    }
    return;
  }

  // Ctrl+, opens settings. `e.code` is checked too because with a Chinese IME
  // active `e.key` can come through as the full-width comma or "Process".
  if (e.ctrlKey && (e.key === "," || e.key === "，" || e.code === "Comma")) {
    e.preventDefault();
    openSettings();
    return;
  }

  if (actionMode && (e.key === "Escape" || e.key === "Tab")) {
    e.preventDefault();
    closeActions();
    return;
  }

  if (mode === "normal") handleNormal(e);
  else handleInsert(e);
});

function handleInsert(e) {
  // Ctrl+[ is the vim-standard alias for Esc, and unlike Esc it never gets
  // swallowed by the IME (which uses Esc to cancel composition).
  if (e.ctrlKey && (e.key === "[" || e.key === "【")) {
    e.preventDefault();
    if ($q.value.trim()) setMode("normal");
    else dismiss();
    return;
  }

  // Mode-free navigation for people who never want to leave insert mode.
  if (e.ctrlKey && (e.key === "j" || e.key === "J")) {
    e.preventDefault();
    move(1);
    return;
  }
  if (e.ctrlKey && (e.key === "k" || e.key === "K")) {
    e.preventDefault();
    move(-1);
    return;
  }

  switch (e.key) {
    case "ArrowDown":
      e.preventDefault();
      move(1);
      break;
    case "Tab":
      e.preventDefault();
      openActions();
      break;
    case "ArrowUp":
      e.preventDefault();
      move(-1);
      break;
    case "Enter":
      e.preventDefault();
      activate(e.ctrlKey || e.metaKey);
      break;
    case "Escape":
      e.preventDefault();
      // Empty box: Esc means "never mind". Non-empty: switch to navigation.
      if ($q.value.trim()) setMode("normal");
      else dismiss();
      break;
  }

  if (e.key === "n" && e.ctrlKey) {
    e.preventDefault();
    move(1);
  }
  if (e.key === "p" && e.ctrlKey) {
    e.preventDefault();
    move(-1);
  }
}

function handleNormal(e) {
  if (e.ctrlKey && e.key === "Enter") {
    e.preventDefault();
    activate(true);
    return;
  }

  switch (e.key) {
    case "j":
    case "ArrowDown":
      e.preventDefault();
      move(1);
      break;
    case "Tab":
      e.preventDefault();
      openActions();
      break;
    case "k":
    case "ArrowUp":
      e.preventDefault();
      move(-1);
      break;
    case "g": {
      e.preventDefault();
      const now = Date.now();
      if (now - lastG < 500) {
        sel = 0;
        paintSelection();
        lastG = 0;
      } else {
        lastG = now;
      }
      break;
    }
    case "G":
      e.preventDefault();
      sel = Math.max(0, hits.length - 1);
      paintSelection();
      break;
    case "Enter":
      e.preventDefault();
      activate(false);
      break;
    case "i":
    case "a":
    case "/":
      e.preventDefault();
      setMode("insert");
      break;
    case "q":
    case "Escape":
      e.preventDefault();
      dismiss();
      break;
  }
}

/* ---------- theme ---------- */
const mq = window.matchMedia("(prefers-color-scheme: dark)");
function applyTheme() {
  document.documentElement.dataset.theme = mq.matches ? "dark" : "light";
}
applyTheme();
mq.addEventListener("change", applyTheme);

/* ---------- lifecycle ---------- */
function replayIntro() {
  $frame.style.animation = "none";
  void $frame.offsetWidth;
  $frame.style.animation = "";
}

/** Back to a clean slate. Runs while the window is hidden, so the user never
 *  sees the palette rearrange itself. */
function reset() {
  $q.value = "";
  hits = [];
  sel = 0;
  seq++;
  lastG = 0;
  mode = "insert";
  settingsOpen = false;
  commandMode = false;
  actionMode = false;
  actionTarget = null;
  savedHits = null;
  document.documentElement.dataset.mode = "insert";
  document.documentElement.dataset.view = "search";
  $q.readOnly = false;
  $mode.textContent = "INSERT";
  $keys.innerHTML = KEYHINTS.insert;
  $list.replaceChildren();
  replayIntro();
  // Keep the caret parked in the box: the very first keystroke after summoning
  // must land in the query, not nowhere.
  $q.focus();
  run("");
  if (!indexed) pollIndex();
}

window.addEventListener("blur", () => {
  // Window is about to be hidden — clear the deck for the next summon.
  // The delay lets a transient focus flicker settle before we wipe the query.
  setTimeout(() => {
    if (suppressBlurReset) return;
    if (!document.hasFocus()) reset();
  }, 90);
});

window.addEventListener("focus", () => {
  $q.focus();
  if ($q.value.length) $q.select();
});

event.listen("flint:indexed", (e) => {
  onIndexed(Number(e.payload) || 0);
});

event.listen("flint:open-settings", () => {
  // Let the summon settle first, otherwise the blur-driven reset that runs on
  // hide can land after us and close the panel again.
  setTimeout(() => {
    if (!settingsOpen) openSettings();
  }, 160);
});

/* Belt and braces: events can land before the listener is attached, so the UI
   also polls until the index reports something. Stops as soon as it does. */
let pollTimer = null;
let pollTries = 0;

function pollIndex() {
  if (indexed || pollTimer) return;
  pollTries = 0;
  const tick = async () => {
    pollTimer = null;
    if (indexed) return;
    try {
      const s = await invoke("stats");
      if (s && s.total > 0) {
        onIndexed(s.total);
        return;
      }
    } catch (_) {}
    if (++pollTries < 60) pollTimer = setTimeout(tick, 400);
  };
  pollTimer = setTimeout(tick, 400);
}

function onIndexed(n) {
  indexed = true;
  if (pollTimer) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
  if (!$q.value.trim()) run("");
  meta(`索引就绪 · ${n} 项 · 输入关键词开始`);
}

/* first paint */
document.addEventListener("contextmenu", (e) => e.preventDefault());
$q.focus();
reset();
