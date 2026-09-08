// Strict validation of the new AppKit whole-point layout contract. The older
// fixed-width checker is retained unchanged for historical evidence. This
// checker never rounds measured positions or tolerates a center error.
import { readFile } from 'node:fs/promises';
if (process.argv.length !== 3 && !(process.argv.length === 4 && process.argv[3] === '--compact')) throw new Error('Usage: node scripts/check-native-point-grid.mjs <isolated-session.log> [--compact]');
const source = process.argv[2];
const compact = process.argv[3] === '--compact';
const dockHeight = compact ? 148 : 248;
const lines = (await readFile(source, 'utf8')).split('\n');
const number = '(-?\\d+(?:\\.\\d+)?(?:e[+-]?\\d+)?)';
const rect = new RegExp(`CGRect \\{ origin: CGPoint \\{ x: ${number}, y: ${number} \\}, size: CGSize \\{ width: ${number}, height: ${number} \\} \\}`, 'g');
const equal = (a, b) => a.length === b.length && a.every((value, i) => Math.abs(value - b[i]) < 1e-8);
const failures = [], frames = [], atomic = [];
let preview = false;
for (const [index, line] of lines.entries()) {
  const lineNumber = index + 1;
  if (line.startsWith('Native UI test atomic frame:')) {
    preview = line.includes('preview=true');
    const values = Array.from(line.matchAll(rect), m => m.slice(1).map(Number));
    if (values.length !== 3) { failures.push(`line ${lineNumber}: unparsed atomic update`); continue; }
    const [previous, requested, applied] = values;
    atomic.push({ line: lineNumber, preview, previous, requested, applied });
    if (!equal(requested, applied)) failures.push(`line ${lineNumber}: native frame differs from requested frame`);
  }
  if (line.startsWith('Native UI test AppKit geometry:')) {
    const values = Array.from(line.matchAll(rect), m => m.slice(1).map(Number));
    if (values.length < 2) { failures.push(`line ${lineNumber}: unparsed native geometry`); continue; }
    const [frame, visible] = values;
    const [x, y, w, h] = frame, [vx, vy, vw, vh] = visible;
    // Fixed acceptance fixture only; fail closed if it is too small or has
    // fractional work-area coordinates (not covered by this native run).
    if (!visible.every(Number.isInteger) || vw < 1464 || vh < 808) failures.push(`line ${lineNumber}: unsupported acceptance work area`);
    const ideal = preview ? [960, 760] : [1440, dockHeight];
    const centerError = x + w / 2 - vx - vw / 2;
    const verticalCenterError = y + h / 2 - vy - vh / 2;
    const loss = [ideal[0] - w, ideal[1] - h];
    frames.push({ line: lineNumber, mode: preview ? 'reader' : 'dock', frame, visible, centerError, verticalCenterError, viewportLossPoints: loss });
    if (!frame.every(Number.isInteger)) failures.push(`line ${lineNumber}: native edges are not on the whole-point grid`);
    if (Math.abs(centerError) > 1e-8) failures.push(`line ${lineNumber}: center error ${centerError} pt`);
    if (loss[0] < 0 || loss[0] > 1) failures.push(`line ${lineNumber}: width lost more than the one-point grid adjustment`);
    // Parity decides the exact width, not a range used to excuse bad layout.
    const expectedWidth = ideal[0] - ((vw - ideal[0]) % 2);
    if (w !== expectedWidth) failures.push(`line ${lineNumber}: wrong parity-adjusted width`);
    if (preview) {
      if (h !== ideal[1] - ((vh - ideal[1]) % 2) || Math.abs(verticalCenterError) > 1e-8) failures.push(`line ${lineNumber}: reader not exactly centered on both axes`);
    } else if (h !== dockHeight || Math.abs(y - vy - 12) > 1e-8) failures.push(`line ${lineNumber}: dock height or floating gap changed`);
  }
}
if (!lines.some(line => /^Native (PDF )?UI test profile:/.test(line))) failures.push('not an isolated acceptance profile');
if (compact && !lines.some(line => line.startsWith('Native UI test layout: compact=true'))) failures.push('missing explicit Compact fixture identity');
if (!lines.some(line => line.includes('grid=whole-point-inward'))) failures.push('missing explicit point-grid layout identity');
if (!frames.some(frame => frame.mode === 'reader') || !frames.some(frame => frame.mode === 'dock')) failures.push('missing reader or dock observations');
const restores = atomic.filter(update => !update.preview && update.previous[2] >= 959 && update.previous[2] <= 960 && update.previous[3] >= 759 && update.previous[3] <= 760);
if (restores.length < 2) failures.push('need at least two actual reader-to-dock restorations');
if (!lines.some(line => line.startsWith('Native UI test preview layout: open=false'))) failures.push('no completed preview close');
const report = { result: failures.length ? 'failed' : 'passed', source, originalPastePixelComparison: false,
  scope: `captured integral work area; fixed 1440x${dockHeight} / 960x760 preferences with inward whole-point edge alignment`,
  summary: { restores: restores.length, maxCenterErrorPoints: frames.length ? Math.max(...frames.map(f => Math.abs(f.centerError))) : null,
    requestedFrameMismatches: atomic.filter(update => !equal(update.requested, update.applied)).length }, frames, atomic, failures };
console.log(JSON.stringify(report, null, 2));
if (failures.length) process.exitCode = 1;
