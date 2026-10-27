// Read-only diagnosis of each stage of the pipeline on a YouTube watch page.
import { launch, worker, hosts, wait } from './lib.mjs';
const VID = process.argv[2] || 'aqz-KE-bpKQ';
const b = await launch(); const w = await worker(b);
await w.evaluate(() => { self.LOG = []; chrome.webRequest.onHeadersReceived.addListener(d => {
  if (d.tabId < 0) return; const h = n => d.responseHeaders.find(x => x.name.toLowerCase() === n)?.value || '';
  LOG.push({ type: d.type, method: d.method, ct: h('content-type'), host: new URL(d.url).host, path: new URL(d.url).pathname });
}, { urls: ['<all_urls>'] }, ['responseHeaders']); });
const p = await b.newPage(); await p.setViewport({ width: 1280, height: 900 });
const s = await p.target().createCDPSession(); const ctx = [];
s.on('Runtime.executionContextCreated', e => ctx.push(e.context)); await s.send('Runtime.enable');
await p.goto(`https://www.youtube.com/watch?v=${VID}`, { waitUntil: 'domcontentloaded' }); await wait(12000);
const ev = async (expr, world) => (await s.send('Runtime.evaluate', { expression: expr, returnByValue: true, ...(world ? { contextId: world.id } : {}) })).result.value;

const cs = ctx.filter(c => c.auxData?.type === 'isolated' && c.name === 'ADM Browser Integration' && c.auxData.frameId === p.mainFrame()._id);
console.log('1. content script world in main frame:', cs.length);
console.log('   guard flag set:', cs.length ? await ev('window.__admContentLoaded === true', cs.at(-1)) : null);
console.log('2. <video> elements:', await ev(`[...document.querySelectorAll('video')].map(v=>{const r=v.getBoundingClientRect();return {src:v.currentSrc.slice(0,30),w:r.width|0,h:r.height|0,paused:v.paused,t:v.currentTime|0}})`));
const L = await w.evaluate(() => LOG);
const media = L.filter(x => /googlevideo/.test(x.host));
const group = {}; for (const x of media) { const k = `${x.type} ${x.method} ${x.path} [${x.ct}]`; group[k] = (group[k] || 0) + 1; }
console.log('3. googlevideo responses seen by webRequest:', group);
console.log('   any response with video/*, audio/*, mpegurl or dash+xml type:', L.filter(x => /^(video|audio)\/|mpegurl|dash\+xml/.test(x.ct)).map(x => x.host + x.path));
const tabId = await w.evaluate(async () => (await chrome.tabs.query({ url: '*://www.youtube.com/*' }))[0].id);
console.log('4. background media list:', JSON.stringify(await w.evaluate(id => chrome.storage.session.get([`media:${id}`, `hls:${id}`]), tabId)));
console.log('5. floating button hosts:', await hosts(p));
// What the page's own player data offers (main world, read-only).
console.log('6. player streamingData:', await ev(`(()=>{const r=document.querySelector('#movie_player')?.getPlayerResponse?.()||window.ytInitialPlayerResponse;const sd=r?.streamingData||{};
 const f=x=>({itag:x.itag,mime:(x.mimeType||'').split(';')[0],q:x.qualityLabel||x.audioQuality,url:!!x.url,cipher:!!(x.signatureCipher||x.cipher),drm:!!x.drmFamilies});
 return {playability:r?.playabilityStatus?.status,formats:(sd.formats||[]).map(f),adaptive:(sd.adaptiveFormats||[]).length,adaptiveWithUrl:(sd.adaptiveFormats||[]).filter(x=>x.url).length,
  hls:!!sd.hlsManifestUrl,dash:!!sd.dashManifestUrl,sabr:!!sd.serverAbrStreamingUrl}})()`));
await b.close();
