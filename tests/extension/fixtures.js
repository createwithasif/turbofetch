// Local test media. Payload bytes are synthetic; the engine does not decode them.
const http = require('http');
const PORT = 8799, O = `http://127.0.0.1:${PORT}`;
const bytes = (n, b) => Buffer.alloc(n, b);
const files = {
  '/clip.mp4': ['video/mp4', bytes(2 << 20, 1)],
  '/clip2.mp4': ['video/mp4', bytes(3 << 20, 2)],
  '/clip.webm': ['video/webm', bytes(1 << 20, 3)],
  '/ad.mp4': ['video/mp4', bytes(100 << 10, 4)], // under the 300 KB floor
  '/hls/master.m3u8': ['application/vnd.apple.mpegurl',
    '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720\nv720.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nv360.m3u8\n'],
  '/dash/manifest.mpd': ['application/dash+xml', `<?xml version="1.0"?><MPD type="static" mediaPresentationDuration="PT8S"><Period>
<AdaptationSet mimeType="video/mp4">
<Representation id="v720" bandwidth="2000000" height="720"><BaseURL>v720.mp4</BaseURL></Representation>
<Representation id="v360" bandwidth="600000" height="360"><BaseURL>v360.mp4</BaseURL></Representation>
</AdaptationSet></Period></MPD>`],
  '/dash/av.mpd': ['application/dash+xml', `<?xml version="1.0"?><MPD type="static" mediaPresentationDuration="PT8S"><Period>
<AdaptationSet mimeType="video/mp4"><Representation id="v480" bandwidth="900000" height="480"><BaseURL>v360.mp4</BaseURL></Representation></AdaptationSet>
<AdaptationSet mimeType="audio/mp4"><Representation id="a1" bandwidth="128000"><BaseURL>a.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>`],
  '/dash/v720.mp4': ['video/mp4', bytes(700 << 10, 7)],
  '/dash/v360.mp4': ['video/mp4', bytes(400 << 10, 8)],
};
for (const q of ['720', '360']) {
  files[`/hls/v${q}.m3u8`] = ['application/vnd.apple.mpegurl',
    `#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n${[0, 1, 2].map(i => `#EXTINF:4,\n${q}_${i}.ts`).join('\n')}\n#EXT-X-ENDLIST\n`];
  for (const i of [0, 1, 2]) files[`/hls/${q}_${i}.ts`] = ['video/mp2t', bytes(q === '720' ? 200 << 10 : 80 << 10, 10 + i)];
}
const V = (src, extra = '') => `<video ${src ? `src="${src}"` : ''} autoplay muted ${extra} style="width:640px;height:360px;display:block;background:#000"></video>`;
// A player script that streams a playlist the way hls.js / dash.js do: fetch() + MediaSource.
const streamer = (u) => `${V('')}<script>fetch('${u}').then(r=>r.text()).then(t=>{const m=t.match(/^v\\d+\\.m3u8$/m);if(m)fetch(new URL(m[0],'${O}${u}'))});
const ms=new MediaSource();document.querySelector('video').src=URL.createObjectURL(ms);</script>`;
const pages = {
  '/direct': V('/clip.mp4'),
  '/webm': V('/clip.webm'),
  '/ad': V('/ad.mp4'),
  '/two': V('/clip.mp4') + V('/clip2.mp4', 'style="width:320px;height:180px"'),
  '/hls': streamer('/hls/master.m3u8'),
  '/dash': streamer('/dash/manifest.mpd'),
  '/dashav': streamer('/real/dash/av.mpd'),
  '/hlsreal': streamer('/real/hls2/master.m3u8'),
  '/hlsbad': streamer('/hls/v360.m3u8'),
  '/blob': `${V('')}<script>document.querySelector('video').src=URL.createObjectURL(new MediaSource())</script>`,
  '/empty': '<p>no media here</p>',
  '/tt': V('/clip.mp4'),
  '/late': `<div id=box></div><script>setTimeout(()=>box.innerHTML=${JSON.stringify(V('/clip.mp4'))},1500)</script>`,
  '/replace': `${V('/clip.mp4')}<script>setTimeout(()=>{document.querySelector('video').remove();document.body.insertAdjacentHTML('beforeend',${JSON.stringify(V('/clip2.mp4'))})},2000)</script>`,
  // pushState like a single-page site: first a video page, then a page without media.
  '/spa': `${V('/clip.mp4')}<script>window.go=()=>{history.pushState({},'', '/spa-next');document.querySelector('video').remove()}</script>`,
};
http.createServer((q, r) => {
  const path = q.url.split('?')[0];
  if (files[path]) { const [t, b] = files[path]; const buf = Buffer.from(b); r.writeHead(200, { 'content-type': t, 'content-length': buf.length, 'accept-ranges': 'bytes' }); return r.end(buf); }
  if (pages[path]) {
    const h = { 'content-type': 'text/html' };
    if (path === '/tt') h['content-security-policy'] = "require-trusted-types-for 'script'";
    r.writeHead(200, h); return r.end(`<!doctype html><title>ADM test ${path.slice(1)}</title><body style="margin:40px">${pages[path]}`);
  }
  // A server-side download, the way YouTube Studio hands an owner their own upload.
  if (path === '/owner-download') {
    const buf = require('fs').readFileSync(__dirname + '/media/dash/v480.mp4');
    r.writeHead(200, { 'content-type': 'video/mp4', 'content-length': buf.length, 'content-disposition': 'attachment; filename="ADM test owner download.mp4"' });
    return r.end(buf);
  }
  if (path.startsWith('/real/') && !path.includes('..')) {
    const f = __dirname + '/media/' + path.slice(6);
    if (require('fs').existsSync(f)) {
      const t = { m3u8: 'application/vnd.apple.mpegurl', ts: 'video/mp2t', mpd: 'application/dash+xml', mp4: 'video/mp4' }[f.split('.').pop()];
      const buf = require('fs').readFileSync(f); r.writeHead(200, { 'content-type': t, 'content-length': buf.length }); return r.end(buf);
    }
  }
  r.writeHead(404); r.end();
}).listen(PORT, '127.0.0.1');
