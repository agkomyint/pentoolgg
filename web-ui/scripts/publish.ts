// Copy the single-file build to where the Rust server embeds it.
import { copyFileSync, statSync } from "node:fs";
const from = new URL("../dist/index.html", import.meta.url);
const to = new URL("../../web/studio.html", import.meta.url);
copyFileSync(from, to);
console.log(`published web/studio.html (${statSync(to).size} bytes)`);
