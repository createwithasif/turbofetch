import fs from 'fs';
import { launch, openMenu, clickItem, wait, O } from './lib.mjs';
const STORE = process.env.HOME + '/Library/Application Support/com.turbofetch.app/downloads.json';
const ours = () => JSON.parse(fs.readFileSync(STORE, 'utf8')).filter(d => d.url.startsWith(O));
const before = new Set(ours().map(d => d.id));
const b = await launch();
const cases = (process.argv[2] || '/direct:Video,/hlsreal:360p,/dash:360p,/dashav:480p,/hlsbad:Stream').split(',').map(c => c.split(':'));
for (const [path, pick] of cases) {
  const p = await b.newPage(); await p.setViewport({ width: 1100, height: 800 });
  await p.goto(O + path); await wait(2500);
  const items = await openMenu(p);
  const msg = await clickItem(p, pick);
  console.log(`${path}: menu=${JSON.stringify(items)} picked=${pick} -> popup says ${JSON.stringify(msg)}`);
  await p.close();
}
await b.close();
// Watch the app's queue until our new downloads settle.
let seen = [];
for (let i = 0; i < 40; i++) {
  seen = ours().filter(d => !before.has(d.id));
  if (seen.length && seen.every(d => ['completed', 'failed', 'error'].includes(d.status))) break;
  await wait(1000);
}
for (const d of seen) {
  const f = d.dir + '/' + d.file_name;
  const size = fs.existsSync(f) ? fs.statSync(f).size : null;
  const leftovers = fs.readdirSync(d.dir).filter(n => n.startsWith('.' + d.file_name) || n === d.file_name + '.part');
  console.log(JSON.stringify({ status: d.status, error: d.error, url: d.url.replace(O, ''), file: d.file_name, size, parts: `${d.parts_done}/${d.parts_total}`, leftovers }));
}
