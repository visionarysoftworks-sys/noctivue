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

  // Keep `.noct/` (fetched dependency trees, resolver caches, build
  // output) out of the Explorer, search, and the file watcher. Two
  // reasons, both practical: a dependency's sources are not the user's
  // code, so letting the server analyze and report on them produces
  // diagnostics that look like the user's own; and watching a
  // multi-megabyte tree churns the workspace for no benefit.
  //
  // `configurationDefaults` already ships these keys, so a fresh
  // workspace is correct with zero code here. This block only makes the
  // setting enforceable: turning it OFF removes the keys so the user can
  // manage `.noct` visibility themselves, and turning it back ON
  // restores them.
  void applyFetchedDependencyExcludes();

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

  // Client options: one client, one server binary, two languages.
  // Both `.nv` and `nestpkg` (nestpkg.nvpm / nestpkg.lock) are served
  // by the same noctivue-lsp process, which routes by URI filename.
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'noctivue' },
      { scheme: 'untitled', language: 'noctivue' },
      { scheme: 'file', language: 'nestpkg' },
      { scheme: 'untitled', language: 'nestpkg' }
    ],
    synchronize: {
      // One watcher per family: `*.lock` is deliberately NOT watched —
      // only the fixed `nestpkg.lock` basename is claimed (Cargo.lock
      // must never route to this server).
      fileEvents: [
        vscode.workspace.createFileSystemWatcher('**/*.nv'),
        vscode.workspace.createFileSystemWatcher('**/*.nvpm'),
        vscode.workspace.createFileSystemWatcher('**/nestpkg.lock')
      ]
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

/**
 * Sync the `files.exclude` / `search.exclude` / `files.watcherExclude`
 * entries for `.noct` with the `noctivue.excludeFetchedDependencies`
 * setting.
 *
 * Each section is merged rather than replaced, so unrelated user excludes
 * (node_modules, .venv, target) survive. Writes go to the Workspace
 * target: they are derived from a setting, so they belong to the
 * workspace rather than the user's global profile. Nothing is written
 * when the effective value already matches, so a normal activation is a
 * no-op and a user who hand-edited these keys is only overridden if they
 * leave the setting enabled.
 */
async function applyFetchedDependencyExcludes(): Promise<void> {
  const wanted = vscode.workspace
    .getConfiguration('noctivue')
    .get<boolean>('excludeFetchedDependencies', true);

  const sections: Array<[string, string, Record<string, boolean>]> = [
    // [configuration section, key within it, entries to enforce]
    ['files', 'exclude', { '.noct': true }],
    ['search', 'exclude', { '.noct': true }],
    ['files', 'watcherExclude', { '.noct/**': true }]
  ];

  for (const [section, key, entries] of sections) {
    const config = vscode.workspace.getConfiguration(section);
    const current = config.get<Record<string, boolean>>(key, {});
    const differs = Object.entries(entries).some(
      ([pattern, value]) => Boolean(current?.[pattern]) !== value
    );
    const hasAny = Object.keys(entries).some((pattern) => pattern in (current ?? {}));
    if (wanted && !differs) {
      continue;
    }
    if (!wanted && !hasAny) {
      continue;
    }
    const next = { ...(current ?? {}) };
    for (const [pattern, value] of Object.entries(entries)) {
      if (value) {
        next[pattern] = true;
      } else {
        delete next[pattern];
      }
    }
    try {
      await config.update(key, next, vscode.ConfigurationTarget.Workspace);
    } catch (error) {
      // A read-only or untrusted workspace must not break activation;
      // the exclusion is an optimization, not a correctness requirement.
      console.warn(`Noctivue: could not update ${section}.${key}: ${error}`);
    }
  }
}