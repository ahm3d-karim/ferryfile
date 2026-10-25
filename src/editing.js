// Editing views shown inline in the drawer: Trim (audio/video), Crop
// (images), Merge (2+ videos) and Split pages (PDF). Each view only picks
// values; the edit itself is a normal action id (trim:…, crop:…, split:…,
// extract:…, combine:video) that Rust validates and runs, so the same id can
// also be saved as a recipe step.
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { icon } from "./icons.js";

const ext = (p) => (p.includes(".") ? p.split(".").pop().toLowerCase() : "");
const basename = (p) => p.split(/[\\/]/).pop();

const VIDEO = ["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv"];
const AUDIO = ["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "aiff", "aif", "wma"];
const CROPPABLE = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "heic", "heif", "avif"];

function el(tag, props = {}, children = []) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

const view = () => document.getElementById("tool-view");
let current = null; // { close(), keydown(e) }

/** The editing tools for the dropped files, as rows for the drawer's Edit group. */
export function toolsFor(paths) {
  if (paths.length !== 1) return [];
  const e = ext(paths[0]);
  if (VIDEO.includes(e) || AUDIO.includes(e)) return [{ id: "trim", label: "Trim", hint: "Cut the start or end" }];
  if (CROPPABLE.includes(e)) return [{ id: "crop", label: "Crop", hint: "Drag a frame" }];
  if (e === "pdf") return [{ id: "pages", label: "Split or extract pages", hint: "e.g. 1-3, 5, 8-" }];
  return [];
}

export const isToolOpen = () => current !== null;

export function closeTool() {
  current?.close();
  current = null;
  view().hidden = true;
  view().replaceChildren();
}

/** Keys for the open view; returns true when handled. */
export const toolKeydown = (e) => current?.keydown?.(e) ?? false;

/**
 * Opens tool `id` ("trim", "crop", "pages", "merge") for `ctx.paths`.
 * ctx: { paths, runJob(label, job, targets), saveRecipe(step), onClose() }
 */
export function openTool(id, ctx) {
  closeTool();
  const done = () => {
    closeTool();
    ctx.onClose();
  };
  const build = { trim: trimView, crop: cropView, pages: pagesView, merge: mergeView }[id];
  view().hidden = false;
  current = build(ctx, done);
}

// ---------------------------------------------------------------- shared bits

function shell(title, body, buttons) {
  view().replaceChildren(el("div", { className: "editor-title", textContent: title }), ...body, el("div", { className: "editor-buttons" }, buttons));
}

const button = (label, onclick, className = "btn") => el("button", { type: "button", className, textContent: label, onclick });

/** A segmented choice; `onPick(value)` runs on change. */
function choice(options, value, onPick) {
  const name = `seg-${Math.random().toString(36).slice(2)}`;
  return el(
    "div",
    { className: "seg" },
    options.map(([v, label]) => {
      const input = el("input", { type: "radio", name, value: v, checked: v === value, onchange: () => onPick(v) });
      return el("label", {}, [input, label]);
    }),
  );
}

const note = () => el("p", { className: "tool-note" });

// ---------------------------------------------------------------- time

/** 65.25 -> "1:05.25" (hours when needed). */
export function formatTime(v, digits = 2) {
  const sign = v < 0 ? "-" : "";
  v = Math.abs(v);
  const h = Math.floor(v / 3600);
  const m = Math.floor((v % 3600) / 60);
  const s = (v % 60).toFixed(digits).padStart(digits ? digits + 3 : 2, "0");
  return h ? `${sign}${h}:${String(m).padStart(2, "0")}:${s}` : `${sign}${m}:${s}`;
}

/** "1:05.25", "65.25" or "1:02:05" -> seconds, or NaN. */
export function parseTime(text) {
  const parts = text.trim().split(":");
  if (!parts.length || parts.length > 3 || parts.some((p) => !/^\d+(\.\d+)?$/.test(p.trim()))) return NaN;
  return parts.reduce((acc, p) => acc * 60 + Number(p), 0);
}

/** Seconds as they go into an action id: at most 3 decimals, no trailing zeros. */
const idSecs = (v) => String(+v.toFixed(3));

// ---------------------------------------------------------------- trim

