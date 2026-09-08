// Actual compiled WASM, synthetic IPC, private headless browser. No OS clipboard.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';

const dist = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all(
  (await readdir(dist)).filter(name => /\.(wasm|css|js|html)$/.test(name)).sort().map(async name =>
    [name, createHash('sha256').update(await readFile(new URL(name, dist))).digest('hex')]),
));
const assets = await fingerprint();

await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const observations = [];
  const enter = async () => {
    await page('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13, text: '\r', unmodifiedText: '\r' });
    await page('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 });
  };
  for (const compact of [true, false]) for (const theme of ['dark', 'light']) {
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: compact ? 148 : 248, deviceScaleFactor: 2, mobile: false });
    await page('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    await page('Page.navigate', { url: `${fixture}/?fixture=pinboards${compact ? '&compact=1' : ''}` });
    await waitFor(`document.querySelectorAll('.clip-card').length === 5 && document.querySelector('.status-button')`);
    await evaluate(`(() => {
      const invoke = window.__TAURI__.core.invoke;
      window.captureControlFixture = { state: { isolated: false, paused: false, pausedUntilMs: null, lastError: null, controlPending: null, revision: 10 }, pauses: 0, resumes: 0, restores: 0, reads: 0, delayedRead: false, staleStarted: false, staleReturned: false, failPause: false };
      window.__TAURI__.core.invoke = async (command, args) => {
        const f = window.captureControlFixture;
        if (command === 'capture_status') {
          f.reads++;
          const value = structuredClone(f.state);
          if (f.delayedRead) { f.delayedRead = false; f.staleStarted = true; await new Promise(r => setTimeout(r, 650)); f.staleReturned = true; }
          return value;
        }
        if (command === 'pause_capture' || command === 'resume_capture') {
          if (command === 'pause_capture') { f.pauses++; if (f.failPause) { f.failPause = false; throw new Error('Synthetic pause request rejected'); } } else f.resumes++;
          f.state.controlPending = command === 'pause_capture' ? 'pause' : 'resume';
          f.state.revision++;
          await new Promise(r => setTimeout(r, 80));
          return structuredClone(f.state);
        }
        if (command === 'restore_clip' || command === 'restore_clips') f.restores++;
        return invoke(command, args);
      };
    })()`);
    await waitFor(`window.captureControlFixture.reads >= 1`);
    await evaluate(`window.captureControlFixture.delayedRead = true`);
    await waitFor(`window.captureControlFixture.staleStarted`);
    await evaluate(`window.captureControlButton = document.querySelector('.status-button'); window.captureControlButton.focus()`);
    await enter();
    await waitFor(`document.querySelector('.status-button.pending')?.textContent.includes('正在暂停')`);
    await enter();
    await evaluate(`window.captureControlButton.click()`);
    await waitFor(`window.captureControlFixture.staleReturned`);
    const pending = await evaluate(`(() => {
      const b = document.querySelector('.status-button'); const r = b.getBoundingClientRect();
      return { text: b.textContent, busy: b.getAttribute('aria-busy'), disabled: b.getAttribute('aria-disabled'), sameButton: b === window.captureControlButton, focused: b === document.activeElement, pauses: window.captureControlFixture.pauses, restores: window.captureControlFixture.restores, left: r.left, right: r.right, bottom: r.bottom, width: innerWidth, height: innerHeight, count: document.querySelectorAll('.clip-card').length };
    })()`);
    assert.ok(pending.text.includes('正在暂停'), 'older status cannot erase the newer pending request');
    assert.equal(pending.busy, 'true'); assert.equal(pending.disabled, 'true');
    assert.equal(pending.sameButton, true); assert.equal(pending.focused, true);
    assert.equal(pending.pauses, 1); assert.equal(pending.restores, 0); assert.equal(pending.count, 5);
    assert.ok(pending.left >= 0 && pending.right <= pending.width && pending.bottom <= pending.height);
    await screenshot(`capture-control-pending-${compact ? 'compact' : 'expanded'}-${theme}.png`);
    await evaluate(`Object.assign(window.captureControlFixture.state, { paused: true, controlPending: null, revision: 12 })`);
    await waitFor(`document.querySelector('.status-button.paused')?.textContent.includes('点击恢复')`);
    assert.equal(await evaluate(`document.activeElement === window.captureControlButton && window.captureControlButton === document.querySelector('.status-button')`), true);
    await enter();
    await waitFor(`document.querySelector('.status-button.pending')?.textContent.includes('正在恢复')`);
    await enter();
    assert.equal(await evaluate(`window.captureControlFixture.resumes`), 1);
    await evaluate(`Object.assign(window.captureControlFixture.state, { paused: false, controlPending: null, revision: 14 })`);
    await waitFor(`document.querySelector('.status-button[aria-busy="false"]')?.textContent.includes('暂停 15 分钟')`);
    await evaluate(`Object.assign(window.captureControlFixture.state, { controlPending: 'preferences', revision: 15 })`);
    await waitFor(`document.querySelector('.status-button.pending')?.textContent.includes('正在应用设置')`);
    await evaluate(`Object.assign(window.captureControlFixture.state, { controlPending: null, revision: 16 }); window.captureControlFixture.failPause = true`);
    await waitFor(`document.querySelector('.status-button[aria-disabled="false"]')`);
    await enter();
    await waitFor(`window.captureControlFixture.pauses === 2 && document.querySelector('.status-button[aria-busy="false"]')`);
    assert.equal(await evaluate(`!!document.querySelector('.status-button.paused')`), false, 'rejected request must not claim paused');
    assert.equal(await evaluate(`window.captureControlFixture.restores`), 0, 'toolbar Return must not paste a card');
    await evaluate(`Object.assign(window.captureControlFixture.state, { isolated: true, paused: true, revision: 17 })`);
    await waitFor(`document.querySelector('.native-test-badge') && !document.querySelector('.status-button')`);
    await evaluate(`window.captureControlButton.click()`);
    assert.equal(await evaluate(`window.captureControlFixture.pauses`), 2, 'even a stale detached control cannot request real capture in isolation');
    observations.push({ compact, theme, pending, checks: ['pending is not confirmed pause', 'stale status rejected', 'keyboard focus and element retained', 'duplicate controls and accidental paste rejected', 'resume/settings confirmation', 'IPC failure is not success', 'isolation protected'] });
  }
  assert.deepEqual(await fingerprint(), assets);
  const report = { result: 'passed', browser, nativeEndToEnd: false, systemClipboardUsed: false, assetSha256: assets, observations };
  await writeFile(join(artifacts, 'capture-control-report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
});
