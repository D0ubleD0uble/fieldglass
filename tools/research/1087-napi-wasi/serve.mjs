// Static server for the #1087 browser probe. Three ports, three header sets:
//   8701  COOP same-origin + COEP require-corp
//   8702  no isolation headers
//   8703  COOP same-origin + COEP credentialless
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { join, extname, normalize } from 'node:path';

const ROOT = new URL('./web/', import.meta.url).pathname;
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.wasm': 'application/wasm' };
const MODES = {
  8701: { 'Cross-Origin-Opener-Policy': 'same-origin', 'Cross-Origin-Embedder-Policy': 'require-corp' },
  8702: {},
  8703: { 'Cross-Origin-Opener-Policy': 'same-origin', 'Cross-Origin-Embedder-Policy': 'credentialless' },
};
for (const [port, headers] of Object.entries(MODES)) {
  createServer(async (req, res) => {
    const path = normalize(new URL(req.url, 'http://x').pathname).replace(/^\/+/, '') || 'index.html';
    try {
      const body = await readFile(join(ROOT, path));
      res.writeHead(200, { 'Content-Type': TYPES[extname(path)] ?? 'application/octet-stream', ...headers });
      res.end(body);
    } catch {
      res.writeHead(404); res.end();
    }
  }).listen(Number(port), '127.0.0.1');
}
console.log('serving', ROOT);
