// Headless Chrome: the non-threaded napi WASI build with no isolation headers, main thread and Worker.
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.launch({ executablePath: process.env.CHROME ?? '/usr/bin/google-chrome', headless: true, args: ['--no-sandbox'] });
for (const url of ['http://127.0.0.1:8702/nothreads.html', 'http://127.0.0.1:8702/nothreads.html?worker=1', 'http://127.0.0.1:8702/wb.html']) {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e.message).split('\n')[0]));
  await page.goto(url);
  let result;
  try {
    await page.waitForFunction(() => document.title === 'done', { timeout: 30000 });
    result = await page.evaluate(() => window.__result);
  } catch { result = { ok: false, stage: 'timeout' }; }
  console.log(JSON.stringify({ url, ...result, pageErrors: errors.slice(0, 2) }));
  await page.close();
}
await browser.close();
