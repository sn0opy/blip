import { $ } from "./dom.js";
import { current, state, type Entry } from "./state.js";

const FLASH_PERIOD = 800;

export const canvas = $<HTMLCanvasElement>("canvas");
const ctx = canvas.getContext("2d")!;

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function initView() {
  new ResizeObserver(() => {
    canvas.width = Math.round(canvas.clientWidth * devicePixelRatio);
    canvas.height = Math.round(canvas.clientHeight * devicePixelRatio);
    draw();
  }).observe(canvas);
}

let drawQueued = false;

export function scheduleDraw() {
  if (drawQueued) return;
  drawQueued = true;
  requestAnimationFrame(() => {
    drawQueued = false;
    draw();
  });
}

let flashTimer = 0;

function draw() {
  const dpr = canvas.width / Math.max(1, canvas.clientWidth);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, canvas.clientWidth, canvas.clientHeight);
  clearTimeout(flashTimer);
  const { shown } = state;
  if (!shown) return;

  const r = imageRect(shown);
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(shown.image, r.x, r.y, r.w, r.h);

  if (current()?.name !== shown.name) return;
  if (state.flashClip === shown.name && shown.overlay) drawClipFlash(shown, r, dpr);
  const stars = state.stars.get(shown.name);
  if (state.starView === shown.name && stars?.key === shown.key) drawStars(shown, r, stars.list);
}

function imageRect(shown: Entry): Rect {
  const [w, h] = [canvas.clientWidth, canvas.clientHeight];
  const fit = Math.min(w / shown.width, h / shown.height) * state.zoom;
  const [iw, ih] = [shown.width * fit, shown.height * fit];
  return { x: w / 2 + state.pan.x - iw / 2, y: h / 2 + state.pan.y - ih / 2, w: iw, h: ih };
}

function drawClipFlash(shown: Entry, r: Rect, dpr: number) {
  const o = shown.overlay!;
  const phase = (performance.now() / FLASH_PERIOD) % 1;
  if (phase < 0.5) {
    const cell = (o.cell * r.w) / shown.width;
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(dpr * cell < 1 ? o.grown : o.plain, r.x, r.y, o.plain.width * cell, o.plain.height * cell);
  }
  const nextEdge = (phase < 0.5 ? 0.5 : 1) - phase;
  flashTimer = setTimeout(scheduleDraw, nextEdge * FLASH_PERIOD + 1);
}

function drawStars(shown: Entry, r: Rect, list: Float32Array) {
  const scale = r.w / shown.srcWidth;
  const [w, h] = [canvas.clientWidth, canvas.clientHeight];
  ctx.strokeStyle = ctx.fillStyle = "rgb(70 230 70)";
  ctx.lineWidth = 1;
  ctx.font = "11px system-ui, sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "bottom";
  for (let i = 0; i < list.length; i += 3) {
    const x = r.x + (list[i] + 0.5) * scale;
    const y = r.y + (list[i + 1] + 0.5) * scale;
    if (x < -20 || y < -20 || x > w + 20 || y > h + 20) continue;
    const hfr = list[i + 2];
    const radius = Math.max(hfr * 2 * scale, 5);
    ctx.beginPath();
    ctx.arc(x, y, radius, 0, 2 * Math.PI);
    ctx.stroke();
    ctx.fillText(hfr.toFixed(2), x, y - radius - 1);
  }
}

export function resetZoom() {
  state.zoom = 1;
  state.pan = { x: 0, y: 0 };
  scheduleDraw();
}

export function zoomBy(factor: number, at: { x: number; y: number }, shift = { x: 0, y: 0 }) {
  const { pan, zoom } = state;
  const next = Math.min(32, Math.max(1, zoom * factor));
  const box = canvas.getBoundingClientRect();
  pan.x += shift.x;
  pan.y += shift.y;
  const px = at.x - box.left - box.width / 2 - pan.x;
  const py = at.y - box.top - box.height / 2 - pan.y;
  pan.x += px * (1 - next / zoom);
  pan.y += py * (1 - next / zoom);
  state.zoom = next;
  if (next <= 1) state.pan = { x: 0, y: 0 };
  scheduleDraw();
}
