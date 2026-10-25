// Batch queue: when an action or recipe runs on several files, one row per
// file with its own progress, status and cancel, plus Cancel all / Retry
// failed. Used by the drawer and by shelves. Jobs go through the usual
// commands (run_action, run_recipe, cancel_job) and "ferryfile-progress"
// events; this only decides how many run at once and shows them.

import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { icon, iconForPath } from "./icons.js";

const basename = (p) => p.split(/[\\/]/).pop();
const isCancel = (err) => String(err) === "Cancelled";
/** Errors that mean "this action doesn't apply to this file". */
const isUnsupported = (err) => /No engine can run|can't run on/.test(String(err));

function el(tag, props = {}, children = []) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

/**
 * A queue view rendered into `root` (an empty element, hidden while unused).
 * `onChange(summary)` runs whenever a row changes; `onDone(outputs, summary)`
 * each time the queue goes idle, with the outputs finished since last time.
 */
export function createQueue(root, { onChange = () => {}, onDone = () => {} } = {}) {
  let rows = []; // { path, status, fraction, output, error, jobId, node }
  let job = null;
  let limit = 1;
  let fresh = []; // outputs since the last onDone
  let idleWaiters = [];
  const iconReady = invoke("drag_icon");

  const title = el("span", { className: "q-title" });
  const retryBtn = el("button", { type: "button", className: "btn small", textContent: "Retry failed", hidden: true });
  const cancelBtn = el("button", { type: "button", className: "btn small", textContent: "Cancel all", hidden: true });
  const clearBtn = el("button", { type: "button", className: "icon-btn q-btn", hidden: true, title: "Close this list", ariaLabel: "Close this list" }, [icon("close")]);
  const list = el("ul", { className: "q-rows" });
  root.classList.add("queue");
  const actions = el("div", { className: "q-actions" }, [retryBtn, cancelBtn]);
  root.replaceChildren(el("div", { className: "q-head" }, [el("div", { className: "q-top" }, [title, clearBtn]), actions]), list);

  listen("ferryfile-progress", (e) => {
    const row = rows.find((r) => r.status === "running" && r.path === e.payload.path);
    if (!row) return;
    row.fraction = Math.max(row.fraction, e.payload.fraction);
    update(row);
  });

  const busy = () => rows.some((r) => r.status === "waiting" || r.status === "running");

  function summary() {
    const count = (s) => rows.filter((r) => r.status === s).length;
    const total = rows.length;
    const done = count("done");
    const settled = rows.filter((r) => !["waiting", "running"].includes(r.status)).length;
    const fraction = total ? rows.reduce((sum, r) => sum + (r.status === "running" ? r.fraction : r.status === "waiting" ? 0 : 1), 0) / total : 0;
    return { total, done, settled, failed: count("failed"), cancelled: count("cancelled"), skipped: count("skipped"), running: count("running"), busy: busy(), fraction };
  }

  function stateText(r) {
    switch (r.status) {
      case "waiting":
        return "Waiting";
      case "running":
        return r.fraction > 0 ? `${Math.round(r.fraction * 100)}%` : "Running…";
      case "done":
        return `Done · ${basename(r.output)}`;
      case "failed":
        return `Failed: ${r.error}`;
      case "skipped":
        return "Skipped: not supported for this file";
      default:
        return "Cancelled";
    }
  }

  function update(r) {
    const n = r.node;
    n.className = `q-row ${r.status}`;
    n.querySelector(".q-state").textContent = stateText(r);
    n.querySelector(".q-state").title = r.status === "failed" ? r.error : "";
    const bar = n.querySelector("progress");
    bar.hidden = r.status !== "running";
    if (r.fraction > 0) bar.value = r.fraction;
    else bar.removeAttribute("value"); // indeterminate until the engine reports
    const btn = n.querySelector(".q-btn");
    const live = r.status === "waiting" || r.status === "running";
    btn.hidden = !(live || r.status === "failed" || r.status === "cancelled");
    btn.replaceChildren(live ? icon("close") : el("span", { textContent: "Retry" }));
    btn.className = live ? "icon-btn q-btn" : "icon-btn small q-btn";
    btn.title = live ? "Cancel this file" : "Try this file again";
    btn.ariaLabel = `${live ? "Cancel" : "Retry"} ${basename(r.path)}`;
    n.title = r.status === "done" ? `${r.output}\nDrag into any app · double-click to show in Explorer` : r.path;
    head();
  }

  function head() {
    const s = summary();
    const parts = [`${s.done} of ${s.total} done`];
    if (s.failed) parts.push(`${s.failed} failed`);
    if (s.cancelled) parts.push(`${s.cancelled} cancelled`);
    if (s.skipped) parts.push(`${s.skipped} skipped`);
    title.textContent = parts.join(" · ");
    cancelBtn.hidden = !s.busy;
    retryBtn.hidden = !(s.failed + s.cancelled);
    clearBtn.hidden = s.busy;
    actions.hidden = cancelBtn.hidden && retryBtn.hidden;
    onChange(s);
  }

  function makeRow(path) {
    const r = { path, status: "waiting", fraction: 0, output: null, error: "", jobId: null };
    const btn = el("button", { type: "button", className: "icon-btn q-btn" });
    btn.addEventListener("click", () => (r.status === "failed" || r.status === "cancelled" ? retry([r]) : cancel(r)));
    r.node = el("li", {}, [
      icon(iconForPath(path)),
      el("div", { className: "q-text" }, [
        el("span", { className: "q-name", textContent: basename(path) }),
        el("span", { className: "q-state" }),
        el("progress", { max: 1 }),
      ]),
      btn,
    ]);
    // Finished rows drag out like the drawer's result bar.
    r.node.addEventListener("pointerdown", (down) => {
      if (down.button !== 0 || r.status !== "done" || down.target.closest("button")) return;
      const move = async (e) => {
        if (Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < 4) return;
        stop();
        await startDrag({ item: [r.output], icon: await iconReady });
      };
      const stop = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", stop);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", stop);
    });
    r.node.addEventListener("dblclick", () => r.status === "done" && invoke("reveal", { path: r.output }));
    return r;
  }

  async function start(r) {
    r.status = "running";
    r.fraction = 0;
    r.jobId = crypto.randomUUID();
    const jobId = r.jobId;
    update(r);
    try {
      const output = await job(r.path, jobId);
      if (r.jobId !== jobId) return;
      Object.assign(r, { status: "done", output, fraction: 1 });
      fresh.push(output);
    } catch (err) {
      if (r.jobId !== jobId) return;
      r.status = isCancel(err) ? "cancelled" : isUnsupported(err) ? "skipped" : "failed";
      r.error = String(err);
    }
    update(r);
    pump();
  }

  function pump() {
    let running = rows.filter((r) => r.status === "running").length;
    for (const r of rows) {
      if (running >= limit) break;
      if (r.status === "waiting") {
        running++;
        start(r);
      }
    }
    if (!busy()) {
      const outputs = fresh;
      fresh = [];
      onDone(outputs, summary());
      idleWaiters.splice(0).forEach((resolve) => resolve(outputs));
    }
  }

  function cancel(r) {
    if (r.status === "waiting") {
      r.status = "cancelled";
      update(r);
      pump(); // may have been the last one
    } else if (r.status === "running") {
      invoke("cancel_job", { jobId: r.jobId });
      r.node.querySelector(".q-state").textContent = "Cancelling…";
    }
  }

  function retry(which) {
    if (!job) return;
    for (const r of which) {
      Object.assign(r, { status: "waiting", fraction: 0, error: "", output: null });
      update(r);
    }
    pump();
  }

  cancelBtn.addEventListener("click", () => {
    // Waiting rows first, so a finishing job doesn't start the next one.
    for (const r of rows.filter((r) => r.status === "waiting")) cancel(r);
    for (const r of rows.filter((r) => r.status === "running")) cancel(r);
  });
  retryBtn.addEventListener("click", () => retry(rows.filter((r) => r.status === "failed" || r.status === "cancelled")));
  clearBtn.addEventListener("click", () => api.clear());

  const api = {
    /**
     * Runs `run(path, jobId)` for every target, `max` at a time. Resolves
     * with the outputs once nothing is waiting or running.
     */
    run({ targets, run, max = 1 }) {
      job = run;
      limit = Math.max(1, max);
      fresh = [];
      rows = targets.map(makeRow);
      list.replaceChildren(...rows.map((r) => r.node));
      rows.forEach(update);
      root.hidden = false;
      const done = new Promise((resolve) => idleWaiters.push(resolve));
      pump();
      return done;
    },
    /** Resolves once the queue is idle (right away if it is). */
    idle: () => (busy() ? new Promise((resolve) => idleWaiters.push(resolve)) : Promise.resolve([])),
    busy,
    summary,
    /** Every finished output, in list order. */
    outputs: () => rows.filter((r) => r.status === "done").map((r) => r.output),
    cancelAll: () => cancelBtn.click(),
    clear() {
      if (busy()) return;
      rows = [];
      job = null;
      list.replaceChildren();
      root.hidden = true;
      onChange(summary());
    },
  };
  return api;
}

/** How many of these files to run at once (from the engines; 1 if unsure). */
export async function batchLimit(path, steps) {
  if (!steps?.length) return 1;
  try {
    return await invoke("batch_limit", { path, steps });
  } catch {
    return 1;
  }
}
