import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { icon, hydrateIcons, iconForPath } from "./icons.js";
import { customForm } from "./custom.js";
import { toolsFor, openTool, closeTool, isToolOpen, toolKeydown } from "./editing.js";

const $ = (id) => document.getElementById(id);
const drawer = $("drawer");
const idle = $("idle");
const content = $("content");
const toolsEl = $("tools");
const searchEl = $("search");
const groupsEl = $("groups");
const recipeRows = $("recipe-rows");
const suggestedBtn = $("suggested");

const editor = $("editor");
const nameEl = $("recipe-name");
const stepsEl = $("steps");
const addStepEl = $("add-step");
const editorError = $("editor-error");
const saveBtn = $("save-recipe");
const deleteBtn = $("delete-recipe");

hydrateIcons();

let paths = [];
let suggestedRun = null;
let fileInfo = null; // size and pixel dimensions of a single dropped file

const basename = (p) => p.split(/[\\/]/).pop();
const ext = (p) => (p.includes(".") ? p.split(".").pop().toLowerCase() : "");

function el(tag, props = {}, children = []) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

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

// ---------------------------------------------------------------- copy

/** Shorter labels where the engine's would crowd a segmented control. */
const SHORT = { "compress:75": "Balanced", "compress:50": "Smallest" };

const IMAGES = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];
const AUDIO = ["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "aiff", "aif", "wma"];
const VIDEO = ["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv"];

/** The action most people want for this file, if the engines offer it. */
function preferredIds(path, multiple) {
  if (multiple) return ["combine:pdf", "combine:video"];
  const e = ext(path);
  if (IMAGES.includes(e)) return e === "webp" ? ["convert:png"] : ["convert:webp", "convert:png"];
  if (AUDIO.includes(e)) return e === "mp3" ? ["convert:m4a"] : ["convert:mp3"];
  if (["mp4", "m4v"].includes(e)) return ["compress:28", "convert:webm"];
  if (["mov", "mkv", "webm", "avi", "wmv"].includes(e)) return ["convert:mp4"];
  if (e === "pdf") return ["text:md", "pages:png"];
  if (["csv", "tsv"].includes(e)) return ["convert:md", "convert:pdf"];
  if (["xlsx", "xlsm", "xlsb", "xls", "ods"].includes(e)) return ["convert:pdf", "convert:csv"];
  return ["convert:pdf", "convert:docx", "convert:md"];
}

// ---------------------------------------------------------------- info bar

let lastOutput = null;
let lastOutputs = [];

function setInfo(kind, text = "") {
  const bar = $("infobar");
  bar.hidden = !kind;
  if (!kind) return;
  bar.className = `infobar ${kind}`;
  $("info-text").textContent = text;
  const state = $("info-state");
  state.replaceChildren(icon(kind === "ok" ? "check" : kind === "error" ? "error" : "info"));
  $("progress").hidden = kind !== "busy";
  $("info-cancel").hidden = kind !== "busy";
  $("info-show").hidden = !(kind === "ok" && lastOutput);
  // A finished result can be dragged straight into another app.
  const draggableResult = kind === "ok" && lastOutputs.length > 0;
  bar.classList.toggle("can-drag", draggableResult);
  bar.title = draggableResult ? "Drag the result into any app" : "";
}
$("info-show").addEventListener("click", () => lastOutput && invoke("reveal", { path: lastOutput }));

// Drag the finished result out of the drawer (same native drag as shelves).
const iconReady = invoke("drag_icon");
$("infobar").addEventListener("pointerdown", (down) => {
  if (down.button !== 0 || !$("infobar").classList.contains("can-drag") || down.target.closest("button")) return;
  const move = async (e) => {
    if (Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < 4) return;
    stop();
    await startDrag({ item: [...lastOutputs], icon: await iconReady });
  };
  const stop = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", stop);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", stop);
});

// ---------------------------------------------------------------- jobs

// Jobs run one at a time; starting another while one runs queues it.
const queue = []; // { label, job, targets }
let running = null; // { label, index, count, jobId, cancelled, started }

