import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { icon, hydrateIcons, iconForPath } from "./icons.js";
import { createQueue, batchLimit } from "./queue.js";

const $ = (id) => document.getElementById(id);
const shelf = $("shelf");
const list = $("items");
const panel = $("panel");
const groupsEl = $("groups");
const statusEl = $("status");
const progressEl = $("progress");

hydrateIcons();

// Window label is "shelf-<id>".
const id = getCurrentWindow().label.replace(/^shelf-/, "");
let items = [];
let pinned = false;
const iconReady = invoke("drag_icon");

const basename = (p) => p.split(/[\\/]/).pop();
const folder = (p) => p.split(/[\\/]/).slice(-2, -1)[0] ?? "";

function el(tag, props = {}, children = []) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

function setStatus(text, kind = "") {
  const bar = $("infobar");
  bar.hidden = !text;
  bar.className = `infobar ${kind}`;
  statusEl.textContent = text;
  $("info-state").replaceChildren(icon(kind === "ok" ? "check" : kind === "error" ? "error" : "info"));
}

// Size per path, for the "folder · size" line (looked up once).
const sizes = new Map();
function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 10 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}
async function sizeOf(p) {
  if (!sizes.has(p)) {
    try {
      const info = await invoke("file_info", { path: p });
      sizes.set(p, info.isDir ? `${info.items ?? 0} items` : formatBytes(info.size));
    } catch {
      sizes.set(p, "");
    }
  }
  return sizes.get(p);
}

// ---------------------------------------------------------------- drag out

/** Native drag of `paths` out of the shelf; dropped items leave the shelf. */
async function dragOut(paths) {
  const icon = await iconReady;
  await startDrag({ item: paths, icon }, ({ result }) => {
    if (result === "Dropped") invoke("shelf_remove", { id, paths, draggedOut: true });
  });
}

/** Starts a drag once the pointer has moved a few pixels with the button down. */
function draggable(node, paths) {
  node.addEventListener("pointerdown", (down) => {
    // Presses on other buttons (like an item's ×) aren't drags; the node itself may be a button.
    const button = down.target.closest("button");
    if (down.button !== 0 || (button && button !== node)) return;
    const move = (e) => {
      if (Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < 4) return;
      stop();
      dragOut(paths());
    };
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
  });
}

// ---------------------------------------------------------------- render

function render() {
  const n = items.length;
  // While a batch list is up it takes the items' place (same files, with status).
  const queueShown = !$("queue").hidden;
  list.hidden = queueShown;
  $("empty").hidden = n > 0 || queueShown;
  $("drag-all").hidden = $("actions").hidden = n === 0;
  if (n === 0) panel.hidden = true;
  $("count").textContent = n ? `${n} item${n > 1 ? "s" : ""}` : "";
  $("pin").classList.toggle("on", pinned);
  $("pin").title = pinned
    ? "Pinned: stays open when emptied. Click to unpin."
    : "Pin this shelf so it stays open when emptied";
  $("pin").ariaLabel = pinned ? "Unpin shelf" : "Pin shelf";
  list.replaceChildren(
    ...items.map((p) => {
      const where = el("span", { className: "where", textContent: folder(p) });
      sizeOf(p).then((size) => size && (where.textContent = `${folder(p)} · ${size}`));
      const li = el("li", { title: `${p}\nDrag out to use · double-click to show in Explorer` }, [
        icon(iconForPath(p)),
        el("div", { className: "text" }, [el("span", { className: "name", textContent: basename(p) }), where]),
        el(
          "button",
          {
            className: "icon-btn",
            title: "Remove from shelf",
            ariaLabel: `Remove ${basename(p)} from shelf`,
            onclick: () => invoke("shelf_remove", { id, paths: [p], draggedOut: false }),
          },
          [icon("close")],
        ),
      ]);
      li.addEventListener("dblclick", () => invoke("reveal", { path: p }));
      draggable(li, () => [p]);
      return li;
    }),
  );
  if (!panel.hidden) renderActions();
}

