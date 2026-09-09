// Compiled production UI; synthetic IPC only. Native WebKit focus-ring
// painting remains a separate acceptance step, not implied by Chrome results.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort().map(async n => [n, createHash('sha256').update(await readFile(new URL(n, root))).digest('hex')])));
const assets = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const shots = [];
  const key = async (key, code, windowsVirtualKeyCode, modifiers = 0) => {
    await page('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode, modifiers });
    await page('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode, modifiers });
  };
  const focusStyle = () => evaluate(`(() => {
    const list = document.querySelector('#history-results');
    const active = list.querySelector('.clip-card.keyboard-active');
    const style = active && getComputedStyle(active);
    return { focused: document.activeElement?.id, keyboardFocus: list.matches(':focus-visible'),
      listOutline: getComputedStyle(list).outlineStyle,
      activeCount: list.querySelectorAll('.keyboard-active').length,
      activeId: active?.dataset.dropClip, outline: style?.outlineStyle,
      width: style?.outlineWidth, offset: style?.outlineOffset };
  })()`);
  for (const theme of ['light', 'dark']) {
    await page('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 2, mobile: false });
    await page('Page.navigate', { url: `${fixture}/?fixture=visual` });
    await waitFor(`document.querySelectorAll('.clip-card').length === 8`);
    await evaluate(`document.querySelector('.kind-text').click()`);
    await key(' ', 'Space', 32);
    await waitFor(`document.querySelector('.preview-text') && window.previewFrameFixture.open`);
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 608, deviceScaleFactor: 2, mobile: false });
    await key('Escape', 'Escape', 27);
    await waitFor(`!document.querySelector('.preview-overlay') && !window.previewFrameFixture.open && document.activeElement?.id === 'history-results'`);
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 2, mobile: false });
    const restored = await focusStyle();
    console.log(JSON.stringify({ theme, restored }));
    assert.equal(restored.keyboardFocus, true);
    assert.equal(restored.listOutline, 'none', 'the container must not leave an automatic WebKit ring outside cards');
    assert.equal(restored.activeCount, 1);
    assert.equal(restored.outline, 'solid');
    assert.equal(restored.width, '1px');
    assert.ok(parseFloat(restored.offset) <= -1, 'keyboard focus stays inside the active card');
    shots.push(await screenshot(`timeline-focus-restored-${theme}.png`));
    await key('ArrowRight', 'ArrowRight', 39);
    await waitFor(`document.querySelector('.keyboard-active')?.dataset.dropClip !== ${JSON.stringify(restored.activeId)}`);
    const next = await focusStyle();
    assert.equal(next.activeCount, 1);
    assert.equal(next.outline, 'solid');
    await key('ArrowRight', 'ArrowRight', 39, 8);
    await waitFor(`document.querySelector('.selection-count')?.textContent === '已选 2 项'`);
    assert.equal((await focusStyle()).activeCount, 1, 'multi-selection keeps exactly one keyboard destination');
    shots.push(await screenshot(`timeline-focus-multiple-${theme}.png`));
    await key('Tab', 'Tab', 9);
    // The product's existing Tab route switches results back to search.
    await waitFor(`document.activeElement?.id === 'history-search'`);
    assert.equal((await focusStyle()).outline, 'none', 'the card ring is removed when search owns focus');
    // Check button painting separately; this is not a claim that Tab reaches
    // every control (the full keyboard-navigation audit remains open).
    await evaluate(`document.querySelector('.stack-toggle').focus()`);
    await waitFor(`document.activeElement?.tagName === 'BUTTON'`);
    assert.equal(await evaluate(`getComputedStyle(document.activeElement).outlineStyle`), 'solid', 'button focus is retained');
    assert.equal((await focusStyle()).outline, 'none', 'the card ring is removed when a button owns focus');
    await evaluate(`window.sourceIconFixture.clips.splice(0)`);
    await waitFor(`document.querySelector('.empty-state') && !document.querySelector('.clip-card')`);
    await evaluate(`document.querySelector('#history-results').focus()`);
    await waitFor(`document.querySelector('#history-results').matches(':focus-visible')`);
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.empty-state')).outlineStyle`), 'solid', 'empty results still have a keyboard focus cue');
    assert.ok(await evaluate(`parseFloat(getComputedStyle(document.querySelector('.empty-state')).outlineOffset) <= -2`));
    shots.push(await screenshot(`timeline-focus-empty-${theme}.png`));
  }
  assert.deepEqual(await fingerprint(), assets);
  const report = { result: 'passed', browser, nativeEndToEnd: false, assetSha256: assets, checks: [
    'restored focus stays on results without a container automatic outline',
    'exactly one active-card inset focus cue follows arrows and multi-selection',
    'button focus and empty-results focus remain visible in both themes',
  ], screenshots: shots };
  await writeFile(join(artifacts, 'timeline-focus-report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
});