function trimView(ctx, done) {
  const path = ctx.paths[0];
  const isVideo = VIDEO.includes(ext(path));
  const media = el(isVideo ? "video" : "audio", { preload: "auto", className: "media" });
  media.hidden = !isVideo;
  const status = note();
  let duration = 0;
  let start = 0;
  let end = 0;
  let previewJob = null;

  // Timeline: the kept part between two handles, plus the playhead.
  const keep = el("div", { className: "tl-keep" });
  const head = el("div", { className: "tl-head" });
  const startHandle = el("div", { className: "tl-handle start", title: "Start (I)" });
  const endHandle = el("div", { className: "tl-handle end", title: "End (O)" });
  const timeline = el("div", { className: "timeline disabled" }, [el("div", { className: "tl-track" }), keep, head, startHandle, endHandle]);

  const playBtn = button("Play", () => (media.paused ? play() : media.pause()), "btn small");
  const clock = el("span", { className: "tl-clock", textContent: "0:00.00 / 0:00.00" });
  const setStart = button("Set start", () => setRange(media.currentTime, end), "btn small");
  const setEnd = button("Set end", () => setRange(start, media.currentTime), "btn small");
  const transport = el("div", { className: "transport" }, [playBtn, clock, el("span", { className: "spacer" }), setStart, setEnd]);

  const startBox = el("input", { className: "textbox", type: "text", autocomplete: "off", spellcheck: false });
  const endBox = el("input", { className: "textbox", type: "text", autocomplete: "off", spellcheck: false });
  const length = el("span", { className: "tool-readout" });
  const times = el("div", { className: "tool-grid two" }, [
    el("label", { className: "field" }, [el("span", { textContent: "Start" }), startBox]),
    el("label", { className: "field" }, [el("span", { textContent: "End" }), endBox]),
  ]);

  const precise = el("input", { type: "checkbox", className: "switch" });
  const preciseDesc = el("span", { className: "item-desc" });
  const preciseRow = el("label", { className: "toggle-row" }, [
    el("span", { className: "item-text" }, [el("span", { textContent: "Precise cut" }), preciseDesc]),
    precise,
  ]);
  const describePrecise = () =>
    (preciseDesc.textContent = precise.checked
      ? "Re-encodes so the cut lands on the exact frame. Slower."
      : "Instant and lossless. Video starts at the keyframe just before Start.");
  precise.addEventListener("change", describePrecise);
  describePrecise();
  preciseRow.hidden = !isVideo;

  const runBtn = button("Trim", run, "btn accent");
  const saveBtn = button("Save as recipe", () => ctx.saveRecipe(step()));
  shell("Trim", [media, timeline, transport, times, length, preciseRow, status], [runBtn, saveBtn, button("Done", done)]);

  const pct = (t) => `${(duration ? (t / duration) * 100 : 0).toFixed(3)}%`;
  function paint() {
    keep.style.left = startHandle.style.left = pct(start);
    keep.style.width = `calc(${pct(end)} - ${pct(start)})`;
    endHandle.style.left = pct(end);
    head.style.left = pct(media.currentTime || 0);
    clock.textContent = `${formatTime(media.currentTime || 0)} / ${formatTime(duration)}`;
    if (document.activeElement !== startBox) startBox.value = formatTime(start);
    if (document.activeElement !== endBox) endBox.value = formatTime(end);
    const whole = start <= 0 && end >= duration;
    length.textContent = whole ? "Move a handle to choose what to keep" : `Keeps ${formatTime(end - start, 1)} of ${formatTime(duration, 1)}`;
    runBtn.disabled = saveBtn.disabled = !duration || whole;
  }

  function setRange(s, e) {
    if (!duration) return;
    s = Math.max(0, Math.min(s, duration));
    e = Math.max(0, Math.min(e, duration));
    if (e - s < 0.05) return paint(); // handles can't cross
    start = s;
    end = e;
    paint();
  }

  /** The action for the current cut; an end at the very end is left open. */
  function step() {
    const atEnd = end >= duration - 0.001;
    const id = `trim:${start > 0 ? idSecs(start) : 0}-${atEnd ? "" : idSecs(end)}${precise.checked && isVideo ? ":precise" : ""}`;
    const title = `Trim ${formatTime(start, 1)}–${atEnd ? "end" : formatTime(end, 1)}${precise.checked && isVideo ? " (precise)" : ""}`;
    return { id, title };
  }

  function run() {
    const s = step();
    ctx.runJob(s.title, (p, jobId) => invoke("run_action", { actionId: s.id, path: p, jobId }), [path]);
  }

  function play() {
    if (media.currentTime < start || media.currentTime >= end - 0.05) media.currentTime = start;
    media.play();
  }

  // Playback stays within the cut; the playhead follows smoothly.
  let raf = 0;
  const follow = () => {
    if (!media.paused && media.currentTime >= end) {
      media.pause();
      media.currentTime = end;
    }
    paint();
    if (!media.paused) raf = requestAnimationFrame(follow);
  };
  media.addEventListener("play", () => {
    playBtn.textContent = "Pause";
    raf = requestAnimationFrame(follow);
  });
  media.addEventListener("pause", () => {
    playBtn.textContent = "Play";
    cancelAnimationFrame(raf);
    paint();
  });
  media.addEventListener("seeked", paint);

  // Drag a handle (seeks so you see the frame), or click/drag the track to scrub.
  const timeAt = (clientX) => {
    const r = timeline.getBoundingClientRect();
    return Math.max(0, Math.min(1, (clientX - r.left) / r.width)) * duration;
  };
  timeline.addEventListener("pointerdown", (down) => {
    if (!duration || down.button !== 0) return;
    const which = down.target === startHandle ? "start" : down.target === endHandle ? "end" : "scrub";
    timeline.setPointerCapture(down.pointerId);
    const move = (e) => {
      const t = timeAt(e.clientX);
      if (which === "start") setRange(t, end);
      else if (which === "end") setRange(start, t);
      media.currentTime = which === "start" ? start : which === "end" ? end : t;
      paint();
    };
    move(down);
    const up = () => {
      timeline.removeEventListener("pointermove", move);
      timeline.removeEventListener("pointerup", up);
    };
    timeline.addEventListener("pointermove", move);
    timeline.addEventListener("pointerup", up);
  });

  for (const [box, which] of [[startBox, "start"], [endBox, "end"]]) {
    box.addEventListener("pointerdown", () => invoke("focus_drawer"));
    box.addEventListener("change", () => {
      const t = parseTime(box.value);
      if (!Number.isNaN(t)) which === "start" ? setRange(t, end) : setRange(start, t);
      box.value = formatTime(which === "start" ? start : end);
      media.currentTime = which === "start" ? start : end;
    });
  }

  // Load: the file itself if the webview can play it, else a converted preview.
  let triedPreview = false;
  const ready = () => {
    if (!Number.isFinite(media.duration) || media.duration <= 0) return;
    if (isVideo && !media.videoWidth) return fallback(); // e.g. HEVC: sound but no picture
    duration = media.duration;
    start = 0;
    end = duration;
    status.textContent = triedPreview ? "Previewing a converted copy; the trim uses the original file." : "";
    timeline.classList.remove("disabled");
    paint();
  };
  async function fallback() {
    if (triedPreview) {
      status.textContent = "Couldn't play this file.";
      return;
    }
    triedPreview = true;
    status.textContent = "Preparing a preview…";
    previewJob = crypto.randomUUID();
    try {
      const preview = await invoke("media_preview", { path, jobId: previewJob });
      previewJob = null;
      if (!current || current.path !== path) return;
      media.src = `${convertFileSrc(preview)}?v=${Date.now()}`;
    } catch (err) {
      previewJob = null;
      if (String(err) !== "Cancelled") status.textContent = `Couldn't make a preview: ${err}`;
    }
  }
  media.addEventListener("loadedmetadata", ready);
  media.addEventListener("error", fallback);
  invoke("media_source", { path })
    .then((src) => (media.src = convertFileSrc(src)))
    .catch((err) => (status.textContent = String(err)));
  paint();

  return {
    path,
    close() {
      media.pause();
      media.removeAttribute("src");
      media.load();
      if (previewJob) invoke("cancel_job", { jobId: previewJob });
    },
    keydown(e) {
      if (e.target.matches("input[type=text]")) return false;
      const nudge = e.shiftKey ? 1 : 0.1;
      if (e.key === " ") media.paused ? play() : media.pause();
      else if (e.key === "i" || e.key === "I") setRange(media.currentTime, end);
      else if (e.key === "o" || e.key === "O") setRange(start, media.currentTime);
      else if (e.key === "ArrowLeft") media.currentTime = Math.max(0, media.currentTime - nudge);
      else if (e.key === "ArrowRight") media.currentTime = Math.min(duration, media.currentTime + nudge);
      else return false;
      e.preventDefault();
      return true;
    },
  };
}

