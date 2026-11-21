"use strict";

// Run after scripts/build-rust.ps1: the generated UI must ship the source page and
// stylesheet unchanged and start only the Rust/WASM application.
const assert = require("assert");
const fs = require("fs");
const path = require("path");

const root = path.join(__dirname, "..");
const read = (relative) => fs.readFileSync(path.join(root, relative));
for (const asset of ["index.html", "styles.css"]) {
  assert.deepEqual(read(`rust/dist/${asset}`), read(`rust/ui/assets/${asset}`), `${asset} must be copied unchanged`);
}
const html = read("rust/ui/assets/index.html").toString("utf8");
assert(html.includes('<script type="module" src="bootstrap.js"></script>'), "the page must load the WASM bootstrap");
assert(!/Content-Security-Policy/.test(html), "Tauri supplies the CSP header");
assert(!fs.existsSync(path.join(root, "rust/dist/app.js")), "no handwritten application JS is shipped");
assert(fs.statSync(path.join(root, "rust/dist/clipview_ui_bg.wasm")).size > 0);
assert.equal(read("rust/dist/bootstrap.js").toString("utf8"), 'import init from "./clipview_ui.js"; await init();');
console.log("Rust asset contract passed: unchanged HTML/CSS, Rust/WASM application entry");