listen("ferryfile-progress", (e) => {
  if (running) $("progress").value = (running.index + e.payload.fraction) / running.count;
});

const jobLabel = () => {
  const more = queue.length ? ` · ${queue.length} more queued` : "";
  return `${running.label}${running.count > 1 ? ` (${running.index + 1} of ${running.count})` : ""}…${more}`;
};

/**
 * Queues `job(path, jobId)` for every target (by default each dropped file);
 * each call gets a job id the Cancel button can stop.
 */
function runJob(label, job, targets = paths) {
  queue.push({ label, job, targets: [...targets] });
  if (running) setInfo("busy", jobLabel());
  else pump();
}

/** Long jobs, and anything finishing while the drawer is hidden, get a Windows notification. */
function notifyIfAway(started, body) {
  if (Date.now() - started > 8000 || !drawer.classList.contains("open")) {
    invoke("notify", { title: "Ferryfile", body });
  }
}

async function pump() {
  const task = queue.shift();
  if (!task) return;
  running = { label: task.label, index: 0, count: task.targets.length, jobId: null, cancelled: false, started: Date.now() };
  $("progress").value = 0;
  const results = [];
  let failure = null;
  for (const [index, p] of task.targets.entries()) {
    if (running.cancelled) break;
    running.index = index;
    running.jobId = crypto.randomUUID();
    setInfo("busy", jobLabel());
    try {
      results.push(await task.job(p, running.jobId));
    } catch (err) {
      if (String(err) !== "Cancelled") failure = `${basename(p)}: ${err}`;
      break;
    }
  }
  const { cancelled, started } = running;
  running = null;
  if (failure) {
    setInfo("error", failure);
    notifyIfAway(started, `Couldn't finish: ${failure}`);
  } else if (cancelled) {
    lastOutputs = [];
    setInfo("info", results.length ? `Cancelled after ${results.map(basename).join(", ")}` : "Cancelled");
  } else {
    lastOutputs = results;
    lastOutput = results.at(-1) ?? null;
    const saved = `Saved ${results.map(basename).join(", ")}`;
    setInfo("ok", saved);
    notifyIfAway(started, saved);
  }
  pump();
}

$("info-cancel").addEventListener("click", () => {
  if (!running) return;
  running.cancelled = true;
  queue.length = 0; // Cancel stops everything that was waiting, too
  if (running.jobId) invoke("cancel_job", { jobId: running.jobId });
  setInfo("busy", "Cancelling…");
});

/** Quick file actions: no progress, just a confirmation. */
async function quick(done, task) {
  try {
    const r = await task();
    if (r === false) return;
    lastOutput = null;
    lastOutputs = [];
    if (done) setInfo("ok", done);
  } catch (err) {
    setInfo("error", String(err));
  }
}

// ---------------------------------------------------------------- building blocks

function row(label, onClick, { hint = "", search = label, iconName = null, extra = null } = {}) {
  const b = el("button", { type: "button", className: "row" }, [
    ...(iconName ? [icon(iconName)] : []),
    el("span", { className: "label", textContent: label }),
    ...(hint ? [el("span", { className: "hint", textContent: hint })] : []),
    ...(extra ? [extra] : []),
    el("span", { className: "kbd", hidden: true }),
  ]);
  b.dataset.search = `${search} ${hint}`.toLowerCase();
  b.dataset.numbered = "1";
  b.addEventListener("click", (e) => {
    if (!e.target.closest(".edit")) onClick();
  });
  return b;
}

function segmented(items) {
  return el(
    "div",
    { className: "segmented" },
    items.map(({ label, title, search, run }) => {
      const b = el("button", { type: "button", textContent: label, title, onclick: run });
      b.dataset.search = search.toLowerCase();
      return b;
    }),
  );
}

function group(heading, body) {
  return el("section", { className: "group" }, [el("div", { className: "heading", textContent: heading }), body]);
}

const runAction = (a) => () =>
  runJob(`${a.group}: ${a.label}`, (p, jobId) => invoke("run_action", { actionId: a.id, path: p, jobId }));

