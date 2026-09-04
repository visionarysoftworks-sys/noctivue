// Noctivue Language Support Extension
// Provides LSP client for Noctivue language support

import * as vscode from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind
} from 'vscode-languageclient/node';
import * as path from 'path';
import * as fs from 'fs';

let client: LanguageClient;

export function activate(context: vscode.ExtensionContext) {
  console.log('Noctivue extension activated');

  // Get server path from configuration or use bundled
  const config = vscode.workspace.getConfiguration('noctivue');
  const serverPath = config.get<string>('lsp.serverPath', '');
  const lspEnabled = config.get<boolean>('lsp.enabled', true);

  if (!lspEnabled) {
    console.log('Noctivue LSP disabled by configuration');
    return;
  }

  // Determine server executable path
  let serverExecutable: string;
  if (serverPath) {
    serverExecutable = serverPath;
  } else {
    // Use bundled server from the project root
    const extensionRoot = context.extensionPath;
    const projectRoot = path.dirname(path.dirname(extensionRoot)); // Go up from editors/vscode/
    const binaryName = process.platform === 'win32' ? 'noctivue-lsp.exe' : 'noctivue-lsp';
    // Prefer the release binary (the tested, current build); fall back to
    // debug only if no release binary exists. NOTE: never prefer debug —
    // a stale debug binary shadowed release here and served months-old
    // hover behavior with no visible indication.
    serverExecutable = path.join(projectRoot, 'target', 'release', binaryName);
    if (!fs.existsSync(serverExecutable)) {
      serverExecutable = path.join(projectRoot, 'target', 'debug', binaryName);
    }
  }

  // Check if server exists
  if (!fs.existsSync(serverExecutable)) {
    const message = `Noctivue LSP server not found at: ${serverExecutable}. ` +
      `Please build the project with 'cargo build --release -p noctivue-lsp' ` +
      `or configure 'noctivue.lsp.serverPath' in settings.`;
    vscode.window.showErrorMessage(message);
    console.error(message);
    return;
  }

  console.log(`Starting Noctivue LSP server from: ${serverExecutable}`);

  // Server options
  const serverOptions: ServerOptions = {
    run: { command: serverExecutable, transport: TransportKind.stdio },
    debug: { command: serverExecutable, transport: TransportKind.stdio }
  };

  // Client options
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'noctivue' },
      { scheme: 'untitled', language: 'noctivue' }
    ],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.nv')
    },
    initializationOptions: {}
  };

  // Create and start the language client
  client = new LanguageClient(
    'noctivue',
    'Noctivue Language Server',
    serverOptions,
    clientOptions
  );

  // Start the client
  client.start().then(() => {
    console.log('Noctivue LSP client started');
  }, (error) => {
    console.error('Failed to start Noctivue LSP client:', error);
    vscode.window.showErrorMessage(`Failed to start Noctivue LSP: ${error}`);
  });

  context.subscriptions.push(
    vscode.commands.registerCommand('noctivue.restartServer', () => {
      if (client) {
        client.stop().then(() => client.start());
      }
    })
  );

  // Register a command to show output channel
  context.subscriptions.push(
    vscode.commands.registerCommand('noctivue.showOutput', () => {
      client.outputChannel.show();
    })
  );
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}