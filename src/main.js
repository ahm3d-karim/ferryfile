import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

const $ = (id) => document.getElementById(id);
const drawer = $("drawer");
const idle = $("idle");
const filesEl = $("files");
const toolsEl = $("tools");
const searchEl = $("search");
const recipeChips = $("recipe-chips");
const actionsEl = $("actions");
const statusEl = $("status");
const progressEl = $("progress");

const editor = $("editor");
const editorTitle = $("editor-title");
const nameEl = $("recipe-name");
const stepsEl = $("steps");
const addStepEl = $("add-step");
const editorError = $("editor-error");
const saveBtn = $("save-recipe");
const deleteBtn = $("delete-recipe");

let paths = [];

const basename = (p) => p.split(/[\\/]/).pop();

function el(tag, props = {}, children = []) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

function setStatus(text, kind = "") {
  statusEl.textContent = text;
  statusEl.className = kind;
}

function reset() {
  paths = [];
  idle.hidden = false;
  filesEl.replaceChildren();
  toolsEl.hidden = true;
  closeEditor();
  setStatus("");
}

// ---------------------------------------------------------------- running

// Overall progress across the files of the current run.
let running = null; // { index, count }

function setProgress(fraction) {
  progressEl.hidden = fraction == null;
  if (fraction != null) progressEl.value = fraction;
}

listen("ferryfile-progress", (e) => {
  if (running) setProgress((running.index + e.payload.fraction) / running.count);
});

/** Runs `job(path)` for every target (by default each dropped file), with progress and status. */
async function runJob(label, job, targets = paths) {
  if (running) return;
  setStatus(`${label}…`, "busy");
  toolsEl.classList.add("disabled");
  setProgress(0);
  const results = [];
  try {
    for (const [index, p] of targets.entries()) {
      running = { index, count: targets.length };
      try {
        results.push(basename(await job(p)));
      } catch (err) {
        setStatus(`${basename(p)}: ${err}`, "error");
        return;
      }
    }
    setStatus(`Saved ${results.join(", ")}`, "ok");
  } finally {
    running = null;
    setProgress(null);
    toolsEl.classList.remove("disabled");
  }
}

// ---------------------------------------------------------------- listing

function chip(label, onClick, { title = "", search = label, extra } = {}) {
  const b = el("button", { type: "button", textContent: label, title });
  b.dataset.search = search.toLowerCase();
  b.addEventListener("click", onClick);
  if (!extra) return b;
  const wrap = el("span", { className: "chip-wrap" }, [b, extra]);
  wrap.dataset.search = b.dataset.search;
  return wrap;
}

async function renderRecipes() {
  const recipes = await invoke("recipes_for", { path: paths[0] });
  recipeChips.replaceChildren(
    ...recipes.map((r) => {
      const steps = r.stepTitles.join(" → ");
      const run = () => runJob(r.name, (p) => invoke("run_recipe", { id: r.id, path: p }));
      const edit = r.builtin
        ? null
        : el("button", {
            type: "button",
            className: "edit",
            textContent: "✎",
            title: "Edit recipe",
            onclick: () => openEditor(r),
          });
      return chip(r.name, run, { title: steps, search: `${r.name} ${steps}`, extra: edit });
    }),
    chip("+ New", () => openEditor(null), { title: "Chain actions into a recipe", search: "new recipe" }),
  );
}

/** Runs a quick file action and reports the outcome in the status line. */
async function fileAction(done, task) {
  try {
    const result = await task();
    if (result !== false) setStatus(done, "ok");
  } catch (err) {
    setStatus(String(err), "error");
  }
}

/** Actions for any dropped items, folders included; no conversion involved. */
function fileChips() {
  const n = paths.length === 1 ? basename(paths[0]) : `${paths.length} items`;
  const sendTo = (copy) => async () => {
    const moved = await invoke("send_to_folder", { paths, copy });
    if (!moved.length) return false; // cancelled
    if (!copy) await showFiles(moved); // keep acting on the files where they now are
    return true;
  };
  return [
    chip("Copy path", () => fileAction("Path copied", () => invoke("copy_paths", { paths })), {
      title: "Copy the full path(s) as text",
      search: "file copy path",
    }),
    chip("Copy", () => fileAction(`Copied ${n} — paste in Explorer`, () => invoke("copy_files", { paths })), {
      title: "Put the files on the clipboard",
      search: "file copy clipboard",
    }),
    chip("Show in Explorer", () => fileAction("", () => invoke("reveal", { path: paths[0] })), {
      search: "file show explorer reveal folder",
    }),
    chip("Terminal here", () => fileAction("", () => invoke("open_terminal", { path: paths[0] })), {
      search: "file terminal console powershell",
    }),
    chip("Move to…", () => fileAction(`Moved ${n}`, sendTo(false)), { search: "file move folder" }),
    chip("Copy to…", () => fileAction(`Copied ${n}`, sendTo(true)), { search: "file copy to folder duplicate" }),
    chip("Add to shelf", () => fileAction(`${n} added to the shelf`, () => invoke("shelf_add", { paths })), {
      title: "Keep these handy in the floating shelf",
      search: "file shelf float keep",
    }),
  ];
}

