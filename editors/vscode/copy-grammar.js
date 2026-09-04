const fs = require('fs');
const path = require('path');

const srcDir = path.join(__dirname, 'syntaxes');
const destDir = path.join(__dirname, 'out', 'syntaxes');

if (!fs.existsSync(destDir)) {
  fs.mkdirSync(destDir, { recursive: true });
}

fs.readdirSync(srcDir).forEach(file => {
  if (file.endsWith('.json') || file.endsWith('.tmLanguage')) {
    const src = path.join(srcDir, file);
    const dest = path.join(destDir, file);
    fs.copyFileSync(src, dest);
    console.log(`Copied ${file} to out/syntaxes/`);
  }
});
