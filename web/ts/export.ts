import { $, el, errorMessage, isAbort, setStatus } from "./dom.js";
import { requestExport } from "./loader.js";
import { current, settingsFor } from "./state.js";

interface Target {
  name: string;
  jpeg: boolean;
  handle?: FileSystemFileHandle;
}

export async function exportCurrent() {
  const cur = current();
  if (!cur) return;
  const target = await pickTarget(cur.name.replace(/\.[^.]+$/, ""));
  if (!target) return;
  setStatus(`Exporting ${target.name}…`);
  requestExport({
    entry: cur,
    settings: { ...settingsFor(cur.name), maxSide: null },
    jpeg: target.jpeg,
    save: (bytes) => save(target, bytes),
  });
}

async function pickTarget(stem: string): Promise<Target | null> {
  if (window.showSaveFilePicker) {
    try {
      const handle = await window.showSaveFilePicker({
        id: "blip-export",
        suggestedName: `${stem}.png`,
        types: [
          { description: "PNG", accept: { "image/png": [".png"] } },
          { description: "JPEG", accept: { "image/jpeg": [".jpg", ".jpeg"] } },
        ],
      });
      return { handle, name: handle.name, jpeg: /\.jpe?g$/i.test(handle.name) };
    } catch (e) {
      if (!isAbort(e)) setStatus(`Export failed: ${errorMessage(e)}`, 8000);
      return null;
    }
  }
  const format = await ask($<HTMLDialogElement>("#export-format"));
  if (!format) return null;
  const jpeg = format === "jpeg";
  return { name: `${stem}.${jpeg ? "jpg" : "png"}`, jpeg };
}

function ask(dialog: HTMLDialogElement) {
  return new Promise<string>((resolve) => {
    dialog.returnValue = "";
    dialog.onclick = (e) => {
      const button = e.target as HTMLButtonElement;
      if (button.tagName === "BUTTON") dialog.close(button.value);
    };
    dialog.onkeydown = (e) => e.stopPropagation();
    dialog.onclose = () => resolve(dialog.returnValue);
    dialog.showModal();
  });
}

async function save(target: Target, bytes: Uint8Array<ArrayBuffer>) {
  try {
    if (target.handle) {
      const out = await target.handle.createWritable();
      await out.write(bytes);
      await out.close();
    } else {
      const url = URL.createObjectURL(new Blob([bytes], { type: target.jpeg ? "image/jpeg" : "image/png" }));
      el("a", { href: url, download: target.name }).click();
      setTimeout(() => URL.revokeObjectURL(url), 60_000);
    }
    setStatus(`Exported ${target.name}`, 2500);
  } catch (e) {
    setStatus(`Export failed: ${errorMessage(e)}`, 8000);
  }
}
