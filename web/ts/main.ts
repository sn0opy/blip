import { changed, updateText } from "./app.js";
import { initFiles } from "./files.js";
import { initGestures } from "./gestures.js";
import { initKeys } from "./keys.js";
import { initList } from "./list.js";
import { startLoader } from "./loader.js";
import { initView } from "./view.js";

startLoader(changed);
initView();
initList();
initFiles();
initKeys();
initGestures();
updateText();