// ---------------------------------------------------------------- crop

const ASPECTS = [
  ["free", "Free"],
  ["1x1", "1:1"],
  ["4x3", "4:3"],
  ["16x9", "16:9"],
  ["9x16", "9:16"],
];

function cropView(ctx, done) {
  const path = ctx.paths[0];
  const status = note();
  let W = 0;
  let H = 0;
  let aspect = "free";
  let rect = { x: 0, y: 0, w: 0, h: 0 };

  const img = el("img", { className: "crop-img", alt: "", draggable: false });
  const frame = el("div", { className: "crop-rect" }, ["nw", "ne", "sw", "se"].map((c) => el("div", { className: `crop-handle ${c}` })));
  const stage = el("div", { className: "crop-stage" }, [img, frame]);
  stage.hidden = true;

  const boxes = Object.fromEntries(
    ["x", "y", "w", "h"].map((k) => [k, el("input", { className: "textbox", type: "number", min: 0, step: 1, inputMode: "numeric" })]),
  );
  const readouts = el(
    "div",
    { className: "tool-grid four" },
    [["x", "X"], ["y", "Y"], ["w", "Width"], ["h", "Height"]].map(([k, label]) => el("label", { className: "field" }, [el("span", { textContent: label }), boxes[k]])),
  );

  const ratio = () => (aspect === "free" ? null : aspect.split("x").map(Number).reduce((a, b) => a / b));
  const aspects = choice(ASPECTS, aspect, (v) => {
    aspect = v;
    fitAspect();
  });

  const runBtn = button("Crop", run, "btn accent");
  const saveBtn = button("Save as recipe", () => ctx.saveRecipe(step()));
  shell("Crop", [aspects, stage, readouts, status], [runBtn, saveBtn, button("Done", done)]);

  function paint() {
    const p = (v, of) => `${((v / of) * 100).toFixed(4)}%`;
    Object.assign(frame.style, { left: p(rect.x, W), top: p(rect.y, H), width: p(rect.w, W), height: p(rect.h, H) });
    for (const k of Object.keys(boxes)) if (document.activeElement !== boxes[k]) boxes[k].value = rect[k];
    const whole = rect.x === 0 && rect.y === 0 && rect.w === W && rect.h === H;
    runBtn.disabled = !W || whole;
    saveBtn.disabled = !W || (whole && aspect === "free");
    saveBtn.textContent = aspect === "free" ? "Save as recipe" : `Save ${aspect.replace("x", ":")} as recipe`;
    saveBtn.title = aspect === "free" ? "Saves this exact area in pixels" : "Saves a centred crop that fits any picture";
  }

  /** Keeps `r` inside the picture with whole, non-zero pixels. */
  function clamp(r) {
    const w = Math.max(1, Math.min(Math.round(r.w), W));
    const h = Math.max(1, Math.min(Math.round(r.h), H));
    return { w, h, x: Math.max(0, Math.min(Math.round(r.x), W - w)), y: Math.max(0, Math.min(Math.round(r.y), H - h)) };
  }

  /** Largest centred area with the chosen ratio (all of it for Free). */
  function fitAspect() {
    const r = ratio();
    if (!r) rect = { x: 0, y: 0, w: W, h: H };
    else {
      const w = W / H > r ? H * r : W;
      const h = w / r;
      rect = clamp({ x: (W - w) / 2, y: (H - h) / 2, w, h });
    }
    paint();
  }

  // Dragging: inside the frame moves it, a corner resizes (keeping the ratio).
  stage.addEventListener("pointerdown", (down) => {
    if (!W || down.button !== 0) return;
    down.preventDefault();
    const box = stage.getBoundingClientRect();
    const scale = W / box.width;
    const at = (e) => ({ x: (e.clientX - box.left) * scale, y: (e.clientY - box.top) * scale });
    const corner = down.target.classList.contains("crop-handle") ? [...down.target.classList].find((c) => c.length === 2) : null;
    const start = at(down);
    const from = { ...rect };
    const inside = start.x >= rect.x && start.x <= rect.x + rect.w && start.y >= rect.y && start.y <= rect.y + rect.h;
    // Outside the frame: draw a new one from where you pressed.
    const mode = corner ?? (inside ? "move" : "draw");
    stage.setPointerCapture(down.pointerId);
    const move = (e) => {
      const p = at(e);
      const r = ratio();
      if (mode === "move") {
        rect = clamp({ ...from, x: from.x + p.x - start.x, y: from.y + p.y - start.y });
      } else {
        // The fixed corner stays put; the dragged one follows the pointer.
        const fixed = mode === "draw" ? start : { x: mode.includes("w") ? from.x + from.w : from.x, y: mode.includes("n") ? from.y + from.h : from.y };
        const px = Math.max(0, Math.min(p.x, W));
        const py = Math.max(0, Math.min(p.y, H));
        let w = Math.abs(px - fixed.x);
        let h = Math.abs(py - fixed.y);
        if (r) {
          // Follow the larger movement, then fit inside the picture.
          if (w / h > r) h = w / r;
          else w = h * r;
          const maxW = px < fixed.x ? fixed.x : W - fixed.x;
          const maxH = py < fixed.y ? fixed.y : H - fixed.y;
          const k = Math.min(1, maxW / w, maxH / h);
          w *= k;
          h *= k;
        }
        const x = px < fixed.x ? fixed.x - w : fixed.x;
        const y = py < fixed.y ? fixed.y - h : fixed.y;
        if (w >= 1 && h >= 1) rect = clamp({ x, y, w, h });
      }
      paint();
    };
    const up = () => {
      stage.removeEventListener("pointermove", move);
      stage.removeEventListener("pointerup", up);
    };
    stage.addEventListener("pointermove", move);
    stage.addEventListener("pointerup", up);
  });

  for (const [k, box] of Object.entries(boxes)) {
    box.addEventListener("pointerdown", () => invoke("focus_drawer"));
    box.addEventListener("change", () => {
      const v = Number(box.value);
      if (!Number.isFinite(v)) return paint();
      const next = { ...rect, [k]: v };
      const r = ratio();
      if (r && k === "w") next.h = v / r;
      if (r && k === "h") next.w = v * r;
      if (r && (next.w > W || next.h > H)) {
        const f = Math.min(W / next.w, H / next.h);
        next.w *= f;
        next.h *= f;
      }
      rect = clamp(next);
      paint();
    });
  }

  function step() {
    if (aspect !== "free") return { id: `crop:${aspect}`, title: `Crop ${aspect.replace("x", ":")}, centred` };
    const { x, y, w, h } = rect;
    return { id: `crop:${x},${y},${w},${h}`, title: `Crop ${w}×${h} at ${x}, ${y}` };
  }

  function run() {
    const { x, y, w, h } = rect;
    const id = `crop:${x},${y},${w},${h}`; // always the exact area shown
    ctx.runJob(`Crop ${w}×${h}`, (p, jobId) => invoke("run_action", { actionId: id, path: p, jobId }), [path]);
  }

  status.textContent = "Loading…";
  invoke("crop_source", { path })
    .then((src) => {
      W = src.width;
      H = src.height;
      img.onload = () => {
        // Fit the picture to the drawer's width and a fixed height.
        const k = Math.min(view().clientWidth / img.naturalWidth, 300 / img.naturalHeight);
        stage.style.width = `${Math.round(img.naturalWidth * k)}px`;
        stage.style.height = `${Math.round(img.naturalHeight * k)}px`;
        stage.hidden = false;
        status.textContent = "Drag inside the frame to move it, or a corner to resize.";
        fitAspect();
      };
      img.onerror = () => (status.textContent = "Couldn't show this picture.");
      img.src = convertFileSrc(src.path);
    })
    .catch((err) => (status.textContent = String(err)));
  paint();

  return {
    path,
    close() {},
    keydown(e) {
      if (e.target.matches("input")) return false;
      const n = e.shiftKey ? 10 : 1;
      const d = { ArrowLeft: [-n, 0], ArrowRight: [n, 0], ArrowUp: [0, -n], ArrowDown: [0, n] }[e.key];
      if (!d || !W) return false;
      rect = clamp({ ...rect, x: rect.x + d[0], y: rect.y + d[1] });
      paint();
      e.preventDefault();
      return true;
    },
  };
}

