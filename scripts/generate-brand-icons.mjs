// Generate all platform artwork from the same authored vector mark.
import { readFile, writeFile, copyFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const root = fileURLToPath(new URL('../', import.meta.url));
const icons = join(root, 'apps/desktop/src-tauri/icons');
const output = join(root, 'apps/desktop/src-tauri/generated-icons');
const mark = await readFile(join(root, 'apps/desktop/brand.svg'), 'utf8');
const app = mark.replace('fill="currentColor">', 'fill="currentColor">\n  <rect width="512" height="512" rx="112" fill="#141414"/>').replaceAll('currentColor', '#ffffff');
const tray = mark.replace('viewBox="0 0 512 512"', 'viewBox="80 80 352 352"').replaceAll('currentColor', '#000000');
await writeFile(join(icons, 'icon.svg'), app);
await writeFile(join(icons, 'tray.svg'), tray);
execFileSync('cargo', ['tauri', 'icon', join(icons, 'icon.svg'), '--output', output], { cwd: root, stdio: 'inherit' });
await copyFile(join(output, 'icon.png'), join(icons, 'icon.png'));
execFileSync('cargo', ['tauri', 'icon', join(icons, 'tray.svg'), '--output', join(icons, 'tray'), '--png', '36'], { cwd: root, stdio: 'inherit' });
