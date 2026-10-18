import type { Rendered, Settings, Stars } from "./messages.js";

export interface FileEntry {
  name: string;
  getFile(): Promise<File>;
}

export interface Source {
  label: string;
  rescan?: () => Promise<FileEntry[]>;
}

export interface Entry extends Rendered {
  name: string;
  settings: Settings;
  lastUsed: number;
}

export const EXTENSIONS = [".fit", ".fits", ".fts", ".xisf"];
const MAX_SIDE = 3200;
const CACHE_SIZE = 5;
const CACHE_SIZE_FULL = 2;

export const state = {
  source: null as Source | null,
  files: [] as FileEntry[],
  listed: new Set<string>(),
  selected: 0,
  direction: 1,
  errors: new Map<string, string>(),
  cache: new Map<string, Entry>(),
  tick: 0,
  shown: null as Entry | null,
  stars: new Map<string, Stars>(),
  starsFailed: new Set<string>(),
  selectedAt: null as { name: string; time: number } | null,
  shownAfter: null as number | null,

  neutralize: true,
  fullRes: false,
  unstretched: null as string | null,
  showInfo: true,
  showDebug: false,
  zoom: 1,
  pan: { x: 0, y: 0 },
  flashClip: null as string | null,
  starView: null as string | null,
  tiltView: null as string | null,
};

export const isImage = (name: string) => EXTENSIONS.some((ext) => name.toLowerCase().endsWith(ext));

export const current = (): FileEntry | undefined => state.files[state.selected];

export function settingsFor(name: string): Settings {
  return {
    maxSide: state.fullRes ? null : MAX_SIDE,
    neutralize: state.neutralize,
    autoStretch: state.unstretched !== name,
  };
}

export const sameSettings = (a: Settings, b: Settings) =>
  a.maxSide === b.maxSide && a.neutralize === b.neutralize && a.autoStretch === b.autoStretch;

export function valid(name: string | undefined): Entry | null {
  const entry = name === undefined ? undefined : state.cache.get(name);
  return entry && sameSettings(entry.settings, settingsFor(entry.name)) ? entry : null;
}

export function needsStars(name: string) {
  const entry = valid(name);
  return !!entry && state.stars.get(name)?.key !== entry.key && !state.starsFailed.has(entry.key);
}

export const cacheLimit = () => (state.fullRes ? CACHE_SIZE_FULL : CACHE_SIZE);

export function offsets() {
  const d = state.direction;
  return [0, d, 2 * d, -d, 3 * d].slice(0, cacheLimit());
}

export const wanted = () => offsets().flatMap((o) => state.files[state.selected + o] ?? []);
