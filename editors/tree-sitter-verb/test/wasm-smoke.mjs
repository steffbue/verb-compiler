// Smoke test for the tree-sitter-verb WASM build (`npm run build:wasm`).
//
// Loads `tree-sitter-verb.wasm` with `web-tree-sitter` and parses
// `examples/demo.verb`, asserting the resulting tree contains no ERROR
// node anywhere (a top-level ERROR node would mean the parse failed
// almost entirely; this also catches ERROR nodes nested deeper in the
// tree, which is a strictly stronger check).
//
// Run: node test/wasm-smoke.mjs
// (requires `tree-sitter-verb.wasm` to already exist — see `npm run build:wasm`)

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { Parser, Language } from "web-tree-sitter";

const here = path.dirname(fileURLToPath(import.meta.url));
const wasmPath = path.join(here, "..", "tree-sitter-verb.wasm");
const demoPath = path.join(here, "..", "..", "..", "examples", "demo.verb");

function findErrorNode(node) {
  if (node.isError || node.type === "ERROR") {
    return node;
  }
  for (let i = 0; i < node.childCount; i++) {
    const child = node.child(i);
    if (child) {
      const found = findErrorNode(child);
      if (found) return found;
    }
  }
  return null;
}

async function main() {
  await Parser.init();

  const wasmBytes = await readFile(wasmPath);
  const VerbLanguage = await Language.load(wasmBytes);

  const parser = new Parser();
  parser.setLanguage(VerbLanguage);

  const source = await readFile(demoPath, "utf8");
  const tree = parser.parse(source);

  if (!tree) {
    console.error("FAIL: parser.parse() returned null");
    process.exit(1);
  }

  const root = tree.rootNode;

  if (root.type === "ERROR") {
    console.error("FAIL: root node itself is an ERROR node");
    console.error(root.toString());
    process.exit(1);
  }

  if (root.hasError) {
    const errorNode = findErrorNode(root);
    console.error("FAIL: tree contains an ERROR node");
    if (errorNode) {
      console.error(
        `  at ${errorNode.startPosition.row + 1}:${errorNode.startPosition.column + 1}`,
      );
      console.error(`  text: ${JSON.stringify(errorNode.text.slice(0, 80))}`);
    }
    process.exit(1);
  }

  console.log(
    `OK: parsed ${path.relative(process.cwd(), demoPath)} (${root.childCount} top-level nodes, no ERROR nodes)`,
  );
}

main().catch((err) => {
  console.error("FAIL:", err);
  process.exit(1);
});