// ---------------------------------------------------------------- actions

let running = null; // { index, count, jobId, cancelled }

// Several files: a queue list with a row per file (see queue.js).
const batch = createQueue($("queue"), {
  onChange(s) {
    panel.classList.toggle("disabled", s.busy);
    if (s.busy) setStatus("");
    if (!s.total) render(); // list cleared: show the items again
  },
  async onDone(outputs, s) {
    if (outputs.length) await invoke("shelf_add", { id, paths: outputs });
    const skipped = s.skipped ? ` (${s.skipped} skipped: not supported)` : "";
    const added = outputs.length ? `Added ${outputs.length} result${outputs.length > 1 ? "s" : ""} to the shelf` : "";
    if (s.failed) setStatus(`${s.failed} of ${s.total} failed${added ? `. ${added}` : ""}`, "error");
    else if (s.cancelled) setStatus(added ? `Cancelled. ${added}` : "Cancelled", "");
    else setStatus(added ? `${added}${skipped}` : `Nothing to do${skipped}`, added ? "ok" : "");
  },
});

listen("ferryfile-progress", (e) => {
  if (!running) return;
  progressEl.hidden = false;
  progressEl.value = (running.index + e.payload.fraction) / running.count;
});

$("info-cancel").addEventListener("click", () => {
  if (!running) return;
  running.cancelled = true;
  if (running.jobId) invoke("cancel_job", { jobId: running.jobId });
  setStatus("Cancelling…", "busy");
});

/**
 * Runs `job(path, jobId)` for each target and puts every output on this
 * shelf, so results are ready to drag out. Items an action doesn't apply to
 * are skipped; Cancel stops the current item and the rest.
 */
