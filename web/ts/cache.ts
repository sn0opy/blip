import type { Rendered, Settings } from "./messages.js";
import { cacheLimit, sameSettings, settingsFor, state, valid, wanted, type Entry } from "./state.js";

function close({ image, overlay }: Rendered) {
  image.close();
  overlay?.plain.close();
  overlay?.grown.close();
}

export function addToCache(name: string, settings: Settings, image: Rendered) {
  if (!state.listed.has(name) || !sameSettings(settings, settingsFor(name))) return close(image);
  const old = state.cache.get(name);
  if (old && old !== state.shown) close(old);
  state.cache.set(name, { ...image, name, settings, lastUsed: ++state.tick });
}

export function show(next: Entry | null) {
  const prev = state.shown;
  if (prev && prev !== next && state.cache.get(prev.name) !== prev) close(prev);
  if (next) next.lastUsed = ++state.tick;
  state.shown = next;
}

export function clearCache() {
  show(null);
  for (const entry of state.cache.values()) close(entry);
  state.cache.clear();
}

export function evict() {
  const { cache, shown } = state;
  const drop = (entry: Entry) => {
    close(entry);
    cache.delete(entry.name);
  };
  for (const entry of [...cache.values()]) {
    if (entry !== shown && (!state.listed.has(entry.name) || !valid(entry.name))) drop(entry);
  }
  const keep = new Set(wanted().map((f) => f.name));
  const spare = [...cache.values()]
    .filter((e) => e !== shown && !keep.has(e.name))
    .sort((a, b) => a.lastUsed - b.lastUsed);
  while (cache.size > cacheLimit() && spare.length) drop(spare.shift()!);
}
