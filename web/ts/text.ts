import { fileState, status } from "./loader.js";
import { cacheLimit, offsets, state, type Entry } from "./state.js";

const thousands = (n: number) => Math.round(n).toLocaleString("en-US");
const megabytes = (bytes: number) => bytes / (1 << 20);
const ms = (n: number) => `${n.toFixed(0)} ms`;
const onOff = (b: boolean) => (b ? "on" : "off");

export function infoText(s: Entry) {
  const lines = [s.name];
  if (s.info) lines.push(s.info);
  if (!s.settings.autoStretch) lines.push("auto stretch off");
  const { total, dark, bright } = s.clip;
  if (total > 0) {
    const clipped = (n: number) => `${thousands(n)} (${((n * 100) / total).toFixed(3)}%)`;
    lines.push(`clipped: dark ${clipped(dark)} · bright ${clipped(bright)}`);
  }
  lines.push(`stars: ${starsText(s)}`);
  const stars = state.stars.get(s.name);
  const tilt = stars?.key === s.key && stars.tilt;
  if (tilt) {
    const px = (n: number) => `${n.toFixed(2)} px`;
    lines.push(
      `tilt: ${px(tilt.spread)} (${tilt.percent.toFixed(0)}%) · corners ${px(tilt.mean)} · center ${px(tilt.center)}`,
    );
  }
  return lines.join("\n");
}

function starsText(s: Entry) {
  const stars = state.stars.get(s.name);
  if (state.starsFailed.has(s.key)) return "none found";
  if (stars?.key !== s.key) return "measuring…";
  if (!stars.list.length) return "none found";
  const { list, median, mean } = stars;
  return `${thousands(list.length / 3)} · HFR median ${median.toFixed(2)} px · mean ${mean.toFixed(2)} px`;
}

export function debugText(s: Entry) {
  const { shadows, midtones, medians, gains, target } = s.stretch;
  const { readMs, decodeMs, renderMs } = s.timings;
  const stars = state.stars.get(s.name);
  const channels = medians.length === 3 ? ["R", "G", "B"] : ["L"];
  const perChannel = (values: Float64Array, format: (v: number) => string) =>
    channels.map((c, i) => `${c} ${format(values[i])}`).join(" ");
  const size = megabytes(s.fileBytes);
  const rate = size / Math.max(readMs / 1000, 1e-3);
  const shownAfter = s.name === state.selectedAt?.name ? state.shownAfter : null;
  const bayer = s.bayer ? ` ${s.bayer}` : "";
  const mode = s.bin === 1 ? "full res" : `bin ${s.bin}`;
  const stretch = s.settings.autoStretch
    ? `stretch shadows ${shadows.toFixed(4)} · midtones ${midtones.toFixed(4)} · target ${target}`
    : "stretch off";
  return [
    `sensor ${s.srcWidth}×${s.srcHeight}${bayer} → preview ${s.width}×${s.height} (${mode})`,
    shownAfter === null ? "selected image still loading" : `shown ${ms(shownAfter)} after selecting`,
    `read ${ms(readMs)} (${size.toFixed(0)} MB, ${rate.toFixed(0)} MB/s) · decode ${ms(decodeMs)}`,
    `render ${ms(renderMs)} · stars ${stars?.key === s.key ? ms(stars.ms) : "…"}`,
    `background ${perChannel(medians, (v) => (v * 65535).toFixed(0))} ADU · gain ${perChannel(gains, (v) => v.toFixed(2))}`,
    `${stretch} · neutralize ${onOff(s.settings.neutralize)}`,
    `saturation level ${s.clip.level} ADU`,
  ].join("\n");
}

export function debugGeneral() {
  const pixels = (e: Entry) =>
    e.width * e.height + (e.overlay ? 2 * e.overlay.plain.width * e.overlay.plain.height : 0);
  const bitmaps = [...state.cache.values()].reduce((sum, e) => sum + 4 * pixels(e), 0);
  const prefetch = offsets()
    .slice(1)
    .flatMap((o) => {
      const f = state.files[state.selected + o];
      return f ? [`${o > 0 ? "+" : ""}${o} ${fileState(f.name)}`] : [];
    });
  const { workers, images, stars } = status();
  return [
    `cache ${state.cache.size}/${cacheLimit()} images · ${megabytes(bitmaps).toFixed(0)} MB bitmaps · ${state.stars.size} star measurements`,
    `prefetch ${prefetch.join(" · ")}`,
    `queue: ${images} images · ${stars} star measurements waiting`,
    ...workers.map((w, i) => `worker ${i + 1}: ${megabytes(w.memory).toFixed(0)} MB wasm · ${w.work}`),
  ].join("\n");
}
