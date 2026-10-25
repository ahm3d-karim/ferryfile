import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { icon, hydrateIcons } from "./icons.js";

const $ = (id) => document.getElementById(id);
hydrateIcons();

const info = await invoke("tutorial_info");
const iconReady = invoke("drag_icon");
$("sample-name").textContent = info.sample.split(/[\\/]/).pop();
$("step-3-desc").textContent = `It slides in from the ${info.side} edge of the screen`;

if (info.dragHotkey) {
  const names = { Super: "Win", Backquote: "`" };
  $("keys").replaceChildren(
    ...info.dragHotkey.split("+").map((k) => Object.assign(document.createElement("span"), { className: "cap", textContent: names[k] ?? k })),
  );
} else {
  // "Opens when: Any drag": there's no key to learn.
  $("step-2-title").textContent = "Wait a moment while you drag";
  $("step-2-desc").textContent = "The drawer opens by itself when you drag a file out of Explorer";
}

// ---------------------------------------------------------------- steps

let step = 0;
let drawerOpened = false;

function setStep(n) {
  step = n;
  for (const li of document.querySelectorAll(".step")) {
    const s = Number(li.dataset.step);
    li.classList.toggle("done", s < n);
    li.classList.toggle("active", s === n);
    li.querySelector(".num").replaceChildren(s < n ? icon("check") : String(s));
  }
}

function feedback(kind, text) {
  const el = $("feedback");
  el.hidden = !kind;
  el.className = `feedback ${kind ?? ""}`;
  el.textContent = text ?? "";
}

function finish() {
  setStep(4);
  feedback("ok", "You're all set. Pick an action in the drawer, or press Esc to close it.");
  $("done").hidden = false;
  $("skip").hidden = true;
  $("done").focus();
}

setStep(1);

// The drawer opening is the moment the gesture worked.
listen("ferryfile-drag-start", () => {
  if (step < 2 || step > 3) return;
  drawerOpened = true;
  setStep(3);
  feedback("ok", "The drawer is open. Let go of the file over it.");
});

// ---------------------------------------------------------------- the sample drag

async function dragSample() {
  drawerOpened = false;
  setStep(2);
  feedback(null);
  invoke("tutorial_drag_started");
  await startDrag({ item: [info.sample], icon: await iconReady }, ({ result }) => {
    if (drawerOpened && result === "Dropped") return finish();
    if (drawerOpened) {
      feedback("warn", "Almost. Drag the sample again and let go over the drawer.");
      return setStep(1);
    }
    setStep(1);
    feedback(
      "warn",
      info.dragHotkey
        ? `The drawer didn't open. Press ${info.dragHotkey} while you're still holding the mouse button.`
        : "The drawer didn't open. Keep holding the mouse button for a moment.",
    );
  });
}

$("sample").addEventListener("pointerdown", (down) => {
  if (down.button !== 0 || step > 3) return;
  const move = (e) => {
    if (Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < 4) return;
    stop();
    dragSample();
  };
  const stop = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", stop);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", stop);
});

// Dropping the sample back here would make WebView2 open it as a page.
document.addEventListener("dragover", (e) => {
  e.preventDefault();
  e.dataTransfer.dropEffect = "none";
});
document.addEventListener("drop", (e) => e.preventDefault());

$("skip").addEventListener("click", () => invoke("close_tutorial"));
$("done").addEventListener("click", () => invoke("close_tutorial"));
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") invoke("close_tutorial");
});
