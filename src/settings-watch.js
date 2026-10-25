// Watch folders section of Settings. Each watch is a few rows in the
// section's card: the folder with an on/off switch, what to run, and where
// results go. Saved through the regular settings (state.watchFolders).
import { invoke } from "@tauri-apps/api/core";

const list = document.getElementById("watch-list");
const addRow = document.getElementById("watch-add-row");

let getState = () => ({ watchFolders: [] });
let save = async () => {};
let choices = { recipes: [], actions: [] };
let errors = {};

const OUTPUTS = [
  ["subfolder", "In “converted”"],
  ["beside", "Same folder"],
  ["folder", "Other…"],
];

function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === "class") node.className = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (k in node) node[k] = v;
    else node.setAttribute(k, v);
  }
  node.append(...children.filter((c) => c != null));
  return node;
}

const baseName = (p) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;

function taskSelect(w) {
  const select = el("select", { class: "select", "aria-label": "What to run" });
  select.append(el("option", { value: "", textContent: "Choose…", disabled: true }));
  const groups = new Map();
  const add = (group, value, title) => {
    if (!groups.has(group)) groups.set(group, el("optgroup", { label: group }));
    groups.get(group).append(el("option", { value, textContent: title }));
  };
  for (const r of choices.recipes) add(`Recipes · ${r.group}`, `recipe|${r.id}`, r.title);
  for (const a of choices.actions) add(a.group, `action|${a.id}`, a.title);
  select.append(...groups.values());
  select.value = w.target ? `${w.kind}|${w.target}` : "";
  // A recipe that was deleted still shows, so the choice isn't silently lost.
  if (w.target && select.value === "") {
    select.prepend(el("option", { value: `${w.kind}|${w.target}`, textContent: `${w.target} (missing)` }));
    select.value = `${w.kind}|${w.target}`;
  }
  select.addEventListener("change", () => {
    const [kind, ...rest] = select.value.split("|");
    update(w, { kind, target: rest.join("|") });
  });
  return select;
}

function outputSeg(w) {
  const name = `watchOut-${w.id}`;
  const seg = el("div", { class: "seg", role: "radiogroup", "aria-label": "Save results" });
  for (const [value, text] of OUTPUTS) {
    const input = el("input", { type: "radio", name, value, checked: w.output === value });
    seg.append(el("label", {}, input, text));
  }
  seg.addEventListener("change", async (e) => {
    const output = e.target.value;
    if (output === "folder" && !w.outputDir) {
      const dir = await invoke("pick_watch_folder", { title: "Save results to…" });
      if (!dir) return render();
      return update(w, { output, outputDir: dir });
    }
    update(w, { output });
  });
  return seg;
}

function describe(w) {
  if (errors[w.id]) return { text: errors[w.id], warn: true };
  if (!w.target) return { text: "Choose what to run on new files", warn: true };
  return { text: w.folder, warn: false };
}

function rows(w) {
  const desc = describe(w);
  const sw = el("input", {
    class: "switch",
    type: "checkbox",
    role: "switch",
    checked: w.enabled,
    "aria-label": `Watch ${baseName(w.folder)}`,
    onchange: (e) => update(w, { enabled: e.target.checked }),
  });
  const head = el(
    "label",
    { class: "item watch-head" },
    el(
      "span",
      { class: "item-text" },
      el("span", { class: "item-title", textContent: baseName(w.folder) }),
      el("span", { class: `item-desc${desc.warn ? " warn" : ""}`, textContent: desc.text, title: w.folder }),
    ),
    sw,
  );
  const run = el(
    "div",
    { class: "item sub" },
    el("span", { class: "item-text" }, el("span", { class: "item-title", textContent: "Run" })),
    taskSelect(w),
  );
  const out = el(
    "div",
    { class: "item sub" },
    el("span", { class: "item-text" }, el("span", { class: "item-title", textContent: "Save to" })),
    outputSeg(w),
  );
  const folder =
    w.output === "folder"
      ? el(
          "div",
          { class: "item sub" },
          el("button", {
            type: "button",
            class: "path",
            title: "Show in Explorer",
            textContent: w.outputDir ?? "",
            onclick: () => invoke("reveal", { path: w.outputDir }).catch(() => {}),
          }),
          el("button", {
            type: "button",
            class: "btn small",
            textContent: "Change…",
            onclick: async () => {
              const dir = await invoke("pick_watch_folder", { title: "Save results to…" });
              if (dir) update(w, { outputDir: dir });
            },
          }),
        )
      : null;
  const actions = el(
    "div",
    { class: "item sub watch-actions" },
    el("button", {
      type: "button",
      class: "link-btn",
      textContent: "Open folder",
      onclick: () => invoke("reveal", { path: w.folder }).catch(() => {}),
    }),
    el("button", {
      type: "button",
      class: "link-btn",
      textContent: "Remove",
      onclick: () => {
        const s = getState();
        s.watchFolders = s.watchFolders.filter((x) => x.id !== w.id);
        persist();
      },
    }),
  );
  return [head, run, out, folder, actions].filter(Boolean);
}

function update(w, changes) {
  Object.assign(w, changes);
  persist();
}

async function persist() {
  await save();
  errors = await invoke("watch_errors").catch(() => ({}));
  render();
}

export function render() {
  const watches = getState().watchFolders ?? [];
  list.replaceChildren(...watches.flatMap(rows), addRow);
}

async function addFolder() {
  const folder = await invoke("pick_watch_folder", { title: "Watch this folder" });
  if (!folder) return;
  const s = getState();
  s.watchFolders = [
    ...(s.watchFolders ?? []),
    { id: crypto.randomUUID(), folder, kind: "recipe", target: "", output: "subfolder", outputDir: null, enabled: true },
  ];
  persist();
}

export async function setupWatch(opts) {
  getState = opts.getState;
  save = opts.save;
  document.getElementById("watch-add").addEventListener("click", addFolder);
  [choices, errors] = await Promise.all([
    invoke("watch_choices").catch(() => choices),
    invoke("watch_errors").catch(() => ({})),
  ]);
  render();
}
