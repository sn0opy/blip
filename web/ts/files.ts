import { open, replaceFiles } from "./app.js";
import { $, errorMessage, isAbort, setStatus } from "./dom.js";
import { EXTENSIONS, state, type FileEntry } from "./state.js";

const fileEntry = (file: File): FileEntry => ({ name: file.name, getFile: async () => file });
const handleEntry = (h: FileSystemFileHandle): FileEntry => ({ name: h.name, getFile: () => h.getFile() });
const entryOf = (e: FileSystemFileEntry): FileEntry => ({
  name: e.name,
  getFile: () => new Promise((ok, err) => e.file(ok, err)),
});

export function initFiles() {
  $<HTMLInputElement>("#files").accept = EXTENSIONS.join(",");
  $<HTMLInputElement>("#files").onchange = (e) => {
    const picked = takeFiles(e);
    open({ label: `${picked.length} files` }, picked.map(fileEntry));
  };
  // The folder input also lists subfolders.
  $<HTMLInputElement>("#folder").onchange = (e) => {
    const picked = takeFiles(e);
    const top = picked.filter((f) => f.webkitRelativePath.split("/").length === 2);
    const label = picked[0]?.webkitRelativePath.split("/")[0] ?? "Folder";
    open({ label }, top.map(fileEntry));
  };
  addEventListener("dragover", (e) => e.preventDefault());
  addEventListener("drop", drop);
}

function takeFiles(e: Event) {
  const input = e.target as HTMLInputElement;
  const files = [...(input.files ?? [])];
  input.value = "";
  return files;
}

async function listDir(dir: FileSystemDirectoryHandle) {
  const entries: FileEntry[] = [];
  for await (const h of dir.values()) {
    if (h.kind === "file") entries.push(handleEntry(h as FileSystemFileHandle));
  }
  return entries;
}

async function readDirEntry(dir: FileSystemDirectoryEntry) {
  const reader = dir.createReader();
  const entries: FileEntry[] = [];
  for (;;) {
    const batch = await new Promise<FileSystemEntry[]>((ok, err) => reader.readEntries(ok, err));
    if (!batch.length) return entries;
    for (const e of batch) {
      if (e.isFile) entries.push(entryOf(e as FileSystemFileEntry));
    }
  }
}

const openDir = async (dir: FileSystemDirectoryHandle) =>
  open({ label: dir.name, rescan: () => listDir(dir) }, await listDir(dir));

export async function openFolder() {
  if (!window.showDirectoryPicker) return $("#folder").click();
  try {
    await openDir(await window.showDirectoryPicker({ id: "blip" }));
  } catch (e) {
    if (!isAbort(e)) setStatus(`Cannot open folder: ${errorMessage(e)}`, 8000);
  }
}

export async function openFiles() {
  if (!window.showOpenFilePicker) return $("#files").click();
  try {
    const handles = await window.showOpenFilePicker({
      id: "blip",
      multiple: true,
      types: [{ description: "FITS / XISF", accept: { "image/fits": EXTENSIONS } }],
    });
    open({ label: `${handles.length} files` }, handles.map(handleEntry));
  } catch (e) {
    if (!isAbort(e)) setStatus(`Cannot open files: ${errorMessage(e)}`, 8000);
  }
}

export const canRescan = () => !!state.source?.rescan;

export async function rescan() {
  const { source } = state;
  if (!source?.rescan) {
    setStatus("Rescan needs a folder opened with Open folder or dropped (Chromium-based browsers)", 4000);
    return;
  }
  let entries: FileEntry[] = [];
  try {
    entries = await source.rescan();
  } catch (e) {
    setStatus(`Cannot read folder: ${errorMessage(e)}`);
  }
  replaceFiles(entries);
}

async function drop(e: DragEvent) {
  e.preventDefault();
  const items = [...(e.dataTransfer?.items ?? [])].filter((i) => i.kind === "file");
  // The data transfer is cleared at the first await.
  const handles = items.map((i) => i.getAsFileSystemHandle?.());
  const entries = items.flatMap((i) => i.webkitGetAsEntry() ?? []);
  try {
    if (handles.length && handles.every(Boolean)) await dropHandles(handles as Promise<FileSystemHandle | null>[]);
    else await dropEntries(entries);
  } catch (err) {
    setStatus(`Cannot open: ${errorMessage(err)}`, 8000);
  }
}

async function dropHandles(pending: Promise<FileSystemHandle | null>[]) {
  const handles = (await Promise.all(pending)).filter((h) => h !== null);
  const isDir = (h: FileSystemHandle): h is FileSystemDirectoryHandle => h.kind === "directory";
  if (handles.length === 1 && isDir(handles[0])) return openDir(handles[0]);
  const lists = await Promise.all(
    handles.map((h) => (isDir(h) ? listDir(h) : [handleEntry(h as FileSystemFileHandle)])),
  );
  open({ label: `${handles.length} dropped items` }, lists.flat());
}

async function dropEntries(entries: FileSystemEntry[]) {
  const isDir = (e: FileSystemEntry): e is FileSystemDirectoryEntry => e.isDirectory;
  const lists = await Promise.all(
    entries.map((e) => (isDir(e) ? readDirEntry(e) : [entryOf(e as FileSystemFileEntry)])),
  );
  const label = entries.length === 1 && isDir(entries[0]) ? entries[0].name : `${entries.length} dropped items`;
  open({ label }, lists.flat());
}
