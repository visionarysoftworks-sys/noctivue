// Bundles src/extension.ts (+ vscode-languageclient) into a single
// out/extension.js so the packaged VSIX needs no node_modules at runtime.
// Run: node esbuild.js [watch]
const esbuild = require('esbuild');

const watch = process.argv.includes('watch');

async function build() {
  const ctx = await esbuild.context({
    entryPoints: ['src/extension.ts'],
    bundle: true,
    platform: 'node',
    target: 'node20',
    format: 'cjs',
    sourcemap: true,
    outfile: 'out/extension.js',
    external: ['vscode'], // provided by the extension host
    logLevel: 'info',
  });
  if (watch) {
    await ctx.watch();
    console.log('watching...');
  } else {
    await ctx.rebuild();
    await ctx.dispose();
  }
}

build().catch((e) => { console.error(e); process.exit(1); });
