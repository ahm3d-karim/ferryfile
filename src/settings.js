import { invoke } from "@tauri-apps/api/core";
import { setupWatch, render as renderWatch } from "./settings-watch.js";

const $ = (id) => document.getElementById(id);
const statusEl = $("status");

let state = await invoke("get_settings");

// Segmented choices are radio groups: <div class="seg" data-field="…">.
const seg = (field) => document.querySelector(`.seg[data-field="${field}"]`);
function setSeg(field, value) {
  const input = seg(field).querySelector(`input[value="${value}"]`);
  if (input) input.checked = true;
}
function onSeg(field, fn) {
  seg(field).addEventListener("change", (e) => fn(e.target.value));
}

// "Ctrl+Alt+Space" -> three keycaps.
function showKeycaps(button, combo) {
  const keys = showKeys(combo);
  if (!keys) return (button.textContent = "None");
  button.replaceChildren(
    ...keys.split("+").map((k) => {
      const cap = document.createElement("span");
      cap.className = "cap";
      cap.textContent = k;
      return cap;
    }),
  );
  button.setAttribute("aria-label", `${keys}. Click to change`);
}

function render() {
  $("autostart").checked = state.autostart;
  setSeg("drawerSide", state.drawerSide);
  setSeg("openOn", state.openOn);
  $("delay-card").hidden = state.openOn !== "drag";
  $("drag-hotkey-card").hidden = state.openOn !== "hotkey";
  setSeg("delay", state.openDelayMs >= 1 ? "400" : "0");
  for (const r of Object.values(RECORDERS)) {
    r.reset.hidden = state[r.field] === r.fallback;
    if (recording === r) continue;
    showKeycaps(r.button, state[r.field]);
    r.button.classList.remove("recording");
  }
  const warn = $("hotkey-desc");
  warn.hidden = !(state.hotkey && !state.hotkeyActive);
  warn.textContent = "Another app already uses this combo. Pick a different one.";
  $("shake").checked = state.shakeToShelf;
  setSeg("shakeSensitivity", state.shakeSensitivity);
  $("sensitivity-card").classList.toggle("off", !state.shakeToShelf);
  setSeg("output", state.outputDir ? "folder" : "next");
  $("folder-card").hidden = !state.outputDir;
  $("folder-path").textContent = state.outputDir ?? "";
  $("about").textContent = `Ferryfile ${state.version}`;
  renderWatch();
}

async function save() {
  try {
    await invoke("set_settings", {
      settings: {
        openOn: state.openOn,
        hotkey: state.hotkey,
        dragHotkey: state.dragHotkey,
        drawerSide: state.drawerSide,
        openDelayMs: state.openDelayMs,
        outputDir: state.outputDir,
        shakeToShelf: state.shakeToShelf,
        shakeSensitivity: state.shakeSensitivity,
        watchFolders: state.watchFolders,
      },
      autostart: state.autostart,
    });
    statusEl.textContent = "";
    statusEl.className = "";
  } catch (err) {
    statusEl.textContent = String(err);
    statusEl.className = "error";
    state = await invoke("get_settings");
  }
  render();
setupWatch({ getState: () => state, save });
}

async function chooseFolder() {
  const dir = await invoke("pick_output_dir");
  if (dir) state.outputDir = dir;
  return Boolean(dir);
}

$("autostart").addEventListener("change", (e) => {
  state.autostart = e.target.checked;
  save();
});
onSeg("drawerSide", (v) => {
  state.drawerSide = v;
  save();
});
onSeg("openOn", (v) => {
  state.openOn = v;
  save();
});

// ---- hotkey recorders
// Combos are stored in the form the Rust side reads ("Ctrl+Alt+Space",
// "Super" for the Windows key) and shown with "Win" instead.
//  - drag hotkey: only caught mid-drag, so a single key like Space is fine.
//  - drawer hotkey: works any time, so it needs a modifier (or an F-key).
const RECORDERS = {
  drag: { button: $("drag-hotkey"), reset: $("drag-hotkey-reset"), field: "dragHotkey", fallback: "Space", needsModifier: false },
  drawer: { button: $("hotkey"), reset: $("hotkey-reset"), field: "hotkey", fallback: "Ctrl+Alt+Space", needsModifier: true },
};
let recording = null;
const showKeys = (k) => (k || "").replace("Super", "Win");
const MODIFIERS = ["Control", "Alt", "Shift", "Meta", "OS"];

function keyName(code) {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  return code; // Space, F5, ArrowUp, Backquote, Numpad1…
}

async function stopRecording() {
  recording = null;
  window.removeEventListener("keydown", onRecordKey, true);
  await invoke("pause_hotkey", { paused: false }).catch(() => {});
}

async function onRecordKey(e) {
  e.preventDefault();
  e.stopPropagation();
  const r = recording;
  if (!r || MODIFIERS.includes(e.key)) return; // wait for the actual key
  if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) {
    await stopRecording();
    return render();
setupWatch({ getState: () => state, save });
  }
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean);
  const key = keyName(e.code);
  // A plain letter on its own would fire while typing anywhere.
  if (r.needsModifier && !mods.length && !/^F\d{1,2}$/.test(key)) {
    r.button.textContent = "Add Ctrl, Alt or Shift…";
    return;
  }
  await stopRecording();
  state[r.field] = [...mods, key].join("+");
  save();
}

for (const r of Object.values(RECORDERS)) {
  r.button.addEventListener("click", async () => {
    const was = recording;
    if (was) await stopRecording();
    if (was === r) return render();
setupWatch({ getState: () => state, save });
    render();
setupWatch({ getState: () => state, save });
    recording = r;
    // Turn the drawer hotkey off meanwhile, so pressing it reaches this page.
    await invoke("pause_hotkey", { paused: true }).catch(() => {});
    r.button.textContent = "Press keys · Esc cancels";
    r.button.classList.add("recording");
    window.addEventListener("keydown", onRecordKey, true);
  });
  r.button.addEventListener("blur", async () => {
    if (recording === r) {
      await stopRecording();
      render();
setupWatch({ getState: () => state, save });
    }
  });
  r.reset.title = `Back to ${r.fallback}`;
  r.reset.addEventListener("click", () => {
    state[r.field] = r.fallback;
    save();
  });
}

onSeg("delay", (v) => {
  state.openDelayMs = Number(v);
  save();
});
$("shake").addEventListener("change", (e) => {
  state.shakeToShelf = e.target.checked;
  save();
});
onSeg("shakeSensitivity", (v) => {
  state.shakeSensitivity = v;
  save();
});
onSeg("output", async (v) => {
  if (v === "folder") {
    // Choosing "A folder I choose" asks for the folder right away.
    if (!state.outputDir && !(await chooseFolder())) return render();
setupWatch({ getState: () => state, save });
  } else {
    state.outputDir = null;
  }
  save();
});
$("pick").addEventListener("click", async () => {
  if (await chooseFolder()) save();
});
$("folder-path").addEventListener("click", () => invoke("reveal", { path: state.outputDir }).catch(() => {}));
$("config").addEventListener("click", () => invoke("reveal", { path: state.configDir }).catch(() => {}));

render();
setupWatch({ getState: () => state, save });
