// Actual compiled UI at adjacent AppKit-aligned viewport sizes. No native
// placement or original Paste comparison is implied by these measurements.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort().map(async n => [n, createHash('sha256').update(await readFile(new URL(n, root))).digest('hex')])));
const assets = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const results = [];
  const key = async (key, code, virtual) => {
    await page('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode: virtual });
    await page('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: virtual });
  };
  for (const scale of [1, 2]) {
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: scale, mobile: false });
    await page('Page.navigate', { url: `${fixture}/?fixture=visual` });
    await waitFor(`document.querySelectorAll('.clip-card').length===8`);
    await evaluate(`document.querySelector('.kind-text').click()`);
    const metrics = () => evaluate(`(() => {
      const card=document.querySelector('.kind-text'), title=card.querySelector('.card-title');
      const r=card.getBoundingClientRect(), s=getComputedStyle(card), t=getComputedStyle(title);
      return {width:r.width,height:r.height,font:t.fontSize,lineHeight:t.lineHeight,transform:s.transform,
        active:document.querySelector('.keyboard-active')?.dataset.dropClip,
        count:document.querySelectorAll('.clip-card').length};
    })()`);
    const before = await metrics();
    assert.equal(before.width, 196);
    for (const width of [1439, 1440, 1439]) {
      await key(' ', 'Space', 32);
      await waitFor(`document.querySelector('.preview-text') && window.previewFrameFixture.open`);
      await page('Emulation.setDeviceMetricsOverride', { width: width===1439?959:960, height: 759, deviceScaleFactor: scale, mobile: false });
      await key('Escape', 'Escape', 27);
      await waitFor(`!document.querySelector('.preview-overlay') && !window.previewFrameFixture.open && document.activeElement?.id==='history-results'`);
      await page('Emulation.setDeviceMetricsOverride', { width, height: 248, deviceScaleFactor: scale, mobile: false });
      await waitFor(`window.innerWidth===${width}`);
      const after = await metrics();
      assert.deepEqual(after, before, 'native grid changes must not resize cards, typography, selection or data');
      assert.equal(await evaluate(`document.documentElement.scrollWidth <= window.innerWidth`), true);
      results.push({ scale, width, card: after });
    }
    await screenshot(`window-grid-${scale}x.png`);
  }
  assert.deepEqual(await fingerprint(), assets);
  const report = { result: 'passed', browser, nativeEndToEnd: false, assetSha256: assets, results };
  await writeFile(join(artifacts, 'window-grid-report.json'), JSON.stringify(report, null, 2)+'\n');
  console.log(JSON.stringify(report));
});
