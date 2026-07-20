#!/usr/bin/env node
// Copies the two WASM blobs the extension needs at runtime into `parsers/`:
//   - node_modules/web-tree-sitter/tree-sitter.wasm  (the tree-sitter runtime)
//   - ../tree-sitter-verb/tree-sitter-verb.wasm       (the compiled Verb grammar,
//     produced by `npm run build:wasm` in editors/tree-sitter-verb — that's a
//     separate, slow step and is NOT invoked from here; it must already exist)
//
// Plain Node/fs rather than `cp` (not portable across shells) or an extra
// devDependency like cpx/shx (unnecessary for two static file copies).
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const root = path.join(__dirname, "..");
const parsersDir = path.join(root, "parsers");

const copies = [
  {
    src: path.join(root, "node_modules", "web-tree-sitter", "tree-sitter.wasm"),
    dest: path.join(parsersDir, "tree-sitter.wasm"),
  },
  {
    src: path.join(root, "..", "tree-sitter-verb", "tree-sitter-verb.wasm"),
    dest: path.join(parsersDir, "tree-sitter-verb.wasm"),
  },
];

fs.mkdirSync(parsersDir, { recursive: true });

for (const { src, dest } of copies) {
  if (!fs.existsSync(src)) {
    console.error(`copy-wasm: missing source file ${src}`);
    if (src.includes("tree-sitter-verb.wasm")) {
      console.error(
        "copy-wasm: run `npm run build:wasm` in editors/tree-sitter-verb first " +
          "(it's a slow Docker-based build, so it isn't run automatically here)."
      );
    }
    process.exit(1);
  }
  fs.copyFileSync(src, dest);
  console.log(`copy-wasm: ${path.relative(root, src)} -> ${path.relative(root, dest)}`);
}
