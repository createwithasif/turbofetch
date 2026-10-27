import { launch, worker, hosts, openMenu, wait, O, EXT } from './lib.mjs';
let pass = 0, fail = 0;
const check = (name, ok, info) => { ok ? pass++ : fail++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${info !== undefined ? '  ' + JSON.stringify(info) : ''}`); };
const b = await launch();
const w = await worker(b);
const visit = async (path, ms = 2500) => { const p = await b.newPage(); await p.setViewport({ width: 1100, height: 800 }); await p.goto(O + path); await wait(ms); return p; };
const shown = (h) => h.filter(x => x.display === 'block').length;

for (const [path, want] of [['/direct', ['Video | 2.0 MB']], ['/webm', ['Video | 1.0 MB']], ['/tt', ['Video | 2.0 MB']],
  ['/hls', ['720p | HLS', '360p | HLS']], ['/dash', ['720p | DASH', '360p | DASH']]]) {
  const p = await visit(path);
  const h = await hosts(p);
  check(`${path}: one button shown`, h.length === 1 && shown(h) === 1, h);
  const items = await openMenu(p);
  check(`${path}: menu lists real options`, JSON.stringify(items) === JSON.stringify(want), items);
  await p.close();
}
{ const p = await visit('/two'); const items = await openMenu(p);
  check('/two: both files listed, largest first', items[0] === 'Video | 3.0 MB' && items[1] === 'Video | 2.0 MB', items);
  const h = await hosts(p); check('/two: still one button', h.length === 1, h); await p.close(); }
for (const path of ['/ad', '/blob', '/empty']) {
  const p = await visit(path); const h = await hosts(p);
  check(`${path}: no button (nothing downloadable)`, shown(h) === 0, h); await p.close();
}
{ const p = await visit('/late', 4000); const h = await hosts(p);
  check('/late: player inserted after load gets a button', shown(h) === 1, h); await p.close(); }
{ const p = await visit('/replace', 4500); const h = await hosts(p); const items = await openMenu(p);
  check('/replace: replaced player keeps one button', h.length === 1 && shown(h) === 1, h);
  check('/replace: new source listed', items.includes('Video | 3.0 MB'), items); await p.close(); }
{ const p = await visit('/spa'); const before = shown(await hosts(p));
  await p.evaluate(() => go()); await wait(1500); const after = await hosts(p);
  check('/spa: button hides after pushState to a page without media', before === 1 && after.length === 0, { before, after });
  await p.goto(O + '/direct#t=10'); await wait(2000); await p.evaluate(() => location.hash = 't=20'); await wait(1200);
  check('hash change keeps the button', shown(await hosts(p)) === 1, await hosts(p)); await p.close(); }
{ const p = await visit('/direct'); await p.evaluate(() => document.querySelector('video').requestFullscreen()); await wait(1200);
  const inFs = await p.evaluate(() => [...document.fullscreenElement.querySelectorAll('div')].length + [...(document.fullscreenElement.children)].filter(e => e.style?.zIndex === '2147483647').length);
  check('fullscreen: button moves into the fullscreen element', inFs >= 1, inFs); await p.close(); }
{ const p = await visit('/direct'); const h = await hosts(p);
  // dismiss with the ×, then a new file appears: one fresh button, not two
  await p.evaluate(() => { const v = document.createElement('video'); v.src = '/clip2.mp4'; v.muted = v.autoplay = true; v.style.cssText = 'width:300px;height:200px'; document.body.append(v); });
  await wait(2000); const h2 = await hosts(p);
  check('more media on the same page does not duplicate the button', h.length === 1 && h2.length === 1, h2); await p.close(); }
await b.close();

// Tabs that were open before the extension was installed / reloaded.
{ const b2 = await launch(false); const p = await b2.newPage(); await p.goto(O + '/empty');
  let id = await b2.installExtension(EXT); await wait(1500);
  await p.evaluate(() => document.body.insertAdjacentHTML('beforeend', '<video src="/clip.mp4" autoplay muted style="width:640px;height:360px;display:block"></video>')); await wait(2500);
  check('tab open before install: button appears', shown(await hosts(p)) === 1, await hosts(p));
  await b2.uninstallExtension(id); id = await b2.installExtension(EXT); await wait(1500);
  await p.evaluate(() => document.body.insertAdjacentHTML('beforeend', '<video src="/clip2.mp4" autoplay muted style="width:640px;height:360px;display:block"></video>')); await wait(2500);
  const h = await hosts(p);
  check('after extension reload: exactly one live button', shown(h) === 1, h);
  await b2.close(); }
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
