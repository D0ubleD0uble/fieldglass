// Drive headless Chrome over the #1087 probe pages: napi WASI (main thread and Worker)
// and wasm-bindgen, under each header set that serve.mjs offers.
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.launch({ executablePath: process.env.CHROME ?? '/usr/bin/google-chrome', headless: true, args: ['--no-sandbox'] });
const cases = [];
for (const [port, label] of [[8701, 'COOP+COEP require-corp'], [8702, 'no headers'], [8703, 'COOP+COEP credentialless']]) {
  cases.push([label, `http://127.0.0.1:${port}/index.html`, 'napi-wasi main']);
  cases.push([label, `http://127.0.0.1:${port}/index.html?worker=1`, 'napi-wasi worker']);
  cases.push([label, `http://127.0.0.1:${port}/index.html?polyfill=1`, 'napi-wasi main + Buffer polyfill']);
  cases.push([label, `http://127.0.0.1:${port}/index.html?worker=1&polyfill=1`, 'napi-wasi worker + Buffer polyfill']);
  cases.push([label, `http://127.0.0.1:${port}/wb.html`, 'wasm-bindgen main']);
}
for (const [label, url, what] of cases) {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e.message).split('\n')[0]));
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text().split('\n')[0]); });
  await page.goto(url);
  let result;
  try {
    await page.waitForFunction(() => document.title === 'done', { timeout: 30000 });
    result = await page.evaluate(() => window.__result);
  } catch {
    result = { ok: false, stage: 'timeout' };
  }
  if (result && result.error) result.error = result.error.split('\n')[0];
  console.log(JSON.stringify({ headers: label, what, ...result, consoleErrors: errors.slice(0, 2) }));
  await page.close();
}
await browser.close();