async function renderActions() {
  // Actions come from the first file; other files get them if they support the same id.
  const [actions, combined] = await Promise.all([
    invoke("list_actions", { path: paths[0] }),
    paths.length > 1 ? invoke("combine_actions", { paths }) : [],
  ]);

  // Combine actions take all dropped files at once and produce one output.
  const combineChip = (a) =>
    chip(
      a.label,
      () => runJob(`${a.group}: ${a.label}`, () => invoke("run_combine", { actionId: a.id, paths }), [paths[0]]),
      { title: `All ${paths.length} files, in drop order`, search: `${a.group} ${a.label}` },
    );
  const actionChip = (a) =>
    chip(a.label, () => runJob(`${a.group}: ${a.label}`, (p) => invoke("run_action", { actionId: a.id, path: p })), {
      search: `${a.group} ${a.label}`,
    });

  const groups = [
    ...[...Map.groupBy(combined, (a) => a.group)].map(([name, items]) => [name, items.map(combineChip)]),
    ...[...Map.groupBy(actions, (a) => a.group)].map(([name, items]) => [name, items.map(actionChip)]),
    ["File", fileChips()],
  ];
  actionsEl.replaceChildren(
    ...groups.map(([name, chips]) =>
      el("section", {}, [el("h2", { textContent: name }), el("div", { className: "chips" }, chips)]),
    ),
  );

}

function applySearch() {
  const q = searchEl.value.trim().toLowerCase();
  for (const section of toolsEl.querySelectorAll("section")) {
    let visible = 0;
    for (const c of section.querySelectorAll(".chips > *")) {
      const show = !q || c.dataset.search.includes(q);
      c.hidden = !show;
      if (show) visible++;
    }
    section.hidden = visible === 0;
  }
}

async function showFiles(dropped) {
  paths = dropped;
  idle.hidden = paths.length > 0;
  filesEl.replaceChildren(...paths.map((p) => el("li", { textContent: basename(p), title: p })));
  closeEditor();
  setStatus("");
  if (!paths.length) return;

  searchEl.value = "";
  await Promise.all([renderRecipes(), renderActions()]);
  toolsEl.hidden = false;
  applySearch();
}

searchEl.addEventListener("input", applySearch);

// ---------------------------------------------------------------- editor

let editing = null; // { id: string | null, steps: [{ id, title }] }

function openEditor(recipe) {
  editing = {
    id: recipe?.id ?? null,
    steps: recipe ? recipe.steps.map((id, i) => ({ id, title: recipe.stepTitles[i] })) : [],
  };
  editorTitle.textContent = recipe ? "Edit recipe" : "New recipe";
  nameEl.value = recipe?.name ?? "";
  deleteBtn.hidden = !recipe;
  toolsEl.hidden = true;
  editor.hidden = false;
  setStatus("");
  refreshEditor();
  invoke("focus_drawer").then(() => nameEl.focus());
}

function closeEditor() {
  editing = null;
  editor.hidden = true;
  if (paths.length) toolsEl.hidden = false;
}

async function refreshEditor() {
  stepsEl.replaceChildren(
    ...editing.steps.map((s, i) =>
      el("li", {}, [
        el("span", { textContent: s.title }),
        el("button", {
          type: "button",
          className: "remove",
          textContent: "×",
          title: "Remove step",
          onclick: () => {
            editing.steps.splice(i, 1);
            refreshEditor();
          },
        }),
      ]),
    ),
  );

  let next = [];
  editorError.textContent = "";
  try {
    next = await invoke("next_actions", { path: paths[0], steps: editing.steps.map((s) => s.id) });
  } catch (err) {
    editorError.textContent = err;
  }
  addStepEl.replaceChildren(
    el("option", { value: "", textContent: next.length ? "Add a step…" : "No further steps for this format" }),
    ...next.map((a) => el("option", { value: a.id, textContent: `${a.group} ${a.label}` })),
  );
  addStepEl.disabled = !next.length;
  saveBtn.disabled = !!editorError.textContent || !editing.steps.length;
}

addStepEl.addEventListener("change", () => {
  const opt = addStepEl.selectedOptions[0];
  if (!opt?.value) return;
  editing.steps.push({ id: opt.value, title: opt.textContent });
  refreshEditor();
});

editor.addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    await invoke("save_recipe", {
      id: editing.id,
      name: nameEl.value,
      steps: editing.steps.map((s) => s.id),
    });
  } catch (err) {
    editorError.textContent = err;
    return;
  }
  closeEditor();
  await renderRecipes();
  applySearch();
  setStatus("Recipe saved", "ok");
});

$("cancel-recipe").addEventListener("click", closeEditor);

deleteBtn.addEventListener("click", async () => {
  try {
    await invoke("delete_recipe", { id: editing.id });
  } catch (err) {
    editorError.textContent = err;
    return;
  }
  closeEditor();
  await renderRecipes();
  applySearch();
  setStatus("Recipe deleted");
});

// Typing needs keyboard focus, which the drawer doesn't take by default.
for (const input of [searchEl, nameEl]) {
  input.addEventListener("pointerdown", () => invoke("focus_drawer"));
}

// ---------------------------------------------------------------- drawer

$("close").addEventListener("click", () => invoke("hide_drawer"));
$("shelf-btn").addEventListener("click", () => invoke("toggle_shelf"));
$("settings-btn").addEventListener("click", () => invoke("open_settings"));
const showShelfCount = (items) => ($("shelf-count").textContent = items.length || "");
listen("ferryfile-shelf-changed", (e) => showShelfCount(e.payload));
invoke("shelf_items").then(showShelfCount);
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (editing) closeEditor();
  else invoke("hide_drawer");
});

listen("ferryfile-drag-start", (e) => {
  document.body.classList.toggle("left", e.payload === "left");
  reset();
  drawer.classList.add("open");
});
listen("ferryfile-drag-end", () => drawer.classList.remove("open", "hover"));

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
  window.chrome.webview.postMessageWithAdditionalObjects("ferryfile-drop", files);
});
listen("ferryfile-dropped", (e) => showFiles(e.payload));
