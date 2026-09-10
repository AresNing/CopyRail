import { copyFileSync, mkdirSync, readFileSync, readdirSync, lstatSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

// Publish an explicit allowlist: never upload the repository, clipboard data or local deliveries.
const root = fileURLToPath(new URL('../', import.meta.url));
const destination = resolve(root, 'target/pages/CopyRail');
const assets = [
  ['site/index.html', 'index.html'],
  ['site/style.css', 'style.css'],
  ['apps/desktop/src-tauri/icons/icon.svg', 'assets/icon.svg'],
  ['docs/images/clipboard-dark.png', 'assets/clipboard-dark.png'],
  ['docs/images/preview-dark.png', 'assets/preview-dark.png'],
];
mkdirSync(resolve(destination, 'assets'), { recursive: true });
for (const [source, target] of assets) copyFileSync(resolve(root, source), resolve(destination, target));
writeFileSync(resolve(destination, '.nojekyll'), '');
writeFileSync(resolve(destination, 'sitemap.xml'), '<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>https://aresning.github.io/CopyRail/</loc></url></urlset>\n');

const allowed = new Set([...assets.map(([, target]) => target), '.nojekyll', 'sitemap.xml']);
for (const entry of readdirSync(destination, { recursive: true, withFileTypes: true })) {
  const path = resolve(entry.parentPath, entry.name);
  if (entry.isDirectory()) continue;
  const relative = path.slice(destination.length + 1).replaceAll('\\', '/');
  if (!allowed.has(relative) || !lstatSync(path).isFile()) throw new Error(`Unexpected publish file: ${relative}`);
}

// Check local URLs against the exact GitHub Pages project path before publishing.
const html = readFileSync(resolve(destination, 'index.html'), 'utf8');
const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
for (const [, reference] of html.matchAll(/(?:href|src)="([^"]+)"/g)) {
  if (reference.startsWith('#')) {
    if (!ids.has(reference.slice(1))) throw new Error(`Missing section: ${reference}`);
  } else if (!reference.startsWith('https://') && reference !== './') {
    if (reference.startsWith('/') || reference.includes('..')) throw new Error(`Non-portable URL: ${reference}`);
    readFileSync(resolve(destination, reference));
  }
}
if (!html.includes('https://aresning.github.io/CopyRail/')) throw new Error('Missing canonical URL');
console.log('CopyRail landing page prepared and local links verified: target/pages/CopyRail');
