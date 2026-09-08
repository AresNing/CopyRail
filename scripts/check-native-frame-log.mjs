// Read-only acceptance of an explicitly captured isolated native session.
// Checks real AppKit frame values, not model geometry or browser dimensions.
import { readFile } from 'node:fs/promises';

if (process.argv.length !== 3) throw new Error('Usage: node scripts/check-native-frame-log.mjs <isolated-session.log>');
const source = process.argv[2];
const lines = (await readFile(source, 'utf8')).split('\n');
const failures = [];
const frames = [];
const atomic = [];
const number = '(-?\\d+(?:\\.\\d+)?(?:e[+-]?\\d+)?)';
const rect = new RegExp(`CGRect \\{ origin: CGPoint \\{ x: ${number}, y: ${number} \\}, size: CGSize \\{ width: ${number}, height: ${number} \\} \\}`, 'g');
const equal = (a, b) => a.length === b.length && a.every((n, i) => Math.abs(n - b[i]) < 1e-8);
for (const [index, line] of lines.entries()) {
  if (line.startsWith('Native UI test AppKit geometry:')) {
    const values = Array.from(line.matchAll(rect), m => m.slice(1).map(Number));
    if (values.length < 2) { failures.push(`line ${index + 1}: unparsed native frame`); continue; }
    const [frame, visible] = values;
    const [x, y, w, h] = frame;
    const [vx, vy, vw, vh] = visible;
    const centerError = x + w / 2 - vx - vw / 2;
    const mode = w === 960 && h === 760 ? 'reader' : (w === 1440 && [148, 248].includes(h) ? 'dock' : 'unexpected');
    frames.push({ line: index + 1, mode, frame, visible, centerError });
    if (Math.abs(centerError) > 1e-8) failures.push(`line ${index + 1}: center error ${centerError} pt`);
    if (mode === 'dock' && Math.abs(y - vy - 12) > 1e-8) failures.push(`line ${index + 1}: dock bottom gap ${y - vy} pt`);
    if (mode === 'reader' && Math.abs(y + h / 2 - vy - vh / 2) > 1e-8) failures.push(`line ${index + 1}: reader vertical center error`);
    if (mode === 'unexpected') failures.push(`line ${index + 1}: unexpected size for the fixed acceptance scenario`);
  }
  if (line.startsWith('Native UI test atomic frame:')) {
    const values = Array.from(line.matchAll(rect), m => m.slice(1).map(Number));
    if (values.length !== 3) { failures.push(`line ${index + 1}: unparsed atomic update`); continue; }
    const [previous, requested, applied] = values;
    atomic.push({ line: index + 1, previous, requested, applied });
    if (!equal(requested, applied)) failures.push(`line ${index + 1}: AppKit frame differs from requested frame`);
  }
}
if (!lines.some(line => line.startsWith('Native PDF UI test profile:'))) failures.push('not the isolated PDF acceptance scenario');
if (!frames.some(frame => frame.mode === 'reader')) failures.push('no observed reader frame');
if (!frames.some(frame => frame.mode === 'dock')) failures.push('no observed dock frame');
const restores = atomic.filter(update => equal(update.previous.slice(2), [960, 760]) && equal(update.applied.slice(2), [1440, 248]));
if (!restores.length) failures.push('no atomic reader-to-dock transition');
if (!lines.some(line => line.startsWith('Native UI test preview layout: open=false'))) failures.push('no completed preview close');
const summary = {
  atomicReaderToDockUpdates: restores.length,
  misplacedDockFrames: frames.filter(({ mode, frame, visible }) => mode === 'dock' && Math.abs(frame[1] - visible[1] - 12) > 1e-8).length,
  maximumAbsoluteCenterErrorPoints: frames.length ? Math.max(...frames.map(frame => Math.abs(frame.centerError))) : null,
  requestedFrameMismatches: atomic.filter(update => !equal(update.requested, update.applied)).length,
};
console.log(JSON.stringify({ result: failures.length ? 'failed' : 'passed', source, originalPastePixelComparison: false, nativeScope: 'fixed 1440x248 dock / 960x760 reader on the captured screen only', summary, frames, atomic, failures }, null, 2));
if (failures.length) process.exitCode = 1;
