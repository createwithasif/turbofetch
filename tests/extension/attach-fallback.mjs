// When ADM can't accept a download, the browser must still finish it itself.
import { launch, wait, O } from './lib.mjs';
const b = await launch();
const w = await (await b.waitForTarget(t => t.type() === 'service_worker')).worker();
// Simulate ADM being closed: the bridge is unreachable from this worker.
await w.evaluate(() => { const real = self.fetch; self.BRIDGE_CALLS = 0; self.fetch = (u, o) => String(u).startsWith('http://127.0.0.1:47321') ? (self.BRIDGE_CALLS++, Promise.reject(new TypeError('Failed to fetch'))) : real(u, o); });
const p = await b.newPage(); await p.goto(O + '/empty');
await p.evaluate(u => { const a = document.createElement('a'); a.href = u; document.body.append(a); a.click(); }, O + '/owner-download');
const completed = () => w.evaluate(() => chrome.downloads.search({ state: 'complete' }));
for (let i = 0; i < 30 && !(await completed()).length; i++) await wait(500);
const calls = await w.evaluate(() => BRIDGE_CALLS);
const items = await w.evaluate(() => chrome.downloads.search({}));
await b.close();
const ok = calls === 2 && items.filter(i => i.state === 'complete').length === 1;
console.log(ok ? 'PASS' : 'FAIL', JSON.stringify({ bridgeAttempts: calls, downloads: items.map(i => [i.state, i.filename, i.fileSize]) }));
process.exit(ok ? 0 : 1);
