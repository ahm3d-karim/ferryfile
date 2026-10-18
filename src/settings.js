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
  $("delay").value = state.openDelayMs >= 1 ? "400" : "0";
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
