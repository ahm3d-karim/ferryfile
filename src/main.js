import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

const drawer = document.getElementById("drawer");
const idle = document.getElementById("idle");
const filesEl = document.getElementById("files");
const actionsEl = document.getElementById("actions");
const statusEl = document.getElementById("status");

let paths = [];

const basename = (p) => p.split(/[\\/]/).pop();

function setStatus(text, kind = "") {
  statusEl.textContent = text;
  statusEl.className = kind;
}

function reset() {
  paths = [];
  idle.hidden = false;
  filesEl.replaceChildren();
  actionsEl.replaceChildren();
  setStatus("");
}

async function runAction(action) {
  setStatus(`${action.group}: ${action.label}…`, "busy");
  const results = [];
  for (const p of paths) {
    try {
      results.push(basename(await invoke("run_action", { actionId: action.id, path: p })));
    } catch (err) {
      setStatus(`${basename(p)}: ${err}`, "error");
      return;
    }
  }
  setStatus(`Saved ${results.join(", ")}`, "ok");
}

async function showFiles(dropped) {
  paths = dropped;
  idle.hidden = paths.length > 0;
  filesEl.replaceChildren(
    ...paths.map((p) => {
      const li = document.createElement("li");
      li.textContent = basename(p);
      li.title = p;
      return li;
    }),
  );
  actionsEl.replaceChildren();
  setStatus("");
  if (!paths.length) return;

  // Actions come from the first file; other files get them if they support the same id.
  const actions = await invoke("list_actions", { path: paths[0] });
  if (!actions.length) {
    setStatus("No actions for this file type yet.");
    return;
  }
  const groups = new Map();
  for (const a of actions) {
    if (!groups.has(a.group)) groups.set(a.group, []);
    groups.get(a.group).push(a);
  }
  for (const [name, items] of groups) {
    const section = document.createElement("section");
    const h = document.createElement("h2");
    h.textContent = name;
    const row = document.createElement("div");
    row.className = "chips";
    for (const a of items) {
      const b = document.createElement("button");
      b.textContent = a.label;
      b.addEventListener("click", () => runAction(a));
      row.append(b);
    }
    section.append(h, row);
    actionsEl.append(section);
  }
}

document.getElementById("close").addEventListener("click", () => invoke("hide_drawer"));
window.addEventListener("keydown", (e) => e.key === "Escape" && invoke("hide_drawer"));

listen("skiff-drag-start", () => {
  reset();
  drawer.classList.add("open");
});
listen("skiff-drag-end", () => drawer.classList.remove("open", "hover"));

// Drops use WebView2's native drag-and-drop (see src-tauri/src/dropbridge.rs):
// the page accepts the drop and hands the File objects to Rust, which replies
// with their real paths.
document.addEventListener("dragover", (e) => {
  e.preventDefault();
  e.dataTransfer.dropEffect = "copy";
  drawer.classList.add("hover");
});
document.addEventListener("dragleave", (e) => {
  if (!e.relatedTarget) drawer.classList.remove("hover");
});
document.addEventListener("drop", (e) => {
  e.preventDefault();
  drawer.classList.remove("hover");
  const files = e.dataTransfer.files;
  if (!files.length) return;
  invoke("pin_drawer");
  window.chrome.webview.postMessageWithAdditionalObjects("skiff-drop", files);
});
listen("skiff-dropped", (e) => {
  showFiles(e.payload);
});
