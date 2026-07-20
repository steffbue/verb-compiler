import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

import { registerSemanticTokensProvider } from "./highlight";

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const config = vscode.workspace.getConfiguration("verb");
  const lspPath = config.get<string>("lspPath", "");

  const serverOptions: ServerOptions = {
    command: lspPath,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ language: "verb" }],
  };

  client = new LanguageClient(
    "verb",
    "Verb Language Server",
    serverOptions,
    clientOptions
  );

  void client.start().catch((err: unknown) => {
    console.error("verb: language client failed to start", err);
  });
  context.subscriptions.push(client);

  const semanticTokensProvider = registerSemanticTokensProvider(context);
  context.subscriptions.push(semanticTokensProvider);
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
