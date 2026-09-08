import assert from 'node:assert/strict';
import { test } from 'node:test';
import { analyzeNativeTabTrace } from './check-native-tab-trace.mjs';

function fixture(change = () => {}) {
  const lines = ['Native UI test profile: synthetic', 'Native UI test layout: compact=true pdf=false'];
  for (let ordinal = 0; ordinal < 4; ordinal++) for (const delay of [0, 200, 800]) {
    const snapshot = { visible: true, focused: true, now_ms: ordinal * 1000 + delay, timeline_ms: ordinal * 1000 + delay, tab_count: 4, tabs: Array.from({ length: 4 }, (_, i) => ({ ordinal: i, active: i === ordinal, current: i === ordinal, hovered: false, background: i === ordinal ? [255, 255, 255, 0.18] : [0, 0, 0, 0], animation_count: 0, animations: [] })) };
    change(snapshot, ordinal, delay);
    lines.push(`Native UI test tabs: sequence=${ordinal + 1} scheduled_ms=${delay} scoped=${ordinal !== 0} search=false native_visible=Some(true) native_focused=Some(true) app_hidden_active=Some((false, true)) snapshot=${JSON.stringify(snapshot)}`);
  }
  return lines.join('\n');
}

test('complete visible four-board fixture passes only its narrow scope', () => {
  const report = analyzeNativeTabTrace(fixture(), 'dark');
  assert.equal(report.result, 'passed');
  assert.equal(report.nativeEndToEnd, false);
  assert.deepEqual(report.summary.coveredOrdinals, [0, 1, 2, 3]);
});
test('native focus cannot promote a hidden frozen document to acceptance', () => {
  const report = analyzeNativeTabTrace(fixture(s => { s.visible = false; s.timeline_ms = 121; }), 'dark');
  assert.equal(report.result, 'incomplete');
  assert.equal(report.summary.eligibleSampleCount, 0);
  assert.ok(report.sequences.every(s => s.wallTimeAdvanced && !s.timelineAdvanced));
});
test('unknown native state is not treated as foreground', () => {
  assert.equal(analyzeNativeTabTrace(fixture().replaceAll('native_focused=Some(true)', 'native_focused=None'), 'dark').result, 'incomplete');
});
test('visible frozen animation timeline fails', () => {
  assert.equal(analyzeNativeTabTrace(fixture(s => { s.timeline_ms = 121; }), 'dark').result, 'failed');
});
test('visible wrong pill background fails even with correct active class', () => {
  assert.equal(analyzeNativeTabTrace(fixture((s, ordinal) => { if (ordinal !== 0) s.tabs[0].background = [255, 255, 255, 0.18]; }), 'dark').result, 'failed');
});
test('active and accessible current disagreement fails', () => {
  assert.equal(analyzeNativeTabTrace(fixture(s => { s.tabs[0].current = !s.tabs[0].current; }), 'dark').result, 'failed');
});
test('missing, duplicate or failed samples cannot pass', () => {
  const lines = fixture().split('\n');
  assert.equal(analyzeNativeTabTrace(lines.slice(0, -1).join('\n'), 'dark').result, 'incomplete');
  assert.equal(analyzeNativeTabTrace([...lines, lines.at(-1)].join('\n'), 'dark').result, 'incomplete');
  assert.equal(analyzeNativeTabTrace([...lines, 'Native UI test tabs: invalid_snapshot=true'].join('\n'), 'dark').result, 'failed');
});
test('missing isolation identity, truncated tabs or bad values cannot pass', () => {
  assert.equal(analyzeNativeTabTrace(fixture().replace('Native UI test profile:', 'profile:'), 'dark').result, 'failed');
  // Truncation removes the selected fourth board too, contradicting scope.
  assert.equal(analyzeNativeTabTrace(fixture(s => { s.tabs = s.tabs.slice(0, 3); }), 'dark').result, 'failed');
  assert.equal(analyzeNativeTabTrace(fixture(s => { s.tabs[0].background[3] = 2; }), 'dark').result, 'failed');
});
