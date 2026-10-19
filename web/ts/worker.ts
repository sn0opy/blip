import init, { decode, type Decoded, type InitOutput, type Overlay } from "./pkg/blip_web.js";
import type { Reply, Request, Settings } from "./messages.js";

type LoadRequest = Extract<Request, { type: "load" }>;

interface Loaded {
  key: string;
  decoded: Decoded;
  fileBytes: number;
  readMs: number;
  decodeMs: number;
}

let ready: Promise<InitOutput>;
let wasm: InitOutput | undefined;
let loaded: Loaded | null = null;

const reply = (msg: Reply, transfer: Transferable[] = []) => postMessage(msg, transfer);

onmessage = async ({ data: msg }: MessageEvent<Request>) => {
  if (msg.type === "init") {
    ready = init({ module_or_path: msg.module });
    return;
  }
  wasm ??= await ready;
  try {
    if (msg.type === "load") await loadJob(msg);
    else exportJob(msg.id, await load(msg.key, msg.file), msg.settings, msg.jpeg);
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    const fatal = e instanceof WebAssembly.RuntimeError;
    reply({ type: "error", id: msg.id, error: fatal ? `internal error: ${message}` : message, fatal });
  }
  reply({ type: "done", id: msg.id, memory: wasm.memory.buffer.byteLength });
};

async function load(key: string, file: File): Promise<Loaded> {
  if (loaded?.key === key) return loaded;
  // Free the previous image first so this one can reuse its memory.
  loaded?.decoded.free();
  loaded = null;
  const t = performance.now();
  const bytes = new Uint8Array(await file.arrayBuffer());
  const readMs = performance.now() - t;
  const decoded = decode(bytes);
  loaded = { key, decoded, fileBytes: bytes.length, readMs, decodeMs: performance.now() - t - readMs };
  return loaded;
}

async function loadJob({ id, key, file, settings, render, stars }: LoadRequest) {
  const image = await load(key, file);
  if (render) await renderImage(id, image, settings);
  if (stars) measureStars(id, image);
}

function bitmap(ptr: number, len: number, width: number, height: number) {
  const view = new Uint8ClampedArray(wasm!.memory.buffer, ptr, len);
  return createImageBitmap(new ImageData(view, width, height));
}

async function renderImage(id: number, { key, decoded, fileBytes, readMs, decodeMs }: Loaded, settings: Settings) {
  const t = performance.now();
  const r = decoded.render(settings.maxSide, settings.neutralize, settings.autoStretch, settings.boost);
  const renderMs = performance.now() - t;
  try {
    const image = await bitmap(r.pixels_ptr(), r.pixels_len(), r.width, r.height);
    const overlay = await overlayBitmaps(r.take_overlay());
    const { total, dark, bright, level } = decoded.clip;
    reply(
      {
        type: "image",
        id,
        image: {
          key,
          image,
          overlay,
          width: r.width,
          height: r.height,
          bin: r.bin,
          srcWidth: decoded.width,
          srcHeight: decoded.height,
          bayer: decoded.bayer,
          info: decoded.info,
          clip: { total, dark, bright, level },
          stretch: { medians: r.medians, gains: r.gains, shadows: r.shadows, midtones: r.midtones, target: r.target },
          fileBytes,
          timings: { readMs, decodeMs, renderMs },
        },
      },
      overlay ? [image, overlay.plain, overlay.grown] : [image],
    );
  } finally {
    r.free();
  }
}

async function overlayBitmaps(o: Overlay | undefined) {
  if (!o) return null;
  try {
    return {
      plain: await bitmap(o.pixels_ptr(false), o.pixels_len(), o.width, o.height),
      grown: await bitmap(o.pixels_ptr(true), o.pixels_len(), o.width, o.height),
      cell: o.cell,
    };
  } finally {
    o.free();
  }
}

function measureStars(id: number, { key, decoded }: Loaded) {
  const t = performance.now();
  const s = decoded.stars();
  const ms = performance.now() - t;
  const list = s.list as Float32Array<ArrayBuffer>;
  const info = s.tilt;
  let tilt = null;
  if (info) {
    const c = info.corners;
    const corners = [0, 3, 6, 9].map((i) => ({ x: c[i], y: c[i + 1], hfr: c[i + 2] }));
    tilt = { corners, center: info.center, spread: info.spread, percent: info.percent, mean: info.mean };
    info.free();
  }
  reply({ type: "stars", id, stars: { key, list, tilt, median: s.median_hfr, mean: s.mean_hfr, ms } }, [list.buffer]);
  s.free();
}

function exportJob(id: number, { decoded }: Loaded, settings: Settings, jpeg: boolean) {
  const bytes = decoded.export(
    settings.neutralize,
    settings.autoStretch,
    settings.boost,
    jpeg,
  ) as Uint8Array<ArrayBuffer>;
  reply({ type: "exported", id, bytes }, [bytes.buffer]);
}
