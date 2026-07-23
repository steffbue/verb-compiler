import * as fs from "node:fs/promises";
import * as path from "node:path";

import * as vscode from "vscode";
import { Language, Parser } from "web-tree-sitter";
import type { Node } from "web-tree-sitter";

// Semantic tokens legend — see design spec §4. Custom, minimal: VSCode
// standard type names are reused, no new type names invented. No modifiers
// (YAGNI — nothing in the current highlight rules needs them).
const TOKEN_TYPES = [
  "comment",
  "string",
  "number",
  "keyword",
  "operator",
  "function",
  "variable",
  "parameter",
] as const;

type LegendName = (typeof TOKEN_TYPES)[number];

const LEGEND = new vscode.SemanticTokensLegend([...TOKEN_TYPES]);

// Anonymous-node keyword literals (spec §4 step 2).
const KEYWORD_LITERALS = new Set([
  "assign",
  "declare",
  "be",
  "make",
  "return",
  "check",
  "orelse",
  "repeat",
  "loop",
  "each",
  "in",
  "begin",
  "end",
  "import",
]);

// Anonymous-node operator literals (spec §4 step 2).
const OPERATOR_LITERALS = new Set([
  "add",
  "sub",
  "times",
  "div",
  "mod",
  "equals",
  "differs",
  "trails",
  "beats",
  "atmost",
  "atleast",
  "and",
  "or",
  "not",
  "neg",
  "join",
  "to",
]);

// The grammar WASM (and the web-tree-sitter runtime WASM it depends on) is
// loaded lazily, once, on first use — not reloaded on every
// provideDocumentSemanticTokens call. Concurrent first calls share the same
// in-flight promise.
let parserPromise: Promise<Parser> | undefined;

function getParser(context: vscode.ExtensionContext): Promise<Parser> {
  if (!parserPromise) {
    parserPromise = initParser(context).catch((err: unknown) => {
      // Don't cache a rejected promise forever — if the failure was
      // transient (e.g. parsers/*.wasm missing at the time), a later call
      // should retry rather than replay this same rejection for the rest
      // of the session.
      parserPromise = undefined;
      throw err;
    });
  }
  return parserPromise;
}

async function initParser(context: vscode.ExtensionContext): Promise<Parser> {
  const parsersDir = path.join(context.extensionPath, "parsers");

  // Supply the runtime WASM bytes directly (rather than relying on
  // web-tree-sitter's default locateFile lookup, which would resolve
  // relative to node_modules — a location that isn't guaranteed to exist
  // in a packaged extension). `wasmBinary` isn't part of web-tree-sitter's
  // published .d.ts (it only types the ambient, unresolved
  // `EmscriptenModule`), hence the cast.
  const runtimeWasmBytes = await fs.readFile(
    path.join(parsersDir, "tree-sitter.wasm")
  );
  await Parser.init({ wasmBinary: runtimeWasmBytes } as Parameters<
    typeof Parser.init
  >[0]);

  const grammarWasmBytes = await fs.readFile(
    path.join(parsersDir, "tree-sitter-verb.wasm")
  );
  const language = await Language.load(grammarWasmBytes);

  const parser = new Parser();
  parser.setLanguage(language);
  return parser;
}

// Maps a tree-sitter node to a legend index, per design spec §4's table.
// Mirrors `queries/highlights.scm`'s patterns, but implemented as a manual
// tree walk (not the tree-sitter query engine).
//
// Nodes with no mapping (punctuation, whitespace, ERROR nodes) return
// `undefined` and are simply skipped — never thrown on.
function classifyNode(
  node: Node,
  parent: Node | null,
  fieldName: string | null
): LegendName | undefined {
  const type = node.type;

  if (type === "int" || type === "float") {
    return "number";
  }
  if (type === "string" || type === "escape_sequence") {
    return "string";
  }
  if (type === "true" || type === "false" || type === "nil") {
    return "keyword";
  }
  if (type === "line_comment" || type === "block_comment") {
    return "comment";
  }
  // `mod`/`std` inside an import_statement are the import-kind keyword
  // (`import mod x;` / `import std io;`), not the `mod` arithmetic
  // operator — checked before the generic literal sets below, which would
  // otherwise misclassify `mod` as an operator (or leave `std` unstyled).
  if ((type === "mod" || type === "std") && parent?.type === "import_statement") {
    return "keyword";
  }
  if (KEYWORD_LITERALS.has(type)) {
    return "keyword";
  }
  if (OPERATOR_LITERALS.has(type)) {
    return "operator";
  }

  if (type === "identifier") {
    if (parent?.type === "fn_statement" && fieldName === "name") {
      return "function";
    }
    if (parent?.type === "call_expression" && fieldName === "function") {
      return "function";
    }
    if (parent?.type === "parameters") {
      return "parameter";
    }
    // Covers assign_statement.name, declare_statement.name,
    // reassign_statement.name, import_statement.library/module, and
    // every other bare identifier.
    return "variable";
  }

  return undefined;
}

// Pushes one token for `node`. SemanticTokensBuilder's range-based `push`
// requires single-line ranges, but a `block_comment` (`!?! ... !?!`) can
// span multiple lines, so multi-line nodes are split into one push per
// line.
function pushToken(
  builder: vscode.SemanticTokensBuilder,
  node: Node,
  tokenType: LegendName,
  document: vscode.TextDocument
): void {
  const start = node.startPosition;
  const end = node.endPosition;

  if (start.row === end.row) {
    if (end.column <= start.column) {
      return;
    }
    builder.push(
      new vscode.Range(start.row, start.column, end.row, end.column),
      tokenType
    );
    return;
  }

  for (let line = start.row; line <= end.row && line < document.lineCount; line++) {
    const lineLength = document.lineAt(line).text.length;
    const startChar = line === start.row ? start.column : 0;
    const endChar = line === end.row ? end.column : lineLength;
    if (endChar <= startChar) {
      continue;
    }
    builder.push(new vscode.Range(line, startChar, line, endChar), tokenType);
  }
}

// Recursively walks the whole tree (a full reparse per request, per spec —
// accepted cost), visiting every child (named and anonymous alike, since
// keyword/operator literals are anonymous nodes) and pushing a token for
// each node that classifies to a legend index.
function walk(
  node: Node,
  parent: Node | null,
  fieldName: string | null,
  document: vscode.TextDocument,
  builder: vscode.SemanticTokensBuilder
): void {
  const legendName = classifyNode(node, parent, fieldName);
  if (legendName !== undefined) {
    pushToken(builder, node, legendName, document);
  }

  const childCount = node.childCount;
  for (let i = 0; i < childCount; i++) {
    const child = node.child(i);
    if (!child) {
      continue;
    }
    walk(child, node, node.fieldNameForChild(i), document, builder);
  }
}

export function registerSemanticTokensProvider(
  context: vscode.ExtensionContext
): vscode.Disposable {
  const provider: vscode.DocumentSemanticTokensProvider = {
    async provideDocumentSemanticTokens(
      document: vscode.TextDocument
    ): Promise<vscode.SemanticTokens> {
      const builder = new vscode.SemanticTokensBuilder(LEGEND);
      try {
        const parser = await getParser(context);
        const tree = parser.parse(document.getText());
        if (tree) {
          // ERROR nodes (malformed source mid-edit) simply fail to
          // classify above and are skipped — no throw.
          walk(tree.rootNode, null, null, document, builder);
        }
      } catch (err) {
        console.error("verb: semantic tokens provider failed", err);
      }
      return builder.build();
    },
  };

  return vscode.languages.registerDocumentSemanticTokensProvider(
    { language: "verb" },
    provider,
    LEGEND
  );
}
