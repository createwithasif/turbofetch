// Browser download (Content-Disposition: attachment) -> extension takes it over -> ADM completes it.
import fs from 'fs';
import { launch, wait, O } from './lib.mjs';
const STORE = process.env.HOME + '/Library/Application Support/com.turbofetch.app/downloads.json';
const ours = () => JSON.parse(fs.readFileSync(STORE, 'utf8')).filter(d => d.url === O + '/owner-download');
const before = new Set(ours().map(d => d.id));
const b = await launch(); const p = await b.newPage();
await p.goto(O + '/empty'); await p.evaluate(u => { const a = document.createElement('a'); a.href = u; document.body.append(a); a.click(); }, O + '/owner-download');
let d; for (let i = 0; i < 30 && !(d = ours().find(x => !before.has(x.id) && x.status === 'completed')); i++) await wait(1000);
const left = await (await b.waitForTarget(t => t.type() === 'service_worker')).worker().then(w => w.evaluate(() => chrome.downloads.search({})));
await b.close();
if (!d) { console.log('FAIL: no completed ADM entry', ours().filter(x => !before.has(x.id))); process.exit(1); }
const f = `${d.dir}/${d.file_name}`;
console.log('PASS', JSON.stringify({ status: d.status, file: d.file_name, size: fs.statSync(f).size, expected: fs.statSync('media/dash/v480.mp4').size, chromeDownloadsLeft: left.length }));
