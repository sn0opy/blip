import { $ } from "./dom.js";
import type { Tilt } from "./messages.js";
import { current, state, type Entry } from "./state.js";

const FLASH_PERIOD = 800;
/** Space between the mosaic panels, in CSS pixels. */
const MOSAIC_GAP = 3;

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

  const isCurrent = current()?.name === shown.name;
  const flash = isCurrent && state.flashClip === shown.name && !!shown.overlay && flashOn();
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  if (state.mosaic) {
    drawMosaic(shown, dpr, flash);
    return;
  }
  const r = imageRect(shown);
  ctx.drawImage(shown.image, r.x, r.y, r.w, r.h);

  if (!isCurrent) return;
  if (flash) drawClipFlash(shown, r, dpr);
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

/** Whether the clipped-pixel flash is in its visible half. Redraws at the next switch. */
function flashOn() {
  const phase = (performance.now() / FLASH_PERIOD) % 1;
  const nextEdge = (phase < 0.5 ? 0.5 : 1) - phase;
  flashTimer = setTimeout(scheduleDraw, nextEdge * FLASH_PERIOD + 1);
  return phase < 0.5;
}

function drawClipFlash(shown: Entry, r: Rect, dpr: number) {
  const o = shown.overlay!;
  const cell = (o.cell * r.w) / shown.width;
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(dpr * cell < 1 ? o.grown : o.plain, r.x, r.y, o.plain.width * cell, o.plain.height * cell);
}

function drawMosaic(shown: Entry, dpr: number, flash: boolean) {
  for (const { image, screen } of mosaicPanels(shown, dpr)) {
    ctx.drawImage(shown.image, image.x, image.y, image.w, image.h, screen.x, screen.y, screen.w, screen.h);
    if (!flash) continue;
    const o = shown.overlay!;
    const [x, y, w, h] = [image.x, image.y, image.w, image.h].map((v) => v / o.cell);
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(o.plain, x, y, w, h, screen.x, screen.y, screen.w, screen.h);
    ctx.imageSmoothingEnabled = true;
  }
}

/**
 * 3×3 square crops from the corners, edge midpoints and center, row-major, at one
 * sensor pixel per device pixel.
 */
function mosaicPanels(shown: Entry, dpr: number): { image: Rect; screen: Rect }[] {
  const snap = (cssPixels: number) => Math.round(cssPixels * dpr) / dpr;
  const gap = snap(MOSAIC_GAP);
  const side = Math.max(1 / dpr, snap((Math.min(canvas.clientWidth, canvas.clientHeight) - 2 * gap) / 3));
  const sensorCrop = Math.min(side * dpr, Math.min(shown.srcWidth, shown.srcHeight) / 3);

  // Along one axis, for panel 0 (start), 1 (middle) or 2 (end): its screen position
  // and the part of the image it shows.
  const axis = (i: number, viewSize: number, imageSize: number, sensorSize: number) => {
    const crop = (sensorCrop * imageSize) / sensorSize;
    return {
      screen: snap((viewSize - 3 * side - 2 * gap) / 2) + i * (side + gap),
      image: Math.min(imageSize - crop, Math.round((i * (imageSize - crop)) / 2)),
      crop,
    };
  };
  return Array.from({ length: 9 }, (_, i) => {
    const x = axis(i % 3, canvas.clientWidth, shown.width, shown.srcWidth);
    const y = axis(Math.floor(i / 3), canvas.clientHeight, shown.height, shown.srcHeight);
    return {
      image: { x: x.image, y: y.image, w: x.crop, h: y.crop },
      screen: { x: x.screen, y: y.screen, w: side, h: side },
    };
  });
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
