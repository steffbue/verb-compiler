import * as vscode from "vscode";

// STUB: Task 4 replaces this stub's body with the real web-tree-sitter-based
// semantic tokens provider (grammar loading, tree walk, legend mapping —
// see design spec §4). This placeholder registers nothing and exists only
// so `extension.ts` has a stable `./highlight` import to compile and wire
// disposal against.
export function registerSemanticTokensProvider(
  _context: vscode.ExtensionContext
): vscode.Disposable {
  return {
    dispose() {
      // no-op
    },
  };
}
