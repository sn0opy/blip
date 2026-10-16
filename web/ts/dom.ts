export const $ = <T extends HTMLElement = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Partial<HTMLElementTagNameMap[K]> = {},
  ...children: Node[]
) {
  const element = Object.assign(document.createElement(tag), props);
  element.append(...children);
  return element;
}

export const errorMessage = (e: unknown) => (e instanceof Error ? e.message : String(e));

export const isAbort = (e: unknown) => e instanceof DOMException && e.name === "AbortError";

// Some browsers block storage (e.g. for local files or in private mode).
export const storage = (() => {
  try {
    return localStorage;
  } catch {
    return null;
  }
})();

let statusTimer = 0;

export function setStatus(text: string, ttl?: number) {
  const box = $("#status");
  clearTimeout(statusTimer);
  box.textContent = text;
  box.hidden = false;
  box.style.opacity = "1";
  if (ttl) statusTimer = setTimeout(() => (box.style.opacity = "0"), ttl);
}
