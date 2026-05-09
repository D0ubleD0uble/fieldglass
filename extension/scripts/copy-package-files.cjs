// Copies the repo-root README.md and CHANGELOG.md into extension/ so vsce
// (which packages from this directory) ships them on the Marketplace listing.
// LICENSE.md is hand-maintained in extension/ — see this directory.
//
// Run automatically by `vscode:prepublish` (vsce package / vsce publish).
// Safe to run by hand: `node extension/scripts/copy-package-files.cjs`.

const fs = require('node:fs');
const path = require('node:path');

const ext = path.resolve(__dirname, '..');
const root = path.resolve(ext, '..');

const copies = [
  ['README.md', 'README.md'],
  ['CHANGELOG.md', 'CHANGELOG.md'],
];

for (const [from, to] of copies) {
  const src = path.join(root, from);
  const dest = path.join(ext, to);
  fs.copyFileSync(src, dest);
  process.stdout.write(`copied ${from} -> extension/${to}\n`);
}
