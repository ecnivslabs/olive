// VSCode client for `pit lsp` and `pit dap`. Both spawn as a child process
// over stdio -- the language server and the debug adapter need no custom
// transport, just the standard client wiring each protocol already expects.

const vscode = require("vscode");
const { workspace } = vscode;
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function pitPath() {
  return workspace.getConfiguration("olive").get("serverPath") || "pit";
}

function activate(context) {
  const serverOptions = {
    command: pitPath(),
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

  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory("olive", {
      createDebugAdapterDescriptor() {
        return new vscode.DebugAdapterExecutable(pitPath(), ["dap"]);
      },
    })
  );
}

function deactivate() {
  if (!client) {
    return undefined;
  }
  return client.stop();
}

module.exports = { activate, deactivate };
