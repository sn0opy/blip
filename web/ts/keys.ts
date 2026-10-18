import {
  changed,
  select,
  step,
  toggleClipFlash,
  toggleFullRes,
  toggleStars,
  toggleTilt,
  toggleStretch,
  updateText,
  updateToolbar,
} from "./app.js";
import { $, el } from "./dom.js";
import { exportCurrent } from "./export.js";
import { canRescan, openFiles, openFolder, rescan } from "./files.js";
import { toggleList } from "./list.js";
import { state } from "./state.js";
import { resetZoom } from "./view.js";

const help = $<HTMLDialogElement>("#help");

const actions = {
  next: () => step(1),
  prev: () => step(-1),
  next10: () => step(10),
  prev10: () => step(-10),
  first: () => select(0),
  last: () => select(state.files.length - 1),
  info: () => {
    state.showInfo = !state.showInfo;
    updateText();
  },
  debug: () => {
    state.showDebug = !state.showDebug;
    updateText();
  },
  neutralize: () => {
    state.neutralize = !state.neutralize;
    changed();
  },
  stretch: toggleStretch,
  clip: toggleClipFlash,
  stars: toggleStars,
  tilt: toggleTilt,
  fullRes: toggleFullRes,
  fit: resetZoom,
  list: toggleList,
  open: openFolder,
  openFiles,
  export: exportCurrent,
  rescan,
  help: () => help.showModal(),
};

export type Action = keyof typeof actions;
type Binding = [Action, ...keys: string[]];

const SHORTCUTS: [keys: string, description: string, ...Binding[]][] = [
  ["↓ → Space J", "Next image", ["next", "ArrowDown", "ArrowRight", " ", "j"]],
  ["↑ ← K", "Previous image", ["prev", "ArrowUp", "ArrowLeft", "k"]],
  ["PgDn / PgUp", "Jump 10 images", ["next10", "PageDown"], ["prev10", "PageUp"]],
  ["Home / End", "First / last image", ["first", "Home"], ["last", "End"]],
  ["Wheel / drag", "Zoom / pan (kept while blinking)"],
  ["Double-click, F, 0", "Reset zoom", ["fit", "f", "0"]],
  ["Swipe ← / →", "Next / previous image (touch, not zoomed in)"],
  ["Pinch / drag", "Zoom / pan (touch)"],
  ["Double-tap", "Reset zoom (touch)"],
  ["A", "Toggle auto stretch for this image", ["stretch", "a"]],
  ["C", "Toggle flashing clipped pixels on this image", ["clip", "c"]],
  ["S", "Toggle star sizes (HFR) on this image", ["stars", "s"]],
  ["T", "Toggle sensor tilt (corner HFR) on this image", ["tilt", "t"]],
  ["B", "Toggle binned preview / full resolution", ["fullRes", "b"]],
  ["N", "Toggle background neutralization (white balance)", ["neutralize", "n"]],
  ["I", "Toggle info overlay", ["info", "i"]],
  ["L", "Hide / show the file list", ["list", "l"]],
  ["D", "Toggle debug info (timings, stretch, memory)", ["debug", "d"]],
  ["E, Ctrl+S", "Export as PNG / JPEG (full resolution)", ["export", "e"]],
  ["O, Ctrl+O", "Open folder", ["open", "o"]],
  ["F5, Ctrl+R", "Rescan folder", ["rescan", "F5"]],
  ["?, F1", "This help", ["help", "?", "F1"]],
];

const KEYS = new Map(
  SHORTCUTS.flatMap(([, , ...bindings]) =>
    bindings.flatMap(([action, ...keys]) => keys.map((k) => [k, action] as const)),
  ),
);
const CTRL_KEYS = new Map<string, Action>([
  ["o", "open"],
  ["s", "export"],
  ["r", "rescan"],
]);

export function run(action: Action) {
  actions[action]();
  updateToolbar();
}

export function initKeys() {
  addEventListener("keydown", onKey);
  // Buttons shouldn't keep focus: Space would click them instead of blinking.
  document.addEventListener("click", (e) => {
    const button = (e.target as Element).closest<HTMLElement>("button[data-action]");
    if (!button) return;
    button.blur();
    run(button.dataset.action as Action);
  });

  $("#shortcuts").replaceChildren(
    ...SHORTCUTS.map(([keys, description]) =>
      el("tr", {}, el("td", { textContent: keys }), el("td", { textContent: description })),
    ),
  );
  // Otherwise the page's handler sees the key once the dialog has closed.
  help.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "?" || e.key === "F1") {
      e.preventDefault();
      help.close();
    }
  });
}

function onKey(e: KeyboardEvent) {
  if (document.querySelector("dialog[open]")) return;
  const ctrl = e.ctrlKey || e.metaKey;
  if (e.altKey || (ctrl && e.shiftKey)) return;
  const action = (ctrl ? CTRL_KEYS : KEYS).get(e.key);
  // Without a folder to rescan, F5 / Ctrl+R reload the page as usual.
  if (!action || (action === "rescan" && !canRescan())) return;
  e.preventDefault();
  run(action);
}
