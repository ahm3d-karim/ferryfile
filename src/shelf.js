import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startDrag } from "@crabnebula/tauri-plugin-drag";

const $ = (id) => document.getElementById(id);
const shelf = $("shelf");
const list = $("items");
const panel = $("panel");
const groupsEl = $("groups");
const statusEl = $("status");
const progressEl = $("progress");

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
  statusEl.textContent = text;
  statusEl.className = kind;
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
  $("empty").hidden = n > 0;
  $("drag-all").hidden = $("actions").hidden = n === 0;
  if (n === 0) panel.hidden = true;
  $("count").textContent = n ? String(n) : "";
  $("pin").classList.toggle("on", pinned);
  $("pin").title = pinned
    ? "Pinned: stays open when emptied. Click to unpin."
    : "Pin this shelf so it stays open when emptied";
  list.replaceChildren(
    ...items.map((p) => {
      const li = el("li", { title: `${p}\nDrag out to use · double-click to show in Explorer` }, [
        el("div", { className: "text" }, [
          el("span", { className: "name", textContent: basename(p) }),
          el("span", { className: "where", textContent: folder(p) }),
        ]),
        el("button", {
          className: "icon",
          textContent: "×",
          title: "Remove from shelf",
          onclick: () => invoke("shelf_remove", { id, paths: [p], draggedOut: false }),
        }),
      ]);
      li.addEventListener("dblclick", () => invoke("reveal", { path: p }));
      draggable(li, () => [p]);
      return li;
    }),
  );
  if (!panel.hidden) renderActions();
}

// ---------------------------------------------------------------- actions

let running = null; // { index, count }

listen("ferryfile-progress", (e) => {
  if (!running) return;
  progressEl.hidden = false;
  progressEl.value = (running.index + e.payload.fraction) / running.count;
});

/**
 * Runs `job(path)` for each target and puts every output on this shelf, so
 * results are ready to drag out. Items an action doesn't apply to are skipped.
 */
async function runOnShelf(label, job, targets = items) {
  if (running) return;
  setStatus(`${label}…`, "busy");
  panel.classList.add("disabled");
  progressEl.value = 0;
  progressEl.hidden = false;
  const outputs = [];
  let skipped = 0;
  try {
    for (const [index, p] of targets.entries()) {
      running = { index, count: targets.length };
      try {
        outputs.push(await job(p));
      } catch (err) {
        if (/No engine can run|can't run on/.test(String(err))) skipped++;
        else {
          setStatus(`${basename(p)}: ${err}`, "error");
          return;
        }
      }
    }
    if (outputs.length) await invoke("shelf_add", { id, paths: outputs });
    const note = skipped ? ` (${skipped} skipped: not supported)` : "";
    setStatus(outputs.length ? `Added ${outputs.length} result${outputs.length > 1 ? "s" : ""} to the shelf${note}` : `Nothing to do${note}`, outputs.length ? "ok" : "");
  } finally {
    running = null;
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

const chip = (label, onClick, title = "") => el("button", { type: "button", textContent: label, title, onclick: onClick });
const section = (name, chips) => (chips.length ? el("section", {}, [el("h2", { textContent: name }), el("div", { className: "chips" }, chips)]) : "");

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
        chip(a.label, () => runOnShelf(`${a.group}: ${a.label}`, () => invoke("run_combine", { actionId: a.id, paths: snapshot }), [first]), `All ${n} items, in shelf order`),
      ),
    ),
    section(
      "Recipes",
      recipes.map((r) => chip(r.name, () => runOnShelf(r.name, (p) => invoke("run_recipe", { id: r.id, path: p })), r.stepTitles.join(" → "))),
    ),
    ...[...groups].map(([name, acts]) =>
      section(
        name,
        acts.map((a) => chip(a.label, () => runOnShelf(`${a.group}: ${a.label}`, (p) => invoke("run_action", { actionId: a.id, path: p })))),
      ),
    ),
    section("Files", [
      chip("Zip", () => runOnShelf("Zipping", () => invoke("zip_items", { paths: snapshot }), [first]), `One .zip of ${all}`),
      chip("Copy", () => quick(`Copied ${all}: paste in Explorer`, () => invoke("copy_files", { paths: snapshot }))),
      chip("Copy path", () => quick("Path copied", () => invoke("copy_paths", { paths: snapshot }))),
      chip("Move to…", () =>
        quick(`Moved ${all}`, async () => {
          const moved = await invoke("send_to_folder", { paths: snapshot, copy: false });
          if (!moved.length) return false;
          await invoke("shelf_replace", { id, old: snapshot, new: moved });
        }),
      ),
      chip("Copy to…", () =>
        quick(`Copied ${all}`, async () => {
          const copied = await invoke("send_to_folder", { paths: snapshot, copy: true });
          if (!copied.length) return false;
        }),
      ),
      chip("Show in Explorer", () => quick("", () => invoke("reveal", { path: first }))),
      chip("Clear shelf", () => invoke("shelf_remove", { id, paths: snapshot, draggedOut: false }), "Remove everything (files stay where they are)"),
    ]),
  );
}

$("actions").addEventListener("click", () => {
  panel.hidden = !panel.hidden;
  $("actions").textContent = panel.hidden ? "Actions ▾" : "Actions ▴";
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
document.addEventListener("drop", (e) => {
  e.preventDefault();
  shelf.classList.remove("hover");
  if (e.dataTransfer.files.length) {
    window.chrome.webview.postMessageWithAdditionalObjects("ferryfile-drop", e.dataTransfer.files);
  }
});
listen("ferryfile-dropped", (e) => invoke("shelf_add", { id, paths: e.payload }));

apply(await invoke("shelf_state", { id }));
