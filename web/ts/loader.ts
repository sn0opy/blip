import { addToCache } from "./cache.js";
import { errorMessage, setStatus } from "./dom.js";
import type { Reply, Request, Settings } from "./messages.js";
import { current, needsStars, sameSettings, settingsFor, state, valid, wanted, type FileEntry } from "./state.js";

const WORKERS = 2;

export interface ExportRequest {
  entry: FileEntry;
  settings: Settings;
  jpeg: boolean;
  save: (bytes: Uint8Array<ArrayBuffer>) => void;
}

interface ExportJob extends ExportRequest {
  type: "export";
  id: number;
  error: string | null;
}

interface LoadJob {
  type: "load";
  id: number;
  entry: FileEntry;
  settings: Settings;
  plan: Plan | null;
  imageDone: boolean;
  error: string | null;
}

// Decided once the file is read: what of it still needs loading.
interface Plan {
  key: string;
  render: boolean;
  stars: boolean;
}

type Job = LoadJob | ExportJob;

const wasmModule = fetch("pkg/blip_web_bg.wasm")
  .then((r) => r.arrayBuffer())
  .then((bytes) => WebAssembly.compile(bytes));

class Slot {
  worker = this.spawn();
  job: Job | null = null;
  decoded: string | null = null;
  memory = 0;

  get loading() {
    return this.job?.type === "load" ? this.job : null;
  }

  restart() {
    this.worker.terminate();
    this.worker = this.spawn();
    this.job = null;
    this.decoded = null;
    this.memory = 0;
    return this;
  }

  post(msg: Request) {
    this.worker.postMessage(msg);
  }

  private spawn() {
    const worker = new Worker("worker.js", { type: "module" });
    wasmModule.then((module) => worker.postMessage({ type: "init", module } satisfies Request));
    worker.onmessage = ({ data }: MessageEvent<Reply>) => {
      if (this.worker === worker) onReply(this, data);
    };
    worker.onerror = (e) => setStatus(`Worker failed: ${e.message}`);
    return worker;
  }
}

const slots: Slot[] = [];
let onUpdate = () => {};
let nextId = 0;
let pendingExport: ExportRequest | null = null;

export function startLoader(update: () => void) {
  onUpdate = update;
  for (let i = 0; i < WORKERS; i++) slots.push(new Slot());
}

// Before its file is read, a job may still render.
const renders = (job: LoadJob) => job.plan?.render ?? true;

const isLoading = (name: string, image = false) =>
  slots.some((s) => s.loading?.entry.name === name && (!image || renders(s.loading)));

export function fileState(name: string) {
  if (valid(name)) return "cached";
  if (state.errors.has(name)) return "error";
  return isLoading(name) ? "loading" : "queued";
}

export function status() {
  const waiting = wanted().filter((f) => !state.errors.has(f.name) && !isLoading(f.name));
  const describe = (job: Job | null) => {
    if (!job) return "idle";
    if (job.type === "export") return "export";
    return `${renders(job) ? "image" : "stars"} ${job.entry.name}`;
  };
  return {
    workers: slots.map((s) => ({ memory: s.memory, work: describe(s.job) })),
    images: waiting.filter((f) => !valid(f.name)).length,
    stars: waiting.filter((f) => (valid(f.name) ? needsStars(f.name) : !state.stars.has(f.name))).length,
  };
}

export function requestExport(request: ExportRequest) {
  pendingExport = request;
  pump();
}

export function restartLoads() {
  for (const slot of slots) if (slot.loading) slot.restart();
}

export function pump() {
  const files = wanted();
  stopUnwanted(files);
  const cur = current();
  if (cur && !valid(cur.name) && !state.errors.has(cur.name)) return loadCurrent(cur, files);
  startExport();
  prefetch(files);
}

