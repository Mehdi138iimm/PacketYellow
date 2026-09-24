// Downloads icons + fonts into ui/vendor so the app looks right even when CDNs are filtered/slow.
// Run once with internet: npm run vendor
import { mkdir, writeFile } from 'node:fs/promises';
const dir = new URL('../ui/vendor/', import.meta.url);
await mkdir(dir, { recursive: true });
const files = [
  ['lucide.min.js', 'https://unpkg.com/lucide@0.460.0/dist/umd/lucide.min.js'],
  ['Vazirmatn.woff2', 'https://cdn.jsdelivr.net/npm/vazirmatn@33.0.3/fonts/webfonts/Vazirmatn[wght].woff2'],
  ['JetBrainsMono.woff2', 'https://cdn.jsdelivr.net/npm/@fontsource-variable/jetbrains-mono/files/jetbrains-mono-latin-wght-normal.woff2'],
];
for (const [name, url] of files) {
  try {
    const r = await fetch(url); if (!r.ok) throw new Error(r.status);
    await writeFile(new URL(name, dir), Buffer.from(await r.arrayBuffer())); console.log('✓', name);
  } catch (e) { console.warn('✗', name, e.message, '(CDN fallback will be used)'); }
}
await writeFile(new URL('fonts.css', dir), `@font-face{font-family:Vazirmatn;src:url(Vazirmatn.woff2) format("woff2");font-weight:100 900;font-display:swap}
@font-face{font-family:"JetBrains Mono";src:url(JetBrainsMono.woff2) format("woff2");font-weight:100 800;font-display:swap}\n`);
console.log('done');
