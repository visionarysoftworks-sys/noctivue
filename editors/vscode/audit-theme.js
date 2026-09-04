// Audit: for each scope our TextMate grammar emits, find the best-matching
// rule in one or more VS Code theme files (prefix-segment matching with
// "include"-chain resolution, as VS Code resolves it).
//
// Usage: node audit-theme.js <theme.json> [<theme.json> ...]
// See TOOLCHAIN.md §6.3 for the scope-naming convention this verifies.
const fs = require('fs');
const path = require('path');

const grammar = JSON.parse(fs.readFileSync('./syntaxes/noctivue.tmLanguage.json', 'utf8'));
const scopes = new Set();
const walk = (p) => { if (p.name) scopes.add(p.name); (p.patterns || []).forEach(walk); };
grammar.patterns.forEach(walk);

function themeRules(themePath, seen = new Set()) {
  const norm = themePath.toLowerCase();
  if (seen.has(norm)) return [];
  seen.add(norm);
  const theme = JSON.parse(fs.readFileSync(themePath, 'utf8'));
  const rules = [];
  // Follow "include" chains (e.g. dark_plus.json includes dark_vs.json)
  if (theme.include) {
    rules.push(...themeRules(path.join(path.dirname(themePath), theme.include.replace(/^\.\//, '')), seen));
  }
  for (const tc of theme.tokenColors || []) {
    if (!tc.scope || !tc.settings || !tc.settings.foreground) continue;
    const list = Array.isArray(tc.scope) ? tc.scope : tc.scope.split(',');
    for (const s of list) rules.push({ scope: s.trim(), fg: tc.settings.foreground });
  }
  return rules;
}

// A rule matches a token when every dot-segment of the rule scope equals the
// corresponding leading segments of the token scope.
function matches(ruleScope, tokenScope) {
  const r = ruleScope.split('.');
  const t = tokenScope.split('.');
  if (r.length > t.length) return false;
  return r.every((seg, i) => seg === t[i]);
}

let failed = false;
for (const themePath of process.argv.slice(2)) {
  console.log('=== ' + themePath.split(/[\\/]/).pop() + ' ===');
  const rules = themeRules(themePath);
  let uncovered = 0;
  for (const scope of [...scopes].sort()) {
    let best = null;
    for (const r of rules) {
      if (matches(r.scope, scope) && (!best || r.scope.length > best.scope.length)) best = r;
    }
    if (best) {
      console.log(`  OK   ${scope}  <-  ${best.scope} (${best.fg})`);
    } else {
      uncovered++;
      console.log(`  MISS ${scope}  (falls back to editor.foreground)`);
    }
  }
  if (uncovered > 0) failed = true;
  console.log(uncovered === 0 ? 'ALL COVERED' : `${uncovered} UNCOVERED`);
  console.log('');
}
process.exit(failed ? 1 : 0);