async function runOnShelf(label, job, targets = items, steps = []) {
  if (running || batch.busy()) return;
  if (targets.length > 1) {
    panel.hidden = true;
    setActionsLabel();
    setStatus("");
    $("queue").hidden = false;
    render();
    batch.run({ targets: [...targets], run: job, max: await batchLimit(targets[0], steps) });
    return;
  }
  setStatus(`${label}…`, "busy");
  $("info-cancel").hidden = false;
  panel.classList.add("disabled");
  progressEl.value = 0;
  progressEl.hidden = false;
  const outputs = [];
  let skipped = 0;
  try {
    for (const [index, p] of targets.entries()) {
      if (running?.cancelled) break;
      running = { index, count: targets.length, jobId: crypto.randomUUID(), cancelled: running?.cancelled ?? false };
      try {
        outputs.push(await job(p, running.jobId));
      } catch (err) {
        if (String(err) === "Cancelled") break;
        if (/No engine can run|can't run on/.test(String(err))) skipped++;
        else {
          setStatus(`${basename(p)}: ${err}`, "error");
          return;
        }
      }
    }
    if (outputs.length) await invoke("shelf_add", { id, paths: outputs });
    const note = skipped ? ` (${skipped} skipped: not supported)` : "";
    if (running?.cancelled) setStatus(outputs.length ? `Cancelled; ${outputs.length} finished and added` : "Cancelled", "");
    else setStatus(outputs.length ? `Added ${outputs.length} result${outputs.length > 1 ? "s" : ""} to the shelf${note}` : `Nothing to do${note}`, outputs.length ? "ok" : "");
  } finally {
    running = null;
    $("info-cancel").hidden = true;
    progressEl.hidden = true;
    panel.classList.remove("disabled");
  }
}

/** Quick file actions that don't produce results on the shelf. */
async function quick(done, task) {
  try {
    const r = await task();
    if (r !== false) setStatus(done, "ok");
  } catch (err) {
    setStatus(String(err), "error");
  }
}

const chip = (label, onClick, title = "", iconName = null) =>
  el("button", { type: "button", className: "row", title, onclick: onClick }, [
    ...(iconName ? [icon(iconName)] : []),
    el("span", { className: "label", textContent: label }),
  ]);
const section = (name, rows) => (rows.length ? el("section", {}, [el("div", { className: "heading", textContent: name }), ...rows]) : "");

async function renderActions() {
  const snapshot = [...items];
  const first = snapshot[0];
  if (!first) return;
  const [actions, combined, recipes] = await Promise.all([
    invoke("list_actions", { path: first }),
    snapshot.length > 1 ? invoke("combine_actions", { paths: snapshot }) : [],
    invoke("recipes_for", { path: first }),
  ]);
  if (snapshot.join("|") !== items.join("|")) return; // items changed meanwhile

  const n = snapshot.length;
  const all = n === 1 ? basename(first) : `${n} items`;
  const groups = Map.groupBy(actions, (a) => a.group);

  groupsEl.replaceChildren(
    section(
      "Combine",
      combined.map((a) =>
        chip(a.label, () => runOnShelf(`${a.group}: ${a.label}`, (_, jobId) => invoke("run_combine", { actionId: a.id, paths: snapshot, jobId }), [first]), `All ${n} items, in shelf order`),
      ),
    ),
    section(
      "Recipes",
      recipes.map((r) => chip(r.name, () => runOnShelf(r.name, (p, jobId) => invoke("run_recipe", { id: r.id, path: p, jobId }), items, r.steps), r.stepTitles.join(" → "))),
    ),
    ...[...groups].map(([name, acts]) =>
      section(
        name,
        acts.map((a) => chip(a.label, () => runOnShelf(`${a.group}: ${a.label}`, (p, jobId) => invoke("run_action", { actionId: a.id, path: p, jobId }), items, [a.id]))),
      ),
    ),
    section("Files", [
      chip("Zip", () => runOnShelf("Zipping", () => invoke("zip_items", { paths: snapshot }), [first]), `One .zip of ${all}`, "zip"),
      chip("Copy", () => quick(`Copied ${all}. Paste in Explorer.`, () => invoke("copy_files", { paths: snapshot })), "", "copy"),
      chip("Copy path", () => quick("Path copied", () => invoke("copy_paths", { paths: snapshot })), "", "link"),
      chip("Move to…", () =>
        quick(`Moved ${all}`, async () => {
          const moved = await invoke("send_to_folder", { paths: snapshot, copy: false });
          if (!moved.length) return false;
          await invoke("shelf_replace", { id, old: snapshot, new: moved });
        }),
        "",
        "move",
      ),
      chip("Copy to…", () =>
        quick(`Copied ${all}`, async () => {
          const copied = await invoke("send_to_folder", { paths: snapshot, copy: true });
          if (!copied.length) return false;
        }),
        "",
        "copyTo",
      ),
      chip("Show in Explorer", () => quick("", () => invoke("reveal", { path: first })), "", "explorer"),
      chip("Clear shelf", () => invoke("shelf_remove", { id, paths: snapshot, draggedOut: false }), "Remove everything (files stay where they are)", "close"),
    ]),
  );
}

function setActionsLabel() {
  $("actions").replaceChildren("Actions", icon(panel.hidden ? "chevronDown" : "chevronRight"));
}
setActionsLabel();
$("actions").addEventListener("click", () => {
  panel.hidden = !panel.hidden;
  setActionsLabel();
  if (!panel.hidden) renderActions();
});

// ---------------------------------------------------------------- header

$("pin").addEventListener("click", () => invoke("shelf_set_pinned", { id, pinned: !pinned }));
$("close").addEventListener("click", () => invoke("shelf_close", { id }));
draggable($("drag-all"), () => items);

// ---------------------------------------------------------------- state & drops

function apply(state) {
  if (!state) return;
  items = state.items;
  pinned = state.pinned;
  render();
}

listen("ferryfile-shelf-changed", (e) => apply(e.payload));

// Same native drop path as the drawer (see src-tauri/src/dropbridge.rs).
document.addEventListener("dragover", (e) => {
  e.preventDefault();
  e.dataTransfer.dropEffect = "copy";
  shelf.classList.add("hover");
});
document.addEventListener("dragleave", (e) => {
  if (!e.relatedTarget) shelf.classList.remove("hover");
});
// Files go to Rust for their paths. Text, links and images from a browser or
// another app are saved as files (.txt, .url, .png) and land here like any file.
let pendingDrop = null; // { files, link } until Rust answers with the paths

document.addEventListener("drop", (e) => {
  e.preventDefault();
  shelf.classList.remove("hover");
  const dt = e.dataTransfer;
  const link = linkOf(dt);
  if (dt.files.length) {
    pendingDrop = { files: [...dt.files], link };
    window.chrome.webview.postMessageWithAdditionalObjects("ferryfile-drop", dt.files);
  } else if (link) {
    added(invoke("shelf_add_url", { id, url: link.url, title: link.title }));
  } else {
    const text = dt.getData("text/plain");
    if (text.trim()) added(invoke("shelf_add_text", { id, text }));
  }
});

listen("ferryfile-dropped", async (e) => {
  const paths = e.payload;
  if (paths.length) invoke("shelf_add", { id, paths });
  const drop = pendingDrop;
  pendingDrop = null;
  if (!drop) return;
  // Files without a path (an image dragged out of a browser, an Outlook
  // attachment): read their bytes here and let Rust save them.
  const names = new Set(paths.map(basename));
  for (const file of drop.files.filter((f) => !names.has(f.name))) {
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      await added(invoke("shelf_add_bytes", bytes, { headers: { "x-shelf-id": id, "x-name": encodeURIComponent(file.name) } }));
    } catch {
      // Couldn't read it: keep at least a shortcut to where it came from.
      if (drop.link) added(invoke("shelf_add_url", { id, url: drop.link.url, title: drop.link.title }));
    }
  }
});

