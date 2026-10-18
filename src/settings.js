import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

const $ = (id) => document.getElementById(id);
const statusEl = $("status");
const folderPath = $("folder-path");

let state = await invoke("get_settings");

const radio = (name, value) => {
  const input = document.querySelector(`input[name="${name}"][value="${value}"]`);
  if (input) input.checked = true;
};

function render() {
  $("autostart").checked = state.autostart;
  radio("side", state.drawerSide);
  radio("delay", String(state.openDelayMs >= 1 ? 400 : 0));
  radio("output", state.outputDir ? "folder" : "next");
  folderPath.textContent = state.outputDir ?? "";
  $("shake").checked = state.shakeToShelf;
  $("sensitivity").value = state.shakeSensitivity;
  $("sensitivity").disabled = !state.shakeToShelf;
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
    statusEl.textContent = "Saved";
    statusEl.className = "ok";
  } catch (err) {
    statusEl.textContent = String(err);
    statusEl.className = "error";
    state = await invoke("get_settings");
    render();
  }
}

async function chooseFolder() {
  const dir = await invoke("pick_output_dir");
  if (dir) state.outputDir = dir;
  render();
  return Boolean(dir);
}

$("shake").addEventListener("change", (e) => {
  state.shakeToShelf = e.target.checked;
  render();
  save();
});
$("sensitivity").addEventListener("change", (e) => {
  state.shakeSensitivity = e.target.value;
  save();
});
$("autostart").addEventListener("change", (e) => {
  state.autostart = e.target.checked;
  save();
});
document.addEventListener("change", async (e) => {
  const { name, value } = e.target;
  if (name === "side") state.drawerSide = value;
  else if (name === "delay") state.openDelayMs = Number(value);
  else if (name === "output") {
    // Picking "In a folder" without a folder yet asks for one.
    if (value === "folder" && !state.outputDir && !(await chooseFolder())) return render();
    if (value === "next") state.outputDir = null;
  } else return;
  save();
});
$("pick").addEventListener("click", async () => {
  if (await chooseFolder()) save();
});
$("close").addEventListener("click", () => getCurrentWindow().hide());

render();
