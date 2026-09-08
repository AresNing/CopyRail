// Real compiled UI and production-generated named system icons, synthetic IPC.
// Never starts the desktop service or reads the user's clipboard/history.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';

const assetRoot = new URL('../apps/desktop/dist/', import.meta.url);
async function fingerprint() {
  const names = (await readdir(assetRoot)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort();
  return Object.fromEntries(await Promise.all(names.map(async name => [name, createHash('sha256').update(await readFile(new URL(name, assetRoot))).digest('hex')])));
}
const assetsBefore = await fingerprint();
const expectedTextEdit = `data:image/png;base64,${(await readFile(new URL('../target/native-source-icons/textedit.png', import.meta.url))).toString('base64')}`;
const expectedFinder = `data:image/png;base64,${(await readFile(new URL('../target/native-source-icons/finder.png', import.meta.url))).toString('base64')}`;

await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const checks = [];
  const screenshots = [];
  await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 2, mobile: false });
  await page('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] });
  const load = async query => {
    await page('Page.navigate', { url: `${fixture}/?fixture=visual${query}` });
    await waitFor(`document.querySelectorAll('.clip-card').length === 8 && window.sourceIconFixture.calls.length > 0`);
  };
  await load('&icon_delay=700');
  await evaluate(`window.originalIconCard = document.querySelector('.clip-card')`);
  await waitFor(`document.querySelectorAll('.source-app-icon').length === 6 && Array.from(document.querySelectorAll('.source-app-icon')).every(el => el.complete && el.naturalWidth === 64)`);
  assert.equal(await evaluate(`document.querySelector('.source-app-icon').src`), expectedTextEdit);
  assert.equal(await evaluate(`document.querySelector('.clip-card:nth-child(3) .source-app-icon').src`), expectedFinder);
  assert.equal(await evaluate(`document.querySelector('.clip-card:nth-child(4) .source-badge').textContent.trim()`), 'M');
  assert.equal(await evaluate(`document.querySelector('.clip-card:nth-child(5) .source-badge').textContent.trim()`), 'B');
  assert.deepEqual(await evaluate(`window.sourceIconFixture.calls.map(ids => ids.length)`), [4]);
  assert.equal(await evaluate(`Array.from(document.querySelectorAll('.source-app-icon')).every(el => {
    const r=el.getBoundingClientRect(), badge=el.parentElement.getBoundingClientRect();
    return r.width===19 && r.height===19 && badge.width===19 && getComputedStyle(el).pointerEvents==='none' && el.draggable===false;
  })`), true);
  await evaluate(`window.originalStackButton = document.querySelector('.clip-card .stack-toggle'); window.originalStackButton.focus()`);
  await evaluate(`new Promise(resolve => setTimeout(resolve, 1400))`);
  assert.equal(await evaluate(`window.sourceIconFixture.calls.length`), 1, 'cached and missing apps must not cause per-poll requests');
  assert.equal(await evaluate(`window.originalIconCard === document.querySelector('.clip-card')`), true);
  assert.equal(await evaluate(`document.activeElement === window.originalStackButton`), true, 'background polling must preserve keyboard focus');
  screenshots.push(await screenshot('source-icons-dark-2x.png'));
  checks.push('native-generated 64px TextEdit/Finder PNG bytes reach 19pt badges under configured image CSP');
  checks.push('same app deduplicated; missing and invalid PNG fall back; no polling storm or card remount');

  await evaluate(`window.sourceIconFixture.clips[0].source = { bundle_identifier: 'com.apple.finder', display_name: 'Finder' }`);
  await waitFor(`document.querySelector('.source-badge').getAttribute('aria-label') === '来源：Finder'`);
  assert.equal(await evaluate(`document.querySelector('.source-app-icon').src`), expectedFinder);
  assert.equal(await evaluate(`window.originalIconCard === document.querySelector('.clip-card')`), true);
  checks.push('source identity and accessible label update on reused card without stale icon');

  await evaluate(`Object.assign(window.sourceIconFixture.clips[0], {title: 'Updated color', content_kind: 'color', searchable_text: '#ABCDEF'})`);
  await waitFor(`document.querySelector('.clip-card:first-child .card-title').textContent === 'Updated color' && document.querySelector('.clip-card:first-child .card-swatch')?.textContent === '#ABCDEF'`);
  assert.equal(await evaluate(`window.originalIconCard === document.querySelector('.clip-card') && window.originalIconCard.classList.contains('selected') && window.originalIconCard.classList.contains('kind-color') && document.activeElement === window.originalStackButton`), true);
  assert.equal(await evaluate(`document.querySelector('.clip-card:first-child .card-kind').textContent`), 'Color');
  checks.push('title/body/kind updates do not require replacing the card or losing selection/focus');

  await load('&icon_delay=1800');
  await evaluate(`window.sourceIconFixture.clips[0].source = { bundle_identifier: 'io.pasters.later-source', display_name: 'Later app' }`);
  await waitFor(`document.querySelector('.source-badge').getAttribute('aria-label') === '来源：Later app'`);
  await waitFor(`document.querySelector('.clip-card:nth-child(2) .source-app-icon')?.naturalWidth === 64`);
  assert.equal(await evaluate(`!!document.querySelector('.clip-card:first-child .source-app-icon')`), false);
  assert.equal(await evaluate(`document.querySelector('.source-badge').textContent.trim()`), 'L');
  checks.push('late icon response for previous source cannot paint a new source');

  await load('&icon_fail=1');
  await evaluate(`new Promise(resolve => setTimeout(resolve, 1400))`);
  assert.equal(await evaluate(`document.querySelectorAll('.clip-card').length === 8 && !document.querySelector('.source-app-icon') && !document.querySelector('.error-banner')`), true);
  assert.equal(await evaluate(`window.sourceIconFixture.calls.length`), 1);
  checks.push('backend icon failure leaves clipboard cards usable and backs off');
  assert.deepEqual(await fingerprint(), assetsBefore);
  const report = { result: 'passed', browser, nativeWindowEndToEnd: false, originalPixelDiff: null, imageCspFromTauriConfig: true, assetSha256: assetsBefore, checks, screenshots };
  await writeFile(join(artifacts, 'source-icons-report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
});
