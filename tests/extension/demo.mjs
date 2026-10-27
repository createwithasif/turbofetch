// Visible demonstration in a separate Chrome test profile (not the user's own profile).
import fs from 'fs';
import { execFileSync } from 'child_process';
import puppeteer from 'puppeteer-core';
import { EXT, wait, hosts, openMenu, clickItem } from './lib.mjs';
const PROFILE = process.env.PROFILE;
const STORE = process.env.HOME + '/Library/Application Support/com.turbofetch.app/downloads.json';
const VIDEO = 'https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_5MB.mp4';
const log = (...a) => console.log(new Date().toTimeString().slice(0, 8), ...a);

const b = await puppeteer.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: false,
  pipe: true, enableExtensions: [EXT], userDataDir: PROFILE, defaultViewport: null,
  args: ['--no-first-run', '--no-default-browser-check', '--window-size=1280,900', '--autoplay-policy=no-user-gesture-required'] });
const sw = await b.waitForTarget(t => t.type() === 'service_worker' && t.url().endsWith('/background.js'));
const w = await sw.worker();
log('STEP 1 extension loaded:', await w.evaluate(() => { const m = chrome.runtime.getManifest(); return `${m.name} ${m.version} id=${chrome.runtime.id}`; }));
// Show the user the extensions page with the extension enabled.
const [first] = await b.pages();
await first.goto('chrome://extensions'); await wait(4000);

log('STEP 2 opening YouTube watch page (Big Buck Bunny)');
const yt = await b.newPage();
const s = await yt.target().createCDPSession(); const ctx = [];
s.on('Runtime.executionContextCreated', e => ctx.push(e.context)); await s.send('Runtime.enable');
await w.evaluate(() => { self.YT = []; chrome.webRequest.onHeadersReceived.addListener(d => { if (/googlevideo/.test(d.url)) YT.push(`${d.type} ${d.method} ${new URL(d.url).pathname} [${d.responseHeaders.find(h => h.name.toLowerCase() === 'content-type')?.value || ''}]`); }, { urls: ['<all_urls>'] }, ['responseHeaders']); });
await yt.goto('https://www.youtube.com/watch?v=aqz-KE-bpKQ', { waitUntil: 'domcontentloaded' });
await wait(15000);
const ev = async (expr) => (await s.send('Runtime.evaluate', { expression: expr, returnByValue: true })).result.value;
log('STEP 3 YouTube findings:');
log('  content script injected:', ctx.some(c => c.name === 'ADM Browser Integration'));
log('  <video>:', JSON.stringify(await ev(`[...document.querySelectorAll('video')].map(v=>({src:v.currentSrc.slice(0,35),w:v.clientWidth,h:v.clientHeight,playing:!v.paused}))`)));
const counts = {}; for (const x of await w.evaluate(() => YT)) counts[x] = (counts[x] || 0) + 1;
log('  googlevideo network responses:', JSON.stringify(counts));
log('  player streamingData:', JSON.stringify(await ev(`(()=>{const r=document.querySelector('#movie_player')?.getPlayerResponse?.()||window.ytInitialPlayerResponse;const sd=r?.streamingData||{};return {playability:r?.playabilityStatus?.status,progressive:(sd.formats||[]).length,adaptive:(sd.adaptiveFormats||[]).length,adaptiveWithDirectUrl:(sd.adaptiveFormats||[]).filter(f=>f.url).length,hls:!!sd.hlsManifestUrl,dash:!!sd.dashManifestUrl,sabr:!!sd.serverAbrStreamingUrl}})()`)));
log('  media count in extension:', await w.evaluate(async () => { const [t] = await chrome.tabs.query({ url: '*://www.youtube.com/*' }); const k = [`media:${t.id}`, `hls:${t.id}`]; const v = await chrome.storage.session.get(k); return (v[k[0]] || []).length + (v[k[1]] || []).length; }));
log('  floating button on YouTube:', JSON.stringify(await hosts(yt)));
await wait(5000);

log('STEP 4 opening authorized public test video:', VIDEO);
const before = new Set(JSON.parse(fs.readFileSync(STORE, 'utf8')).map(d => d.id));
const p = await b.newPage();
await p.goto(VIDEO); await wait(4000);
log('  floating button:', JSON.stringify(await hosts(p)));
log('STEP 5 clicking the real "Download this video" button');
const items = await openMenu(p); log('  menu shows:', JSON.stringify(items)); await wait(2500);
log('STEP 6 clicking menu item', JSON.stringify(items[0]));
log('  popup result:', JSON.stringify(await clickItem(p, items[0].split(' | ')[0])));

let d, last = '';
for (let i = 0; i < 120; i++) {
  d = JSON.parse(fs.readFileSync(STORE, 'utf8')).find(x => !before.has(x.id));
  if (d) {
    const f = `${d.dir}/${d.file_name}`; const part = fs.existsSync(f + '.part') ? fs.statSync(f + '.part').size : null;
    const line = `queue: status=${d.status} file="${d.file_name}" total=${d.total} on-disk .part=${part} final=${fs.existsSync(f) ? fs.statSync(f).size : null}`;
    if (line !== last) log('  ' + (last = line));
    if (d.status === 'completed' || d.status === 'error') break;
  }
  await wait(250);
}
log('STEP 7 verify output');
const f = `${d.dir}/${d.file_name}`;
log('  final file size:', fs.statSync(f).size, '(server content-length 5229245)');
log('  ffprobe:', execFileSync('/opt/homebrew/opt/ffmpeg/bin/ffprobe', ['-v', 'error', '-show_entries', 'stream=codec_type,codec_name,width,height:format=duration', '-of', 'compact', f]).toString().trim().replace(/\n/g, ' ; '));
log('  leftover temp files:', JSON.stringify(fs.readdirSync(d.dir).filter(n => n === d.file_name + '.part' || n === `.${d.file_name}.parts`)));
log('Keeping the browser open 25 s so you can look at it...');
await wait(25000);
await b.close();