/** The dragged link and its title, if a browser sent one. */
function linkOf(dt) {
  const moz = dt.getData("text/x-moz-url"); // Firefox: url, newline, title
  if (moz) {
    const [url, title] = moz.split(/\r?\n/);
    if (/^https?:/i.test(url)) return { url, title: title || null };
  }
  const url = dt.getData("text/uri-list").split(/\r?\n/).find((l) => l && !l.startsWith("#"));
  if (!url || !/^https?:/i.test(url)) return null;
  // Chrome and Edge send the link as <a href>title</a>; an image as <img alt>.
  const doc = new DOMParser().parseFromString(dt.getData("text/html"), "text/html");
  const a = doc.querySelector("a");
  const title = (a?.textContent || doc.querySelector("img")?.alt || doc.title || "").trim();
  const plain = dt.getData("text/plain").trim();
  return { url, title: title || (plain && plain !== url ? plain : null) };
}

/** Shows what a save added (or why it failed). */
async function added(task) {
  try {
    const paths = await task;
    if (paths?.length) setStatus(`Added ${paths.map(basename).join(", ")}`, "ok");
    return paths;
  } catch (err) {
    setStatus(String(err), "error");
  }
}

// ---------------------------------------------------------------- paste

async function paste() {
  const paths = await added(invoke("shelf_paste", { id }));
  if (paths?.length) setStatus(`Pasted ${paths.map(basename).join(", ")}`, "ok");
}
$("paste").addEventListener("click", paste);

// Shelves never take focus by themselves (drags in mustn't steal it from the
// app you drag from). Clicking one lets it, so Ctrl+V works; it lets go again
// when you click elsewhere.
document.addEventListener("pointerdown", () => {
  if (!document.hasFocus()) invoke("shelf_focus", { id, focus: true });
});
window.addEventListener("blur", () => invoke("shelf_focus", { id, focus: false }));
window.addEventListener("keydown", (e) => {
  if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "v") {
    e.preventDefault();
    paste();
  }
});

apply(await invoke("shelf_state", { id }));
