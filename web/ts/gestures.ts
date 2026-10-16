import { run } from "./keys.js";
import { state } from "./state.js";
import { canvas, resetZoom, scheduleDraw, zoomBy } from "./view.js";

interface Point {
  x: number;
  y: number;
}

const pointers = new Map<number, Point>();
let gesture: (Point & { touch: boolean; multi: boolean }) | null = null;
let lastTap = 0;

const midpoint = (a: Point, b: Point) => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });
const distance = (a: Point, b: Point) => Math.hypot(a.x - b.x, a.y - b.y) || 1;
const useTouch = () => document.body.classList.add("touch");

export function initGestures() {
  if (matchMedia("(pointer: coarse)").matches) useTouch();
  // iOS Safari ignores user-scalable=no.
  document.addEventListener("gesturestart", (e) => e.preventDefault());
  canvas.addEventListener("wheel", onWheel, { passive: false });
  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointermove", onPointerMove);
  canvas.addEventListener("pointerup", onPointerEnd);
  canvas.addEventListener("pointercancel", onPointerEnd);
  canvas.addEventListener("dblclick", resetZoom);
}

function onWheel(e: WheelEvent) {
  e.preventDefault();
  const delta = e.deltaY * [1, 16, 400][e.deltaMode];
  // Trackpad pinch arrives as a wheel event with ctrlKey.
  zoomBy(Math.exp(-delta / (e.ctrlKey ? 100 : 200)), { x: e.clientX, y: e.clientY });
}

function onPointerDown(e: PointerEvent) {
  if (e.pointerType === "touch") useTouch();
  canvas.setPointerCapture(e.pointerId);
  pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
  if (pointers.size === 1) gesture = { x: e.clientX, y: e.clientY, touch: e.pointerType === "touch", multi: false };
  else if (gesture) gesture.multi = true;
}

function onPointerMove(e: PointerEvent) {
  const p = pointers.get(e.pointerId);
  if (!p) return;
  const now = { x: e.clientX, y: e.clientY };
  if (pointers.size === 2) {
    const other = [...pointers.values()].find((q) => q !== p)!;
    const before = midpoint(p, other);
    const after = midpoint(now, other);
    zoomBy(distance(now, other) / distance(p, other), after, { x: after.x - before.x, y: after.y - before.y });
  } else if (pointers.size === 1 && state.zoom > 1) {
    state.pan.x += now.x - p.x;
    state.pan.y += now.y - p.y;
    scheduleDraw();
  }
  Object.assign(p, now);
}

function onPointerEnd(e: PointerEvent) {
  if (!pointers.delete(e.pointerId) || pointers.size || !gesture) return;
  const g = gesture;
  gesture = null;
  if (e.type === "pointercancel" || !g.touch || g.multi) return;
  const [dx, dy] = [e.clientX - g.x, e.clientY - g.y];
  if (state.zoom <= 1 && Math.abs(dx) > 50 && Math.abs(dx) > 1.5 * Math.abs(dy)) return run(dx < 0 ? "next" : "prev");
  if (Math.hypot(dx, dy) > 10) return;
  const t = performance.now();
  if (t - lastTap < 300) {
    resetZoom();
    lastTap = 0;
  } else {
    lastTap = t;
  }
}