// ---------------------------------------------------------------- split pages

/** "1-3,5,8-" -> "1–3, 5, 8–end". */
const rangesLabel = (spec) =>
  spec
    .split(",")
    .map((r) => (r.endsWith("-") ? `${r.slice(0, -1)}–end` : r.replace("-", "–")))
    .join(", ");

function pagesView(ctx, done) {
  const path = ctx.paths[0];
  let mode = "split";
  let checked = null; // { count, spec, ranges } for the current text
  const input = el("input", { className: "textbox", type: "text", placeholder: "1-3, 5, 8-", autocomplete: "off", spellcheck: false });
  input.addEventListener("pointerdown", () => invoke("focus_drawer"));
  const readout = el("p", { className: "tool-note" });
  const modes = choice(
    [
      ["split", "One PDF per range"],
      ["extract", "One PDF of these pages"],
    ],
    mode,
    (v) => {
      mode = v;
      paint();
    },
  );
  const runBtn = button("Split", run, "btn accent");
  const saveBtn = button("Save as recipe", () => ctx.saveRecipe(step()));
  shell(
    "Split pages",
    [el("label", { className: "field" }, [el("span", { textContent: "Pages" }), input]), readout, modes],
    [runBtn, saveBtn, button("Done", done)],
  );

  let count = null;
  function paint(error = "") {
    runBtn.textContent = mode === "split" ? "Split" : "Extract";
    const ok = !!checked?.spec && !error;
    runBtn.disabled = saveBtn.disabled = !ok;
    readout.classList.toggle("error", !!error);
    const pages = count == null ? "" : `${count} page${count === 1 ? "" : "s"}`;
    if (error) readout.textContent = error;
    else if (!ok) readout.textContent = pages ? `${pages}. Ranges like 2-4 or 8- (to the end), separated by commas.` : "";
    else {
      const n = checked.ranges.length;
      const kept = checked.ranges.reduce((sum, [a, b]) => sum + b - a + 1, 0);
      readout.textContent =
        mode === "split"
          ? `${n} PDF${n === 1 ? "" : "s"}: ${checked.ranges.map(([a, b]) => (a === b ? a : `${a}–${b}`)).join(", ")} (of ${pages})`
          : `1 PDF with ${kept} page${kept === 1 ? "" : "s"} (of ${pages})`;
    }
  }

  let seq = 0;
  async function check() {
    const mine = ++seq;
    try {
      const r = await invoke("pdf_ranges", { path, ranges: input.value });
      if (mine !== seq) return;
      count = r.count;
      checked = r;
      paint();
    } catch (err) {
      if (mine !== seq) return;
      checked = null;
      paint(String(err));
    }
  }
  input.addEventListener("input", check);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !runBtn.disabled) {
      e.preventDefault();
      e.stopPropagation();
      run();
    }
  });

  function step() {
    const label = rangesLabel(checked.spec);
    return mode === "split"
      ? { id: `split:${checked.spec}`, title: `Split by pages ${label}` }
      : { id: `extract:${checked.spec}`, title: `Extract pages ${label}` };
  }

  function run() {
    const s = step();
    ctx.runJob(s.title, (p, jobId) => invoke("run_action", { actionId: s.id, path: p, jobId }), [path]);
  }

  check();
  invoke("focus_drawer").then(() => input.focus());
  return { path, close() {}, keydown: () => false };
}

