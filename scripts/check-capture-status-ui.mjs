// Compiled Rust/WASM with synthetic status only; no OS clipboard or native app.
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
const warning = '采集保存失败，1 条内容暂存在内存等待重试；退出会丢失这些缓存，故障期间的新复制可能遗漏。原因：database is locked';
const privacy = '隐私设置已变更，已丢弃 1 条未写入缓存，不会补录。';

await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const observations = [];
  for (const compact of [true, false]) for (const theme of ['dark', 'light']) {
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: compact ? 148 : 248, deviceScaleFactor: 2, mobile: false });
    await page('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    await page('Page.navigate', { url: `${fixture}/?fixture=pinboards${compact ? '&compact=1' : ''}` });
    await waitFor(`document.querySelectorAll('.clip-card').length === 5`);
    await evaluate(`(() => {
      const invoke = window.__TAURI__.core.invoke;
      window.captureStatusFixture = { reads: 0, status: { isolated: false, paused: false, pausedUntilMs: null, lastError: ${JSON.stringify(warning)}, pendingItems: 1 } };
      window.__TAURI__.core.invoke = async (command, args) => {
        if (command === 'capture_status') { window.captureStatusFixture.reads++; return structuredClone(window.captureStatusFixture.status); }
        return invoke(command, args);
      };
    })()`);
    await waitFor(`document.querySelector('.capture-error')?.textContent === ${JSON.stringify(warning)}`);
    await evaluate(`document.querySelector('#history-search').focus()`);
    await waitFor(`window.captureStatusFixture.reads >= 3`);
    const measured = await evaluate(`(() => {
      const banner = document.querySelector('.capture-error');
      const rect = banner.getBoundingClientRect();
      return { text: banner.textContent, role: banner.getAttribute('role'), x: rect.x, y: rect.y, right: rect.right, bottom: rect.bottom, toolbarBottom: document.querySelector('.toolbar').getBoundingClientRect().bottom, width: innerWidth, height: innerHeight, searchFocused: document.activeElement === document.querySelector('#history-search'), count: document.querySelectorAll('.clip-card').length };
    })()`);
    assert.equal(measured.text, warning, 'periodic status refresh must keep the failure visible');
    assert.equal(measured.role, 'status');
    assert.equal(measured.searchFocused, true, 'status refresh must not steal focus');
    assert.equal(measured.count, 5, 'status failure must not remove stored history');
    assert.ok(measured.x >= 0 && measured.right <= measured.width && measured.bottom <= measured.height, 'warning stays inside the panel');
    assert.ok(measured.y >= measured.toolbarBottom, 'warning does not cover search or board controls');
    await screenshot(`capture-status-${compact ? 'compact' : 'expanded'}-${theme}.png`);
    await evaluate(`Object.assign(window.captureStatusFixture.status, { paused: true, lastError: null, pendingItems: 0 })`);
    await waitFor(`!document.querySelector('.capture-error') && document.querySelector('.capture-status.paused')`);
    await evaluate(`window.captureStatusEntry=document.querySelector('.capture-status');window.captureStatusEntry.focus();window.pauseReads=window.captureStatusFixture.reads`);
    await waitFor(`window.captureStatusFixture.reads>=window.pauseReads+3`);
    assert.equal(await evaluate(`document.activeElement===window.captureStatusEntry && document.querySelector('.capture-status')===window.captureStatusEntry`),true,'polling must retain focus on the pause status entry');
    await evaluate(`Object.assign(window.captureStatusFixture.status, { paused: false, lastError: ${JSON.stringify(privacy)} })`);
    await waitFor(`!document.querySelector('.capture-status.paused') && document.querySelector('.capture-error')?.textContent === ${JSON.stringify(privacy)}`);
    await evaluate(`window.captureStatusFixture.status.lastError = null`);
    await waitFor(`!document.querySelector('.capture-error')`);
    observations.push({ compact, theme, measured, pausePrivacyRecovery: 'passed' });
  }
  assert.deepEqual(await fingerprint(), assets);
  const report = { result: 'passed', browser, nativeEndToEnd: false, systemClipboardUsed: false, assetSha256: assets, observations, limitations: ['Synthetic status: backend retry is covered separately by Rust tests', 'One representative storage error, not arbitrary-length error layout or VoiceOver speech'] };
  await writeFile(join(artifacts, 'capture-status-report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
});
