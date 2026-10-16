import { $, el, storage } from "./dom.js";
import { state } from "./state.js";

const list = $("#list");
let selectedItem: HTMLElement | null = null;

export function initList() {
  showList(!storage?.getItem("blip.listHidden"));
}

export function renderList(onSelect: (index: number) => void) {
  // The marks keep punctuation in place when T shows names right-to-left.
  const items = state.files.map((f, i) =>
    el("li", { textContent: `‎${f.name}‎`, title: f.name, onclick: () => onSelect(i) }),
  );
  list.replaceChildren(...items);
  selectedItem = null;
  updateList();
}

export function updateList() {
  for (const [i, item] of [...list.children].entries()) {
    item.classList.toggle("error", state.errors.has(state.files[i].name));
  }
  const item = list.children[state.selected] as HTMLElement | undefined;
  if (item === selectedItem) return;
  selectedItem?.classList.remove("selected");
  item?.classList.add("selected");
  selectedItem = item ?? null;
  revealSelected();
}

function revealSelected() {
  const item = selectedItem;
  if (!item) return;
  const top = item.previousSibling ? item.offsetTop - list.offsetTop : 0;
  const bottom = item.nextSibling ? item.offsetTop - list.offsetTop + item.offsetHeight : list.scrollHeight;
  if (top < list.scrollTop) list.scrollTop = top;
  else if (bottom > list.scrollTop + list.clientHeight) list.scrollTop = bottom - list.clientHeight;
}

export function toggleList() {
  showList(document.body.classList.contains("list-hidden"));
}

function showList(visible: boolean) {
  document.body.classList.toggle("list-hidden", !visible);
  storage?.setItem("blip.listHidden", visible ? "" : "1");
  if (visible) revealSelected();
}

export function toggleNameEnds() {
  list.classList.toggle("name-end");
}