// ---------------------------------------------------------------- custom values

let openCustom = null; // the inline "Custom…" form that's showing

function closeCustom() {
  openCustom?.remove();
  openCustom = null;
}

/** Shows `form`'s inline editor after `anchor`; `onAction(action)` gets the checked step. */
function showCustom(form, anchor, { steps = [], submitLabel, onAction }) {
  const same = openCustom?.dataset.group === form.group && openCustom.previousElementSibling === anchor;
  closeCustom();
  if (same) return; // clicking "Custom…" again closes it
  const node = customForm({
    form,
    info: paths.length === 1 ? fileInfo : null,
    isVideo: VIDEO.includes(ext(paths[0])),
    submitLabel,
    onCancel: closeCustom,
    onFocus: () => invoke("focus_drawer"),
    onSubmit: async (id) => {
      try {
        const action = await invoke("check_step", { path: paths[0], steps, step: id });
        closeCustom();
        onAction(action);
      } catch (err) {
        return err;
      }
    },
  });
  if (!node) return;
  node.dataset.group = form.group;
  anchor.after(node);
  openCustom = node;
  invoke("focus_drawer").then(() => node.focusFirst());
}

const VERBS = { Resize: "Resize", Compress: "Compress", "Shrink to": "Shrink" };

// ---------------------------------------------------------------- render

async function renderActions() {
  const [actions, combined, forms] = await Promise.all([
    invoke("list_actions", { path: paths[0] }),
    paths.length > 1 ? invoke("combine_actions", { paths }) : [],
    invoke("custom_forms", { path: paths[0] }),
  ]);
  const formFor = (name) => forms.find((f) => f.group === name);
  // Runs on every file, like the fixed choices.
  const customRun = (form) => () => {
    const anchor = groupsEl.querySelector(`[data-group="${CSS.escape(form.group)}"]`);
    const top = anchor?.parentElement?.classList.contains("pair") ? anchor.parentElement : anchor;
    showCustom(form, top, { submitLabel: VERBS[form.group] ?? "Run", onAction: (a) => runAction(a)() });
  };

  // Suggested: the most likely action, on Enter.
  const all = [...combined, ...actions];
  const pick = preferredIds(paths[0], paths.length > 1).map((id) => all.find((a) => a.id === id)).find(Boolean);
  suggestedBtn.hidden = !pick;
  suggestedRun = null;
  if (pick) {
    const isCombine = combined.includes(pick);
    const verb = pick.group === "Convert" ? `Convert to ${pick.label}` : `${pick.group}: ${pick.label}`;
    $("suggested-label").textContent =
      pick.id === "combine:video" ? "Merge videos" : isCombine ? `Combine into ${pick.label.replace(/^One /, "one ")}` : verb;
    suggestedBtn.dataset.search = `${verb} ${pick.id}`.toLowerCase();
    suggestedRun = pick.id === "combine:video"
      ? () => showTool("merge")
      : isCombine
      ? () => {
          const all = [...paths]; // the files as they are now, even if the job waits in the queue
          runJob(`Combining ${all.length} files`, (_, jobId) => invoke("run_combine", { actionId: pick.id, paths: all, jobId }), [all[0]]);
        }
      : runAction(pick);
  }

  const sections = [];

  if (combined.length) {
    const rows = combined
      .filter((a) => a !== pick)
      .map((a) =>
        row(a.label, () => {
          if (a.id === "combine:video") return showTool("merge");
          const all = [...paths];
          runJob(`${a.group}: ${a.label}`, (_, jobId) => invoke("run_combine", { actionId: a.id, paths: all, jobId }), [all[0]]);
        }, {
          search: `${a.group} ${a.label}`,
        }),
      );
    if (rows.length) sections.push(group("Combine", el("div", { className: "rows" }, rows)));
  }

  // Editing tools open an inline view (see editing.js).
  const tools = toolsFor(paths);
  if (tools.length) {
    const rows = tools.map((t) => row(t.label, () => showTool(t.id), { hint: t.hint, search: `edit ${t.label} ${t.id}` }));
    sections.push(group("Edit", el("div", { className: "rows" }, rows)));
  }

  // Short-label groups (Resize, Compress) become segmented controls, paired side by side.
  const pending = [];
  const flushPair = () => {
    if (!pending.length) return;
    sections.push(pending.length === 2 ? el("div", { className: "pair" }, pending.splice(0)) : pending.pop());
  };

  for (const [name, items] of Map.groupBy(actions, (a) => a.group)) {
    const shown = items.filter((a) => a !== pick);
    const form = formFor(name);
    if (!shown.length && !form) continue;
    const labels = shown.map((a) => SHORT[a.id] ?? a.label);
    if (shown.length && shown.length <= 4 && labels.every((l) => l.length <= 9) && name !== "Convert") {
      const buttons = shown.map((a, i) => ({
        label: labels[i],
        title: `${a.group}: ${a.label}`,
        search: `${a.group} ${a.label} ${labels[i]}`,
        run: runAction(a),
      }));
      const section = group(name, segmented(buttons));
      if (form) {
        // In the heading: a fourth segment wouldn't fit beside another group.
        const b = el("button", { type: "button", className: "icon-btn small custom-btn", textContent: "Custom…", title: `${name}: type your own value`, onclick: customRun(form) });
        b.dataset.search = `${name} custom own value`;
        section.firstChild.append(el("span", { className: "spacer" }), b);
      }
      section.dataset.group = name;
      pending.push(section);
      if (pending.length === 2) flushPair();
      continue;
    }
    flushPair();
    const heading = name === "Convert" ? "Convert to" : name;
    const rows = shown.map((a) => row(a.label, runAction(a), { search: `${a.group} ${a.label}` }));
    if (form) rows.push(row("Custom…", customRun(form), { search: `${name} custom own value` }));
    const body = el("div", { className: "rows" }, rows.slice(0, 5));
    if (rows.length > 5) {
      const rest = rows.slice(5);
      const more = row(`More: ${rest.map((r) => r.querySelector(".label").textContent).join(", ")}`, () => {
        more.replaceWith(...rest);
        renumber();
      });
      more.classList.add("more");
      more.append(icon("chevronRight"));
      more.dataset.search = rest.map((r) => r.dataset.search).join(" ");
      body.append(more);
    }
    const section = group(heading, body);
    section.dataset.group = name;
    sections.push(section);
  }
  flushPair();

  groupsEl.replaceChildren(...sections);
}

