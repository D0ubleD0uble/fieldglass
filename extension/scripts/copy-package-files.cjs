// Copies the repo-root CHANGELOG.md, LICENSE-APACHE and LICENSE-MIT into
// extension/ so vsce (which packages from this directory) ships them in the
// .vsix. README.md, LICENSE.md and NOTICE are hand-maintained in extension/:
// the Marketplace page is written for extension users, and the repo-root
// README is the project hub.
//
// Run automatically by `vscode:prepublish` (vsce package / vsce publish).
// Safe to run by hand: `node extension/scripts/copy-package-files.cjs`.

const fs = require('node:fs');
const path = require('node:path');

const ext = path.resolve(__dirname, '..');
const root = path.resolve(ext, '..');

const copies = [
  ['CHANGELOG.md', 'CHANGELOG.md'],
  ['LICENSE-APACHE', 'LICENSE-APACHE'],
  ['LICENSE-MIT', 'LICENSE-MIT'],
];

for (const [from, to] of copies) {
  const src = path.join(root, from);
  const dest = path.join(ext, to);
  fs.copyFileSync(src, dest);
  process.stdout.write(`copied ${from} -> extension/${to}\n`);
}
