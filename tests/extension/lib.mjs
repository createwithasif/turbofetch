import puppeteer from 'puppeteer-core';
export const EXT = process.env.EXT || '/Users/asifwatto/idm clone/turbofetch/extension';
export const O = 'http://127.0.0.1:8799';
export const wait = (ms) => new Promise(r => setTimeout(r, ms));
export async function launch(withExt = true) {
  const b = await puppeteer.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true, pipe: true,
    enableExtensions: withExt ? [EXT] : true, args: ['--no-first-run', '--autoplay-policy=no-user-gesture-required'] });
  if (withExt) await wait(1200);
  return b;
}
export async function worker(b) { return (await b.waitForTarget(t => t.type() === 'service_worker')).worker(); }
// The button lives in a closed shadow root; its host is a fixed div on <html> (or the fullscreen element).
export const hosts = (p) => p.evaluate(() => [...document.querySelectorAll('div')].filter(e => e.style.zIndex === '2147483647')
  .map(e => { const r = e.getBoundingClientRect(); return { display: e.style.display, x: r.x, y: r.y }; }));
async function shadowNodes(p, cls) {
  const s = await p.target().createCDPSession();
  const { root } = await s.send('DOM.getDocument', { depth: -1, pierce: true });
  const out = [];
  const text = (n) => n.nodeType === 3 ? n.nodeValue : (n.children || []).map(text).join('');
  (function walk(n) {
    const a = n.attributes || []; const i = a.indexOf('class');
    if (i >= 0 && a[i + 1] === cls) out.push(n);
    for (const c of [...(n.children || []), ...(n.shadowRoots || [])]) walk(c);
  })(root);
  return Promise.all(out.map(async n => {
    let box = null; try { box = (await s.send('DOM.getBoxModel', { backendNodeId: n.backendNodeId })).model.content; } catch {}
    return { text: (n.children || []).map(c => text(c).trim()).filter(Boolean).join(' | '), box };
  }));
}
const center = (q) => [(q[0] + q[4]) / 2, (q[1] + q[5]) / 2];
export async function openMenu(p) {
  const [btn] = (await shadowNodes(p, 'btn')).filter(n => n.box);
  await p.mouse.click(...center(btn.box)); await wait(1500);
  return (await shadowNodes(p, 'item')).map(n => n.text).concat((await shadowNodes(p, 'msg')).filter(n => n.box).map(n => 'MSG: ' + n.text));
}
export async function clickItem(p, startsWith) {
  const it = (await shadowNodes(p, 'item')).find(n => n.text.startsWith(startsWith) && n.box);
  if (!it) throw new Error('no menu item ' + startsWith);
  await p.mouse.click(...center(it.box));
  // The result line shows for ~1.8 s; poll until it is no longer "Sending to ADM…".
  for (let i = 0; i < 40; i++) {
    await wait(150);
    const m = (await shadowNodes(p, 'msg')).filter(n => n.box).map(n => n.text);
    if (m.length && !m[0].startsWith('Sending')) return m;
  }
  return ['(no result shown)'];
}