async function renderRecipes() {
  const recipes = await invoke("recipes_for", { path: paths[0] });
  recipeRows.replaceChildren(
    ...recipes.map((r) => {
      const edit = r.builtin
        ? null
        : el("button", { type: "button", className: "icon-btn edit", title: "Edit recipe", ariaLabel: "Edit recipe", onclick: () => openEditor(r) }, [icon("edit")]);
      return row(r.name, () => runJob(r.name, (p, jobId) => invoke("run_recipe", { id: r.id, path: p, jobId })), {
        hint: r.stepTitles.join(" · "),
        search: `recipe ${r.name}`,
        extra: edit,
      });
    }),
  );
  $("recipes-group").hidden = recipes.length === 0;
}

function renderFileGrid() {
  const n = paths.length === 1 ? basename(paths[0]) : `${paths.length} items`;
  const sendTo = (copy) => async () => {
    const moved = await invoke("send_to_folder", { paths, copy });
    if (!moved.length) return false; // cancelled
    if (!copy) await showFiles(moved); // keep acting on the files where they now are
  };
  const tile = (label, iconName, title, onClick) => {
    const b = el("button", { type: "button", className: "tile", title, onclick: onClick }, [icon(iconName), el("span", { textContent: label })]);
    b.dataset.search = `file ${label} ${title}`.toLowerCase();
    return b;
  };
  $("file-grid").replaceChildren(
    tile("Copy", "copy", "Copy the files, then paste in Explorer", () => quick(`Copied ${n}. Paste in Explorer.`, () => invoke("copy_files", { paths }))),
    tile("Copy path", "link", "Copy the full path as text", () => quick("Path copied", () => invoke("copy_paths", { paths }))),
    tile("Explorer", "explorer", "Show in File Explorer", () => quick("", () => invoke("reveal", { path: paths[0] }))),
    tile("Terminal", "terminal", "Open a terminal here", () => quick("", () => invoke("open_terminal", { path: paths[0] }))),
    tile("Move to…", "move", "Move to another folder", () => quick(`Moved ${n}`, sendTo(false))),
    tile("Copy to…", "copyTo", "Copy to another folder", () => quick(`Copied ${n}`, sendTo(true))),
    tile("Zip", "zip", "Make one .zip", () => {
      const all = [...paths];
      runJob("Zipping", () => invoke("zip_items", { paths: all }), [all[0]]);
    }),
    tile("To shelf", "shelfAdd", "Keep on a floating shelf (or shake while dragging)", () => quick(`${n} added to a shelf`, () => invoke("shelf_add", { id: null, paths }))),
  );
}