function stopUnwanted(files: FileEntry[]) {
  for (const slot of slots) {
    const job = slot.loading;
    if (!job) continue;
    const name = job.entry.name;
    if (!files.some((f) => f.name === name) || (renders(job) && !sameSettings(job.settings, settingsFor(name)))) {
      slot.restart();
    }
  }
}

function loadCurrent(cur: FileEntry, files: FileEntry[]) {
  if (isLoading(cur.name, true)) return;
  const slot = idleSlot(cur.name) ?? leastWanted(files)?.restart();
  if (slot) load(slot, cur);
}

function startExport() {
  const request = pendingExport;
  const slot = request && idleSlot(request.entry.name);
  if (!slot) return;
  pendingExport = null;
  start(slot, { ...request, type: "export", id: ++nextId, error: null });
}

function prefetch(files: FileEntry[]) {
  // One worker stays free for the image the user moves to next.
  while (slots.filter((s) => !s.job).length > 1) {
    const f = files.find(
      (f) => !state.errors.has(f.name) && !isLoading(f.name) && (!valid(f.name) || needsStars(f.name)),
    );
    if (!f) return;
    load(idleSlot(f.name)!, f);
  }
}

function idleSlot(name: string) {
  const idle = slots.filter((s) => !s.job);
  return idle.find((s) => s.decoded === name) ?? idle[0];
}

function leastWanted(files: FileEntry[]) {
  const rank = (s: Slot) => files.findIndex((f) => f.name === s.loading?.entry.name);
  return slots.filter((s) => s.loading).sort((a, b) => rank(b) - rank(a))[0];
}

function load(slot: Slot, entry: FileEntry) {
  const settings = settingsFor(entry.name);
  start(slot, { type: "load", id: ++nextId, entry, settings, plan: null, imageDone: false, error: null });
}

async function start(slot: Slot, job: Job) {
  slot.job = job;
  const name = job.entry.name;
  const file = await job.entry.getFile().catch((e: unknown) => {
    state.errors.set(name, errorMessage(e));
    return null;
  });
  if (!file) {
    if (slot.job === job) slot.job = null;
    if (job.type === "export") setStatus(`Export failed: ${state.errors.get(name)}`, 8000);
    return onUpdate();
  }
  if (slot.job !== job) return;

  const key = `${file.name}:${file.size}:${file.lastModified}`;
  const { id, settings } = job;
  if (job.type === "export") return slot.post({ type: "export", id, key, file, settings, jpeg: job.jpeg });
  const render = valid(name)?.key !== key;
  const stars = state.stars.get(name)?.key !== key && !state.starsFailed.has(key);
  if (!render && !stars) {
    slot.job = null;
    return;
  }
  job.plan = { key, render, stars };
  slot.post({ type: "load", id, key, file, settings, render, stars });
}

function onReply(slot: Slot, reply: Reply) {
  const job = slot.job;
  if (!job || reply.id !== job.id) return;
  switch (reply.type) {
    case "image":
      if (job.type !== "load") break;
      job.imageDone = true;
      addToCache(job.entry.name, job.settings, reply.image);
      break;
    case "stars":
      state.stars.set(job.entry.name, reply.stars);
      break;
    case "exported":
      if (job.type === "export") job.save(reply.bytes);
      break;
    case "error":
      failed(job, reply.error);
      // The wasm instance is unusable after a trap, and its memory never shrinks.
      if (reply.fatal) slot.restart();
      break;
    case "done": {
      const decoded = !job.error || (job.type === "load" && job.imageDone);
      slot.decoded = decoded ? job.entry.name : null;
      slot.memory = reply.memory;
      slot.job = null;
      break;
    }
  }
  onUpdate();
}

function failed(job: Job, error: string) {
  job.error = error;
  if (job.type === "export") setStatus(`Export failed: ${error}`, 8000);
  else if (job.plan?.render && !job.imageDone) state.errors.set(job.entry.name, error);
  else if (job.plan) state.starsFailed.add(job.plan.key);
}
