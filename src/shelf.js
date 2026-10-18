import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { startDrag } from "@crabnebula/tauri-plugin-drag";

const $ = (id) => document.getElementById(id);
const shelf = $("shelf");
const list = $("items");
const empty = $("empty");
const count = $("count");
const dragAll = $("drag-all");
const clear = $("clear");

let items = [];
const iconReady = invoke("drag_icon");

const basename = (p) => p.split(/[\\/]/).pop();
const folder = (p) => p.split(/[\\/]/).slice(-2, -1)[0] ?? "";

/** Native drag of `paths` out of the shelf; dropped items leave the shelf. */
async function dragOut(paths) {
  const icon = await iconReady;
  await startDrag({ item: paths, icon }, ({ result }) => {
    if (result === "Dropped") invoke("shelf_remove", { paths });
  });
}

/** Starts a drag once the pointer has moved a few pixels with the button down. */
function draggable(node, paths) {
  node.addEventListener("pointerdown", (down) => {
    if (down.button !== 0 || down.target.closest("button")) return;
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

function render() {
  empty.hidden = items.length > 0;
  dragAll.hidden = clear.hidden = items.length === 0;
  count.textContent = items.length ? String(items.length) : "";
  list.replaceChildren(
    ...items.map((p) => {
      const li = document.createElement("li");
      li.title = `${p}\nDrag out to use · double-click to show in Explorer`;
      const name = Object.assign(document.createElement("span"), { className: "name", textContent: basename(p) });
      const where = Object.assign(document.createElement("span"), { className: "where", textContent: folder(p) });
      const remove = Object.assign(document.createElement("button"), {
        className: "icon",
        textContent: "×",
        title: "Remove from shelf",
        onclick: () => invoke("shelf_remove", { paths: [p] }),
      });
      li.append(Object.assign(document.createElement("div"), { className: "text" }), remove);
      li.firstChild.append(name, where);
      li.addEventListener("dblclick", () => invoke("reveal", { path: p }));
      draggable(li, () => [p]);
      return li;
    }),
  );
}

draggable(dragAll, () => items);
clear.addEventListener("click", () => invoke("shelf_clear"));
$("hide").addEventListener("click", () => invoke("hide_shelf"));

listen("skiff-shelf-changed", (e) => {
  items = e.payload;
  render();
});

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
    window.chrome.webview.postMessageWithAdditionalObjects("skiff-drop", e.dataTransfer.files);
  }
});
listen("skiff-dropped", (e) => invoke("shelf_add", { paths: e.payload }));

items = await invoke("shelf_items");
render();
