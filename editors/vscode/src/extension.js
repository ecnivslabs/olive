// VSCode client for `pit lsp`. Spawns the server as a child process over
// stdio, exactly what `pit lsp` speaks -- no custom transport, just the
// standard vscode-languageclient wiring every LSP extension uses.

const { workspace } = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function activate(context) {
  const config = workspace.getConfiguration("olive");
  const serverPath = config.get("serverPath") || "pit";

  const serverOptions = {
    command: serverPath,
    args: ["lsp"],
    transport: TransportKind.stdio,
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "olive" }],
  };

  client = new LanguageClient(
    "olive",
    "Olive Language Server",
    serverOptions,
    clientOptions
  );

  context.subscriptions.push(client);
  client.start();
}

function deactivate() {
  if (!client) {
    return undefined;
  }
  return client.stop();
}

module.exports = { activate, deactivate };
