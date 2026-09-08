// Offline classification of bounded native metadata; never drives an app.
// A successful report only covers settled classification backgrounds in the
// four-board, non-search fixture, not animations, gestures or pixel parity.
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function analyzeNativeTabTrace(text, theme) {
  if (!['light', 'dark'].includes(theme)) throw new Error('Explicit theme required');
  const failures = [], samples = [], sequences = [];
  const record = /^Native UI test tabs: sequence=(\d+) scheduled_ms=(\d+) scoped=(true|false) search=(true|false) native_visible=(Some\((?:true|false)\)|None) native_focused=(Some\((?:true|false)\)|None) app_hidden_active=(Some\(\((?:true|false), (?:true|false)\)\)|None) snapshot=(.*)$/;
  const bool = value => value === 'Some(true)' ? true : value === 'Some(false)' ? false : null;
  const near = (a, b) => Math.abs(a - b) < 0.005;
  const validTime = t => t === null || (Number.isFinite(t) && Math.abs(t) <= 1e12);
  if (!/^Native UI test profile:/m.test(text) || !/^Native UI test layout: compact=(true|false) pdf=false$/m.test(text)) failures.push('missing four-board isolated fixture identity');
  for (const [index, line] of text.split('\n').entries()) {
    if (!line.startsWith('Native UI test tabs:')) continue;
    const match = record.exec(line);
    if (!match) { failures.push(`line ${index + 1}: invalid or failed native sample`); continue; }
    try {
      if (match[8].length > 32768) throw new Error();
      const s = JSON.parse(match[8]);
      if (typeof s.visible !== 'boolean' || typeof s.focused !== 'boolean' || !Number.isFinite(s.now_ms) || !validTime(s.timeline_ms) || !Number.isInteger(s.tab_count) || s.tab_count < 0 || !Array.isArray(s.tabs) || s.tabs.length > 16 || s.tabs.length > s.tab_count) throw new Error();
      for (const [ordinal, t] of s.tabs.entries()) {
        if (t.ordinal !== ordinal || !['active', 'current', 'hovered'].every(k => typeof t[k] === 'boolean') || !Array.isArray(t.background) || t.background.length !== 4 || !t.background.every((n, i) => Number.isFinite(n) && n >= 0 && n <= (i === 3 ? 1 : 255)) || !Number.isInteger(t.animation_count) || !Array.isArray(t.animations) || t.animations.length > 8 || t.animations.length > t.animation_count) throw new Error();
        if (t.active !== t.current) failures.push(`line ${index + 1}: active and aria-current disagree`);
      }
      const active = s.tabs.filter(t => t.active).map(t => t.ordinal);
      const search = match[4] === 'true', scoped = match[3] === 'true';
      if (s.tab_count === 4 && (active.length !== (search ? 0 : 1) || (!search && (scoped ? active[0] === 0 : active[0] !== 0)))) failures.push(`line ${index + 1}: selected state contradicts query scope`);
      const nativeVisible = bool(match[5]), nativeFocused = bool(match[6]);
      const app = /Some\(\((true|false), (true|false)\)\)/.exec(match[7]);
      const appHidden = app ? app[1] === 'true' : null, appActive = app ? app[2] === 'true' : null;
      const alpha = theme === 'dark' ? 0.18 : 0.34, hoverAlpha = theme === 'dark' ? 0.06 : 0.14;
      const backgroundMatches = s.tabs.every(t => {
        const expected = t.active ? alpha : t.hovered ? hoverAlpha : 0;
        return near(t.background[3], expected) && (expected === 0 || t.background.slice(0, 3).every(n => near(n, 255)));
      });
      // Require both layers. AppKit focus alone does not prove WebKit's
      // document is visible, as the 50ras8 native counterexample demonstrates.
      const eligible = s.visible && s.focused && nativeVisible === true && nativeFocused === true && appHidden === false && appActive === true && s.tab_count === 4 && s.tabs.length === 4 && !search;
      samples.push({ line: index + 1, sequence: Number(match[1]), scheduledMs: Number(match[2]), scoped, search, documentVisible: s.visible, documentFocused: s.focused, nativeVisible, nativeFocused, appHidden, appActive, nowMs: s.now_ms, timelineMs: s.timeline_ms, active, backgroundMatches, animations: s.tabs.reduce((n, t) => n + t.animation_count, 0), eligible });
    } catch { failures.push(`line ${index + 1}: invalid bounded metadata`); }
  }
  const covered = new Set();
  for (const sequence of new Set(samples.map(s => s.sequence))) {
    const group = samples.filter(s => s.sequence === sequence);
    const first = group.find(s => s.scheduledMs === 0), final = group.find(s => s.scheduledMs === 800);
    const complete = group.length === 3 && [0, 200, 800].every(ms => group.filter(s => s.scheduledMs === ms).length === 1);
    const advanced = complete && first.timelineMs !== null && final.timelineMs !== null && final.timelineMs > first.timelineMs && final.nowMs > first.nowMs;
    const eligible = complete && group.every(s => s.eligible);
    if (eligible && (!advanced || !final.backgroundMatches || final.animations !== 0)) failures.push(`sequence ${sequence}: visible classification did not settle`);
    if (eligible && advanced && final.backgroundMatches && final.animations === 0 && group.every(s => JSON.stringify(s.active) === JSON.stringify(final.active))) covered.add(final.active[0]);
    sequences.push({ sequence, complete, eligible, timelineAdvanced: advanced, wallTimeAdvanced: Boolean(first && final && final.nowMs > first.nowMs), finalBackgroundMatches: final?.backgroundMatches ?? null });
  }
  const missing = [0, 1, 2, 3].filter(n => !covered.has(n));
  return { result: failures.length ? 'failed' : missing.length ? 'incomplete' : 'passed', scope: 'four-board settled backgrounds with visible and focused native/WebKit samples; not full native or pixel acceptance', theme, nativeEndToEnd: false, originalPastePixelComparison: false, summary: { sampleCount: samples.length, eligibleSampleCount: samples.filter(s => s.eligible).length, hiddenDocumentSamples: samples.filter(s => !s.documentVisible).length, coveredOrdinals: [...covered].sort(), missingOrdinals: missing }, sequences, samples, failures };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 4 || !['--dark', '--light'].includes(process.argv[3])) throw new Error('Usage: node scripts/check-native-tab-trace.mjs <isolated-session.log> --dark|--light');
  const report = analyzeNativeTabTrace(await readFile(process.argv[2], 'utf8'), process.argv[3].slice(2));
  console.log(JSON.stringify(report, null, 2));
  process.exitCode = report.result === 'passed' ? 0 : report.result === 'incomplete' ? 2 : 1;
}
