// Visible YouTube check: button after first load, menu contents, and after in-app (SPA) navigation.
import puppeteer from 'puppeteer-core';
import { EXT, wait, hosts, openMenu } from './lib.mjs';
const SHOTS = process.env.SHOTS;
const log = (...a) => console.log(new Date().toTimeString().slice(0, 8), ...a);
const b = await puppeteer.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: false, pipe: true,
  enableExtensions: [EXT], userDataDir: process.env.PROFILE, defaultViewport: null,
  args: ['--no-first-run', '--no-default-browser-check', '--window-size=1400,950', '--autoplay-policy=no-user-gesture-required'] });
const p = (await b.pages())[0];
const errors = []; p.on('pageerror', e => errors.push(e.message));
await p.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'domcontentloaded' });
await wait(12000);
const state = async () => ({ url: await p.evaluate(() => location.pathname + location.search), hosts: await hosts(p),
  video: await p.evaluate(() => { const v = document.querySelector('#movie_player video'); const r = v?.getBoundingClientRect(); return r && { x: r.x | 0, y: r.y | 0, w: r.width | 0, h: r.height | 0, src: v.currentSrc.slice(0, 12) }; }) });
log('first load:', JSON.stringify(await state()));
await p.screenshot({ path: `${SHOTS}/1-first-load.png` });
const items = await openMenu(p); log('menu after click:', JSON.stringify(items));
await p.screenshot({ path: `${SHOTS}/2-menu.png` });
await openMenu(p); // close it again

// In-app navigation: YouTube swaps the video without reloading the document.
const marker = await p.evaluate(() => (window.__admDocMarker = Math.random()));
const next = await p.evaluate(() => { const a = [...document.querySelectorAll('a[href^="/watch?v="]')].find(a => !a.href.includes('aqz-KE-bpKQ') && a.offsetParent); a?.scrollIntoView({ block: 'center' }); return a?.getAttribute('href'); });
log('clicking related video link:', next);
await wait(800);
await p.evaluate((h) => [...document.querySelectorAll(`a[href="${h}"]`)].find(a => a.offsetParent).click(), next);
await wait(10000);
log('same document (no full reload):', (await p.evaluate(() => window.__admDocMarker)) === marker);
log('after SPA navigation:', JSON.stringify(await state()));
await p.screenshot({ path: `${SHOTS}/3-after-spa.png` });
await p.goBack(); await wait(8000);
log('after Back:', JSON.stringify(await state()));
await p.goto('https://www.youtube.com/', { waitUntil: 'domcontentloaded' }); await wait(6000);
log('YouTube home (not a watch page):', JSON.stringify(await hosts(p)));
log('page errors:', JSON.stringify(errors.filter(e => /ADM|content\.js|chrome-extension/.test(e))));
await wait(3000); await b.close();
