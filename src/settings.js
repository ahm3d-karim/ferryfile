import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { hydrateIcons } from "./icons.js";

const $ = (id) => document.getElementById(id);
const statusEl = $("status");

hydrateIcons();

let state = await invoke("get_settings");

function render() {
  $("autostart").checked = state.autostart;
  $("autostart-state").textContent = state.autostart ? "On" : "Off";
  $("side").value = state.drawerSide;
  $("open-on").value = state.openOn;
  $("open-desc").textContent =
    state.openOn === "hotkey"
      ? `Start dragging a file, then press ${showKeys(state.dragHotkey) || "your drag hotkey"}`
      : "Slides in as soon as you drag a file from Explorer";
  $("delay-card").hidden = state.openOn !== "drag";
  $("drag-hotkey-card").hidden = state.openOn !== "hotkey";
  $("delay").value = state.openDelayMs >= 1 ? "400" : "0";
  for (const r of Object.values(RECORDERS)) {
    if (recording === r) continue;
    r.button.textContent = showKeys(state[r.field]) || "None";
    r.button.classList.remove("recording");
  }
  const desc = $("hotkey-desc");
  const broken = state.hotkey && !state.hotkeyActive;
  desc.textContent = broken
    ? "Not working: another app is using this combo. Pick a different one."
    : "Opens or closes the drawer any time";
  desc.classList.toggle("warn", broken);
  $("shake").checked = state.shakeToShelf;
  $("shake-state").textContent = state.shakeToShelf ? "On" : "Off";
  $("sensitivity").value = state.shakeSensitivity;
  $("sensitivity").disabled = !state.shakeToShelf;
  $("output").value = state.outputDir ? "folder" : "next";
  $("folder-card").hidden = !state.outputDir;
  $("folder-path").textContent = state.outputDir ?? "Next to the original file";
  $("about").textContent = `Ferryfile ${state.version} · settings in ${state.configDir}`;
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
$("side").addEventListener("change", (e) => {
  state.drawerSide = e.target.value;
  save();
});
$("open-on").addEventListener("change", (e) => {
  state.openOn = e.target.value;
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
    render();
    recording = r;
    // Turn the drawer hotkey off meanwhile, so pressing it reaches this page.
    await invoke("pause_hotkey", { paused: true }).catch(() => {});
    r.button.textContent = "Press keys… (Esc cancels)";
    r.button.classList.add("recording");
    window.addEventListener("keydown", onRecordKey, true);
  });
  r.button.addEventListener("blur", async () => {
    if (recording === r) {
      await stopRecording();
      render();
    }
  });
  r.reset.title = `Back to ${r.fallback}`;
  r.reset.addEventListener("click", () => {
    state[r.field] = r.fallback;
    save();
  });
}

$("delay").addEventListener("change", (e) => {
  state.openDelayMs = Number(e.target.value);
  save();
});
$("shake").addEventListener("change", (e) => {
  state.shakeToShelf = e.target.checked;
  save();
});
$("sensitivity").addEventListener("change", (e) => {
  state.shakeSensitivity = e.target.value;
  save();
});
$("output").addEventListener("change", async (e) => {
  if (e.target.value === "folder") {
    // Choosing "A folder I choose" asks for the folder right away.
    if (!state.outputDir && !(await chooseFolder())) return render();
  } else {
    state.outputDir = null;
  }
  save();
});
$("pick").addEventListener("click", async () => {
  if (await chooseFolder()) save();
});
$("close").addEventListener("click", () => getCurrentWindow().hide());

render();
