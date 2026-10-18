// Ferryfile's icon set: 24px stroke icons in the spirit of Fluent System
// Icons, drawn with currentColor. `icon(name)` returns an <svg> element;
// elements with data-icon="name" get theirs inserted by `hydrateIcons()`.

const P = {
  app: '<path d="M3 15h18l-2.5 4.5h-13z"/><path d="M6 15V9h9l3 6"/>',
  close: '<path d="M6 6l12 12M18 6L6 18"/>',
  // Gear: eight teeth around a hub.
  settings:
    '<circle cx="12" cy="12" r="3"/><path d="M10.3 3.2h3.4l.5 2.4 1.6.9 2.3-.8 1.7 2.9-1.8 1.6v1.6l1.8 1.6-1.7 2.9-2.3-.8-1.6.9-.5 2.4h-3.4l-.5-2.4-1.6-.9-2.3.8-1.7-2.9 1.8-1.6v-1.6L4.3 8.6 6 5.7l2.3.8 1.6-.9z"/>',
  shelves: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 10h18"/>',
  pin: '<path d="M9 3.5h6l-1 6.5 3.5 3.5h-11L10 10z"/><path d="M12 13.5V21"/>',
  image: '<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="2"/><path d="M21 17l-5-5-9 8"/>',
  audio: '<path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/>',
  video: '<rect x="3" y="6" width="13" height="12" rx="2"/><path d="M16 10l5-3v10l-5-3"/>',
  document: '<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4M9 12h6M9 16h6"/>',
  pdf: '<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/><path d="M9 17v-4h1.5a1.5 1.5 0 0 1 0 3H9"/>',
  sheet: '<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M4 10h16M4 15h16M10 4v16"/>',
  archive: '<rect x="3" y="4" width="18" height="5" rx="1"/><path d="M5 9v10h14V9M10 13h4"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  file: '<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/>',
  files: '<path d="M8 3h7l4 4v11H8z"/><path d="M15 3v4h4"/><path d="M5 7v14h10"/>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
  explorer: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v1H3z"/><path d="M3 10h18l-2 9H5z"/>',
  terminal: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M12 15h5"/>',
  move: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M9 13h6M13 10.5l2.5 2.5-2.5 2.5"/>',
  copyTo: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 10v6M9 13h6"/>',
  zip: '<path d="M6 3h8l4 4v14H6z"/><path d="M11 3v2M11 7v2M11 11v2"/><rect x="9.5" y="14" width="3" height="3" rx="0.5"/>',
  shelfAdd: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M12 9v6M9 12h6"/>',
  chevronRight: '<path d="M9 6l6 6-6 6"/>',
  chevronDown: '<path d="M6 9l6 6 6-6"/>',
  check: '<circle cx="12" cy="12" r="9"/><path d="M8 12.5l2.5 2.5 5.5-5.5"/>',
  error: '<circle cx="12" cy="12" r="9"/><path d="M12 7.5v5.5M12 16.5v.5"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5.5M12 7.5v.5"/>',
  edit: '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>',
  add: '<path d="M12 5v14M5 12h14"/>',
  drop: '<path d="M12 4v11M7.5 10.5L12 15l4.5-4.5"/><path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>',
  dragOut: '<path d="M12 20V9M7.5 13.5L12 9l4.5 4.5"/><path d="M4 9V6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v3"/>',
  search: '<circle cx="10.5" cy="10.5" r="6"/><path d="M15 15l5 5"/>',
};

export function icon(name, cls = "icon") {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.6");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", cls);
  svg.innerHTML = P[name] ?? P.file;
  return svg;
}

export function hydrateIcons(root = document) {
  for (const el of root.querySelectorAll("[data-icon]")) {
    if (el.querySelector(":scope > svg")) continue;
    el.prepend(icon(el.dataset.icon, el.dataset.iconClass || "icon"));
  }
}

const KINDS = [
  ["image", ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "heic", "avif", "svg"]],
  ["audio", ["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "aiff", "aif", "wma"]],
  ["video", ["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv"]],
  ["pdf", ["pdf"]],
  ["sheet", ["xlsx", "xlsm", "xlsb", "xls", "ods", "csv", "tsv"]],
  ["archive", ["zip", "7z", "rar", "tar", "gz"]],
  ["document", ["doc", "docx", "odt", "rtf", "md", "markdown", "txt", "html", "htm", "epub", "pptx", "ppt", "odp", "tex", "typ", "ipynb", "rst", "org"]],
];

/** Icon name for a path, by extension ("folder" when it has none). */
export function iconForPath(path) {
  const ext = path.includes(".") ? path.split(".").pop().toLowerCase() : "";
  if (!ext) return "folder";
  return KINDS.find(([, exts]) => exts.includes(ext))?.[0] ?? "file";
}