async function renderFileCard() {
  const thumb = $("file-thumb");
  if (paths.length > 1) {
    thumb.replaceChildren(icon("files"));
    $("file-name").textContent = `${paths.length} files`;
    $("file-meta").textContent = paths.map(basename).join(", ");
    return;
  }
  const p = paths[0];
  thumb.replaceChildren(icon(iconForPath(p)));
  $("file-name").textContent = basename(p);
  $("file-name").title = p;
  $("file-meta").textContent = "";
  fileInfo = null;
  try {
    const info = await invoke("file_info", { path: p });
    fileInfo = info;
    const parts = [info.kind];
    if (info.isDir) parts.push(`${info.items ?? 0} items`);
    else parts.push(formatBytes(info.size));
    if (info.width && info.height) parts.push(`${info.width} × ${info.height}`);
    $("file-meta").textContent = parts.join(" · ");
  } catch {
    $("file-meta").textContent = "";
  }
}

/** Gives the first nine visible rows their number-key shortcut. */
function renumber() {
  let n = 0;
  for (const r of toolsEl.querySelectorAll("[data-numbered]")) {
    const kbd = r.querySelector(".kbd");
    const visible = !r.hidden && !r.closest("[hidden]");
    if (visible && n < 9) {
      n++;
      kbd.textContent = String(n);
      kbd.hidden = false;
      r.dataset.key = String(n);
    } else {
      kbd.hidden = true;
      delete r.dataset.key;
    }
  }
}

function applySearch() {
  const q = searchEl.value.trim().toLowerCase();
  const match = (node) => !q || node.dataset.search?.includes(q);
  suggestedBtn.hidden = !suggestedRun || !match(suggestedBtn);
  for (const node of toolsEl.querySelectorAll(".row, .segmented button, .tile, .custom-btn")) {
    node.hidden = !match(node);
  }
  for (const section of toolsEl.querySelectorAll(".group")) {
    const any = section.querySelector(".row:not([hidden]), .segmented button:not([hidden]), .tile:not([hidden]), .custom-btn:not([hidden])");
    section.hidden = !any || (section.id === "recipes-group" && !recipeRows.children.length);
  }
  renumber();
}

/** Opens an editing view in place of the action list. */
function showTool(id) {
  closeEditor();
  closeCustom();
  toolsEl.hidden = true;
  openTool(id, {
    paths: [...paths],
    runJob,
    saveRecipe: (step) => {
      closeTool();
      openEditor(null, [step]);
    },
    onClose: () => {
      toolsEl.hidden = false;
      applySearch();
    },
  });
}

async function showFiles(dropped) {
  closeTool();
  paths = dropped;
  fileInfo = null;
  openCustom = null; // its node goes with the old groups
  idle.hidden = paths.length > 0;
  content.hidden = paths.length === 0;
  closeEditor();
  setInfo(null);
  if (!paths.length) return;

  searchEl.value = "";
  renderFileGrid();
  await Promise.all([renderFileCard(), renderRecipes(), renderActions()]);
  applySearch();
}

