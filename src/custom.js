// "Custom…" forms: type your own size, quality or target instead of a fixed
// choice. Each form builds an action id (see src-tauri/src/engine/params.rs)
// and hands it to `onSubmit`, which validates it in Rust.

function el(tag, props = {}, children = []) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

const numberBox = (props) =>
  el("input", { type: "number", className: "textbox num", inputMode: "numeric", min: 1, step: 1, autocomplete: "off", ...props });

/** Two-way toggle; `onChange(value)` fires on click. */
function choice(options, value, onChange) {
  const wrap = el("div", { className: "choice", role: "radiogroup" });
  const set = (v) => {
    for (const b of wrap.children) b.setAttribute("aria-checked", String(b.dataset.value === v));
  };
  for (const [v, label] of options) {
    const b = el("button", { type: "button", textContent: label, role: "radio" });
    b.dataset.value = v;
    b.addEventListener("click", () => {
      set(v);
      onChange(v);
    });
    wrap.append(b);
  }
  set(value);
  return wrap;
}

const int = (input) => {
  const v = input.value.trim();
  return /^\d+$/.test(v) ? Number(v) : null;
};

/** Resize: pixels (keep aspect on by default) or a percentage. */
function imageSize(info) {
  let mode = "px";
  const w = numberBox({ placeholder: "Width", ariaLabel: "Width in pixels" });
  const h = numberBox({ placeholder: "Height", ariaLabel: "Height in pixels" });
  const pct = numberBox({ placeholder: "50", ariaLabel: "Percent", value: "50" });
  const keep = el("input", { type: "checkbox", checked: true });
  const hint = el("p", { className: "hint-line" });
  const known = info?.width && info?.height;

  const pxRow = el("div", { className: "inline" }, [w, el("span", { className: "times", textContent: "×" }), h, el("span", { className: "unit", textContent: "px" })]);
  const pctRow = el("div", { className: "inline", hidden: true }, [pct, el("span", { className: "unit", textContent: "%" })]);
  const keepRow = el("label", { className: "check" }, [keep, el("span", { textContent: "Keep aspect ratio" })]);

  const update = () => {
    pxRow.hidden = keepRow.hidden = mode !== "px";
    pctRow.hidden = mode !== "pct";
    hint.textContent = "";
    if (!known) return;
    const [W, H] = [info.width, info.height];
    if (mode === "pct") {
      const p = int(pct);
      if (p) hint.textContent = `${Math.max(1, Math.round((W * p) / 100))} × ${Math.max(1, Math.round((H * p) / 100))} px`;
      return;
    }
    const [x, y] = [int(w), int(h)];
    // With keep aspect, the empty side shows what it will become.
    w.placeholder = keep.checked && y && !x ? String(Math.max(1, Math.round((W * y) / H))) : "Width";
    h.placeholder = keep.checked && x && !y ? String(Math.max(1, Math.round((H * x) / W))) : "Height";
    hint.textContent = `Now ${W} × ${H} px`;
  };
  for (const input of [w, h, pct, keep]) input.addEventListener("input", update);

  const toggle = choice([["px", "Pixels"], ["pct", "Percent"]], mode, (v) => {
    mode = v;
    update();
    (v === "px" ? w : pct).focus();
  });
  update();

  return {
    fields: [toggle, pxRow, pctRow, keepRow, hint],
    first: w,
    build() {
      if (mode === "pct") return int(pct) == null ? "Type a percentage" : `resize:${int(pct)}`;
      const [x, y] = [int(w), int(h)];
      if (keep.checked) {
        if (x != null && y != null) return `resize:w${x}h${y}`;
        if (x != null) return `resize:w${x}`;
        if (y != null) return `resize:h${y}`;
        return "Type a width or a height";
      }
      return x != null && y != null ? `resize:${x}x${y}` : "Type both width and height";
    },
  };
}

/** JPEG quality slider. */
function quality() {
  const slider = el("input", { type: "range", min: 1, max: 100, value: 80, className: "slider", ariaLabel: "JPEG quality" });
  const value = el("output", { className: "slider-value" });
  const show = () => (value.textContent = slider.value);
  slider.addEventListener("input", show);
  show();
  return {
    fields: [
      el("div", { className: "inline" }, [el("span", { className: "unit", textContent: "Quality" }), slider, value]),
      el("p", { className: "hint-line", textContent: "Lower is smaller. 60–80 looks good for most photos." }),
    ],
    first: slider,
    build: () => `compress:q${slider.value}`,
  };
}

/** "Shrink to" any size, in KB or MB (1 MB = 1000 KB, like the fixed choices). */
function targetSize(info, isVideo) {
  let unit = isVideo ? "MB" : "KB";
  const size = numberBox({ value: isVideo ? "8" : "300", min: 0, step: "any", inputMode: "decimal", ariaLabel: "Target size" });
  const toggle = choice([["KB", "KB"], ["MB", "MB"]], unit, (v) => {
    unit = v;
    size.focus();
  });
  return {
    fields: [
      el("div", { className: "inline" }, [size, toggle]),
      el("p", { className: "hint-line", textContent: isVideo ? "Lowers the bitrate (and size if needed) until it fits." : "Saves the best-looking JPG that fits." }),
    ],
    first: size,
    build() {
      const v = Number(size.value);
      if (!size.value.trim() || !Number.isFinite(v) || v <= 0) return "Type a size";
      return `fitkb:${Math.round(unit === "MB" ? v * 1000 : v)}`;
    },
  };
}

/** Video: any maximum height. */
function videoHeight(info) {
  const height = numberBox({ value: "900", ariaLabel: "Height in pixels" });
  return {
    fields: [
      el("div", { className: "inline" }, [height, el("span", { className: "unit", textContent: "px tall" })]),
      el("p", { className: "hint-line", textContent: "Width follows. Smaller videos are never scaled up." }),
    ],
    first: height,
    build: () => (int(height) == null ? "Type a height" : `scale:${int(height)}`),
  };
}

const KINDS = {
  "image-size": (info) => imageSize(info),
  quality: () => quality(),
  "target-size": (info, isVideo) => targetSize(info, isVideo),
  "video-height": (info) => videoHeight(info),
};

/**
 * The inline form for one "Custom…" entry.
 * `onSubmit(id)` resolves to an error message, or nothing when it worked.
 */
export function customForm({ form, info, isVideo = false, submitLabel, onSubmit, onCancel, onFocus }) {
  const kind = KINDS[form.kind]?.(info, isVideo);
  if (!kind) return null;
  const error = el("p", { className: "error", role: "alert" });
  const submit = el("button", { type: "submit", className: "btn accent small", textContent: submitLabel });
  const cancel = el("button", { type: "button", className: "btn small", textContent: "Cancel", onclick: onCancel });
  const node = el("form", { className: "custom-form", ariaLabel: `${form.group}: custom` }, [
    el("div", { className: "custom-title", textContent: `${form.group}: your own value` }),
    ...kind.fields,
    error,
    el("div", { className: "custom-buttons" }, [submit, cancel]),
  ]);
  node.addEventListener("submit", async (e) => {
    e.preventDefault();
    const id = kind.build();
    if (!id.includes(":")) return (error.textContent = id);
    submit.disabled = true;
    const why = await onSubmit(id);
    submit.disabled = false;
    error.textContent = why ? String(why) : "";
  });
  node.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onCancel();
    }
  });
  // Typing needs keyboard focus, which the drawer doesn't hold by default.
  node.addEventListener("pointerdown", () => onFocus?.());
  node.focusFirst = () => kind.first.focus();
  return node;
}
