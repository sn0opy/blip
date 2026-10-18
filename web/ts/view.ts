import { $ } from "./dom.js";
import type { Tilt } from "./messages.js";
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
  if (state.tiltView === shown.name && stars?.key === shown.key && stars.tilt) drawTilt(shown, r, stars.tilt);
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

/** The tilt polygon with its diagonals, labeled with the corner and center HFRs. */
function drawTilt(shown: Entry, r: Rect, tilt: Tilt) {
  const scale = r.w / shown.srcWidth;
  const color = "rgb(70 230 70)";
  const pts = tilt.corners.map((c) => ({ x: r.x + c.x * scale, y: r.y + c.y * scale, hfr: c.hfr }));
  ctx.strokeStyle = color;
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (const p of pts) ctx.lineTo(p.x, p.y);
  ctx.closePath();
  ctx.moveTo(pts[0].x, pts[0].y);
  ctx.lineTo(pts[2].x, pts[2].y);
  ctx.moveTo(pts[1].x, pts[1].y);
  ctx.lineTo(pts[3].x, pts[3].y);
  ctx.stroke();

  ctx.font = "13px system-ui, sans-serif";
  const label = (x: number, y: number, hfr: number) => {
    ctx.fillStyle = "rgb(0 0 0 / 0.8)";
    ctx.fillText(hfr.toFixed(2), x + 1, y + 1);
    ctx.fillStyle = color;
    ctx.fillText(hfr.toFixed(2), x, y);
  };
  const [cx, cy] = [r.x + r.w / 2, r.y + r.h / 2];
  // Each label goes on the outer side of its corner.
  for (const p of pts) {
    const [sx, sy] = [Math.sign(p.x - cx), Math.sign(p.y - cy)];
    ctx.textAlign = sx < 0 ? "right" : "left";
    ctx.textBaseline = sy < 0 ? "bottom" : "top";
    label(p.x + 4 * sx, p.y + 4 * sy, p.hfr);
  }
  ctx.textAlign = "left";
  ctx.textBaseline = "middle";
  label(cx + 8, cy, tilt.center);
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