searchEl.addEventListener("input", applySearch);
suggestedBtn.addEventListener("click", () => suggestedRun?.());

// ---------------------------------------------------------------- editor

let editing = null; // { id: string | null, steps: [{ id, title }], outputDir: string | null }
let stepForms = []; // "Custom…" entries offered in the add-step list

/** `steps` pre-fills a new recipe, e.g. with a trim from the Trim view. */
function openEditor(recipe, steps = []) {
  closeCustom();
  editing = {
    id: recipe?.id ?? null,
    steps: recipe ? recipe.steps.map((id, i) => ({ id, title: recipe.stepTitles[i] })) : [...steps],
    outputDir: recipe?.outputDir ?? null,
  };
  $("editor-title").textContent = recipe ? "Edit recipe" : "New recipe";
  nameEl.value = recipe?.name ?? steps[0]?.title ?? "";
  deleteBtn.hidden = !recipe;
  toolsEl.hidden = true;
  editor.hidden = false;
  setInfo(null);
  refreshEditor();
  invoke("focus_drawer").then(() => nameEl.focus());
}

function closeEditor() {
  closeCustom();
  editing = null;
  editor.hidden = true;
  toolsEl.hidden = false;
}

async function refreshEditor() {
  stepsEl.replaceChildren(
    ...editing.steps.map((s, i) =>
      el("li", {}, [
        el("span", { textContent: s.title }),
        el(
          "button",
          {
            type: "button",
            className: "icon-btn",
            title: "Remove step",
            ariaLabel: "Remove step",
            onclick: () => {
              editing.steps.splice(i, 1);
              refreshEditor();
            },
          },
          [icon("close")],
        ),
      ]),
    ),
  );

  renderRecipeFolder();
  let next = [];
  stepForms = [];
  editorError.textContent = "";
  const steps = editing.steps.map((s) => s.id);
  try {
    [next, stepForms] = await Promise.all([
      invoke("next_actions", { path: paths[0], steps }),
      invoke("next_custom_forms", { path: paths[0], steps }),
    ]);
  } catch (err) {
    editorError.textContent = err;
  }
  // Each group's "Custom…" entry sits right after its fixed choices.
  const options = [];
  for (const [name, items] of Map.groupBy(next, (a) => a.group)) {
    options.push(...items.map((a) => el("option", { value: a.id, textContent: `${a.group} ${a.label}` })));
    const i = stepForms.findIndex((f) => f.group === name);
    if (i >= 0) options.push(el("option", { value: `form:${i}`, textContent: `${name} custom…` }));
  }
  addStepEl.replaceChildren(
    el("option", { value: "", textContent: next.length ? "Add a step…" : "No further steps for this format" }),
    ...options,
  );
  addStepEl.disabled = !next.length;
  saveBtn.disabled = !!editorError.textContent || !editing.steps.length;
}

addStepEl.addEventListener("change", () => {
  const opt = addStepEl.selectedOptions[0];
  if (!opt?.value) return;
  if (opt.value.startsWith("form:")) {
    addStepEl.value = "";
    const form = stepForms[Number(opt.value.slice(5))];
    showCustom(form, addStepEl, {
      steps: editing.steps.map((s) => s.id),
      submitLabel: "Add step",
      onAction: (a) => {
        editing.steps.push({ id: a.id, title: `${a.group} ${a.label}` });
        refreshEditor();
      },
    });
    return;
  }
  closeCustom();
  editing.steps.push({ id: opt.value, title: opt.textContent });
  refreshEditor();
});

// A recipe can save somewhere other than the "Save to" folder in Settings.
function renderRecipeFolder() {
  const dir = editing.outputDir;
  $("recipe-folder").textContent = dir ?? "Same as Settings";
  $("recipe-folder").title = dir ?? "Uses the Save to choice in Settings";
  $("recipe-folder").classList.toggle("unset", !dir);
  $("recipe-folder-clear").hidden = !dir;
}
$("recipe-folder-pick").addEventListener("click", async () => {
  const dir = await invoke("pick_recipe_folder");
  if (dir && editing) {
    editing.outputDir = dir;
    renderRecipeFolder();
  }
});
$("recipe-folder-clear").addEventListener("click", () => {
  editing.outputDir = null;
  renderRecipeFolder();
});