// ---------------------------------------------------------------- merge

function mergeView(ctx, done) {
  let order = [...ctx.paths];
  const list = el("ol", { className: "tool-list" });
  const plan = note();
  const runBtn = button("Merge", run, "btn accent");
  shell(
    "Merge videos",
    [el("p", { className: "tool-note", textContent: "Joined top to bottom, in the order you dropped them." }), list, plan],
    [runBtn, button("Done", done)],
  );

  const moveBtn = (i, by, label) => {
    const b = el("button", { type: "button", className: `icon-btn ${by < 0 ? "up" : ""}`, title: label, ariaLabel: label, disabled: i + by < 0 || i + by >= order.length }, [icon("chevronDown")]);
    b.onclick = () => {
      [order[i], order[i + by]] = [order[i + by], order[i]];
      paintList();
      checkPlan();
    };
    return b;
  };
  function paintList() {
    list.replaceChildren(
      ...order.map((p, i) => el("li", { title: p }, [el("span", { textContent: basename(p) }), moveBtn(i, -1, "Move up"), moveBtn(i, 1, "Move down")])),
    );
  }

  let seq = 0;
  async function checkPlan() {
    const mine = ++seq;
    plan.textContent = "Checking the clips…";
    plan.classList.remove("error");
    runBtn.disabled = true;
    try {
      const p = await invoke("merge_plan", { paths: order });
      if (mine !== seq) return;
      plan.textContent = p.copy
        ? `Same format and size: joins without re-encoding, so it's quick and lossless (.${p.ext}).`
        : `Re-encodes to a ${p.width}×${p.height} MP4 (the first clip's size) because ${p.reason}. This takes a while.`;
      runBtn.disabled = false;
    } catch (err) {
      if (mine !== seq) return;
      plan.textContent = String(err);
      plan.classList.add("error");
    }
  }

  function run() {
    const clips = [...order];
    ctx.runJob(`Merging ${clips.length} videos`, (_, jobId) => invoke("run_combine", { actionId: "combine:video", paths: clips, jobId }), [clips[0]]);
  }

  paintList();
  checkPlan();
  return { path: order[0], close() {}, keydown: () => false };
}
