import { $, setStatus } from "./dom.js";
import { renderList, updateList } from "./list.js";
import { clearCache, evict, show } from "./cache.js";
import { pump, restartLoads } from "./loader.js";
import { current, isImage, state, valid, type FileEntry, type Source } from "./state.js";
import { debugGeneral, debugText, infoText } from "./text.js";
import { scheduleDraw } from "./view.js";

export function changed() {
  updateShown();
  evict();
  pump();
  updateList();
  updateText();
  scheduleDraw();
}

export function open(source: Source, entries: FileEntry[]) {
  state.source = source;
  setFiles(entries);
  state.selected = 0;
  state.direction = 1;
  state.errors.clear();
  state.stars.clear();
  state.starsFailed.clear();
  // Names may repeat between folders: drop everything from the previous one.
  restartLoads();
  clearCache();
  $("#source").textContent = source.label;
  if (!state.files.length) setStatus("No FITS / XISF files found", 3000);
  renderList(select);
  changed();
}

export function replaceFiles(entries: FileEntry[]) {
  const name = current()?.name;
  setFiles(entries);
  state.selected = Math.max(
    0,
    state.files.findIndex((f) => f.name === name),
  );
  state.errors.clear();
  renderList(select);
  changed();
}

function setFiles(entries: FileEntry[]) {
  const byName = (a: FileEntry, b: FileEntry) => a.name.toLowerCase().localeCompare(b.name.toLowerCase());
  state.files = entries.filter((f) => isImage(f.name)).sort(byName);
  state.listed = new Set(state.files.map((f) => f.name));
}

export function select(index: number) {
  if (!state.files.length) return;
  const i = Math.max(0, Math.min(state.files.length - 1, index));
  if (i !== state.selected) state.direction = i > state.selected ? 1 : -1;
  state.selected = i;
  changed();
}

export const step = (delta: number) => select(state.selected + delta);

function updateShown() {
  const name = current()?.name;
  if (state.selectedAt?.name !== name) {
    state.selectedAt = name ? { name, time: performance.now() } : null;
    state.shownAfter = null;
  }
  const entry = valid(name);
  if (!entry || !state.selectedAt) return;
  show(entry);
  state.shownAfter ??= performance.now() - state.selectedAt.time;
}

export function toggleStretch() {
  const cur = current();
  if (!cur) return;
  state.unstretched = state.unstretched === cur.name ? null : cur.name;
  setStatus(`Auto stretch ${state.unstretched ? "off" : "on"} (A to toggle)`, 2000);
  changed();
}

export function toggleFullRes() {
  state.fullRes = !state.fullRes;
  setStatus(`${state.fullRes ? "Full resolution" : "Binned preview"} (B to toggle)`, 2000);
  changed();
}

export function toggleClipFlash() {
  const cur = current();
  if (!cur) return;
  if (state.flashClip === cur.name) {
    state.flashClip = null;
  } else {
    if (state.shown?.name === cur.name && !state.shown.overlay) setStatus("No clipped pixels in this image", 2000);
    state.flashClip = cur.name;
  }
  scheduleDraw();
}

export function toggleStars() {
  const name = current()?.name ?? null;
  state.starView = state.starView === name ? null : name;
  scheduleDraw();
}

export function updateText() {
  const cur = current();
  const { shown } = state;
  const error = cur && state.errors.get(cur.name);
  const debug = state.showDebug ? [shown && debugText(shown), debugGeneral()] : [];
  setBox("#info", state.showInfo && shown ? infoText(shown) : "");
  setBox("#debug", debug.filter(Boolean).join("\n"));
  setBox("#error", cur && error ? `${cur.name}: ${error}` : "");
  $("#spinner").hidden = !cur || !!error || !!valid(cur.name);
  $("#hint").hidden = state.files.length > 0;
  $<HTMLButtonElement>("#export").disabled = !state.files.length;
  document.title = cur ? `blip · ${cur.name} · ${state.selected + 1}/${state.files.length}` : "blip";
  updateToolbar();
}

function setBox(selector: string, text: string) {
  const box = $(selector);
  box.hidden = !text;
  if (text) box.textContent = text;
}

export function updateToolbar() {
  const cur = current()?.name;
  const pressed: Record<string, boolean> = {
    info: state.showInfo,
    debug: state.showDebug,
    stars: !!cur && state.starView === cur,
    clip: !!cur && state.flashClip === cur,
    stretch: state.unstretched !== cur,
    neutralize: state.neutralize,
    fullRes: state.fullRes,
  };
  for (const button of $("#toolbar").querySelectorAll<HTMLElement>("[data-action]")) {
    const value = pressed[button.dataset.action!];
    if (value !== undefined) button.setAttribute("aria-pressed", String(value));
  }
}