editor.addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    await invoke("save_recipe", { id: editing.id, name: nameEl.value, steps: editing.steps.map((s) => s.id), outputDir: editing.outputDir });
  } catch (err) {
    editorError.textContent = err;
    return;
  }
  closeEditor();
  await renderRecipes();
  applySearch();
  lastOutput = null;
  setInfo("ok", "Recipe saved");
});

$("cancel-recipe").addEventListener("click", closeEditor);
$("new-recipe").addEventListener("click", () => openEditor(null));

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
  lastOutput = null;
  setInfo("ok", "Recipe deleted");
});

// Typing needs keyboard focus, which the drawer doesn't take until a drop.
for (const input of [searchEl, nameEl]) {
  input.addEventListener("pointerdown", () => invoke("focus_drawer"));
}

// ---------------------------------------------------------------- keyboard

window.addEventListener("keydown", (e) => {
  const typing = e.target.matches("input, select, textarea");
  if (e.target.closest?.(".custom-form")) return; // the form handles Enter and Esc
  if (isToolOpen()) {
    if (e.key === "Escape") {
      closeTool();
      toolsEl.hidden = false;
      applySearch();
    } else toolKeydown(e);
    return;
  }
  if (editing) {
    if (e.key === "Escape") closeEditor();
    return;
  }
  if (e.key === "Escape") {
    if (searchEl.value) {
      searchEl.value = "";
      applySearch();
    } else invoke("hide_drawer");
    return;
  }
  if (!paths.length) return;
  if ((e.key === "k" && e.ctrlKey) || (e.key === "/" && !typing)) {
    e.preventDefault();
    searchEl.focus();
    return;
  }
  if (e.key === "Enter" && !e.target.matches("button")) {
    e.preventDefault();
    if (typing && searchEl.value && suggestedBtn.hidden) toolsEl.querySelector("[data-key='1']")?.click();
    else if (!suggestedBtn.hidden) suggestedRun?.();
    return;
  }
  if (!typing && /^[1-9]$/.test(e.key)) toolsEl.querySelector(`[data-key='${e.key}']`)?.click();
});

// ---------------------------------------------------------------- drawer

$("close").addEventListener("click", () => invoke("hide_drawer"));
$("shelf-btn").addEventListener("click", () => invoke("toggle_shelves"));
$("settings-btn").addEventListener("click", () => invoke("open_settings"));
const showShelfCount = (total) => ($("shelf-count").textContent = total || "");
listen("ferryfile-shelves-count", (e) => showShelfCount(e.payload));
invoke("shelves_count").then(showShelfCount);

listen("ferryfile-drag-start", (e) => {
  document.body.classList.toggle("left", e.payload === "left");
  paths = [];
  idle.hidden = false;
  content.hidden = true;
  setInfo(null);
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
listen("ferryfile-dropped", async (e) => {
  await showFiles(e.payload);
  // The drag is over: take keyboard focus so Enter and 1–9 work right away.
  invoke("focus_drawer");
});

// Files sent from Explorer's right-click menu or Send to: shown like a drop.
// Also checked once at load, in case they came in before this page did.
let lastOpened = { paths: null, at: 0 };
async function showOpened() {
  const { side, paths: opened } = await invoke("take_opened_paths");
  if (!opened.length) return;
  document.body.classList.toggle("left", side === "left");
  drawer.classList.add("open");
  // Explorer starts one launch per selected file; a slow one can arrive a
  // moment after the rest. Add it to what's showing instead of replacing it.
  const late = paths === lastOpened.paths && Date.now() - lastOpened.at < 3000;
  const all = late ? [...new Set([...paths, ...opened])] : opened;
  lastOpened = { paths: all, at: Date.now() };
  await showFiles(all);
}
// Listen first, then check: paths that land in between still get picked up.
listen("ferryfile-opened", showOpened).then(showOpened);
