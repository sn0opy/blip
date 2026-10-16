export interface Settings {
  maxSide: number | null;
  neutralize: boolean;
  autoStretch: boolean;
}

interface Job {
  id: number;
  key: string;
  file: File;
  settings: Settings;
}

export type Request =
  | { type: "init"; module: WebAssembly.Module }
  | (Job & { type: "load"; render: boolean; stars: boolean })
  | (Job & { type: "export"; jpeg: boolean });

export interface Clip {
  total: number;
  dark: number;
  bright: number;
  level: number;
}

export interface Stretch {
  medians: Float64Array;
  gains: Float64Array;
  shadows: number;
  midtones: number;
  target: number;
}

export interface Timings {
  readMs: number;
  decodeMs: number;
  renderMs: number;
}

export interface Overlay {
  plain: ImageBitmap;
  grown: ImageBitmap;
  cell: number;
}

export interface Rendered {
  key: string;
  image: ImageBitmap;
  overlay: Overlay | null;
  width: number;
  height: number;
  bin: number;
  srcWidth: number;
  srcHeight: number;
  bayer: string | undefined;
  info: string;
  clip: Clip;
  stretch: Stretch;
  fileBytes: number;
  timings: Timings;
}

export interface Stars {
  key: string;
  list: Float32Array<ArrayBuffer>;
  median: number;
  mean: number;
  ms: number;
}

export type Reply =
  | { type: "image"; id: number; image: Rendered }
  | { type: "stars"; id: number; stars: Stars }
  | { type: "exported"; id: number; bytes: Uint8Array<ArrayBuffer> }
  | { type: "error"; id: number; error: string; fatal: boolean }
  | { type: "done"; id: number; memory: number };
