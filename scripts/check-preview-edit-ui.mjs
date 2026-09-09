// Exercise the compiled UI's native menu event routing. No native application
// or real clipboard is used; the fixture records the command boundary only.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';

const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root))
  .filter(name => /\.(wasm|css|js|html)$/.test(name)).sort()
  .map(async name => [name, createHash('sha256').update(await readFile(new URL(name, root))).digest('hex')])));
const before = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, browser, artifacts }) => {
  await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 1, mobile: false });
  const key = async (key, code, virtual) => {
    await page('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode: virtual });
    await page('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: virtual });
  };
  const emit = action => evaluate(`window.dragEventFixture.emit('pasters-edit-action', ${JSON.stringify(action)})`);
  const calls = () => evaluate(`window.previewEditFixture.calls`);
  const clear = () => evaluate(`window.previewEditFixture.calls.length=0`);
  const settled = () => evaluate(`new Promise(resolve=>setTimeout(resolve,100))`);
  const open = async selector => {
    await evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
    await key(' ', 'Space', 32);
    await waitFor(`document.querySelector('.preview-overlay') && window.previewFrameFixture.open`);
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 608, deviceScaleFactor: 1, mobile: false });
    await waitFor(`document.activeElement?.classList.contains('preview-overlay')`);
  };
  const close = async () => {
    await evaluate(`window.dragEventFixture.emit('pasters-close-preview', null)`);
    await waitFor(`!document.querySelector('.preview-overlay') && document.activeElement?.id==='history-results' && !window.previewFrameFixture.open`);
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 1, mobile: false });
  };
  await page('Page.navigate', { url: `${fixture}/?fixture=visual` });
  await waitFor(`document.querySelectorAll('.clip-card').length===8`);
  const history = await evaluate(`JSON.stringify(window.sourceIconFixture.clips)`);
  await open('.kind-text');
  await waitFor(`document.querySelector('.preview-text')`);
  for (const action of ['undo', 'redo', 'cut', 'paste']) await emit(action);
  await settled();
  assert.deepEqual(await calls(), [], 'read-only preview commands must never undo hidden history');
  await emit('select_all');
  await waitFor(`getSelection()?.toString()===document.querySelector('.preview-text').textContent`);
  assert.equal(await evaluate(`document.querySelectorAll('.clip-card.selected').length`), 1, 'preview select-all must not select hidden history');
  await emit('copy');
  await settled();
  assert.deepEqual(await calls(), [{ command: 'perform_native_text_action', args: { action: 'copy' } }]);
  await clear();
  await evaluate(`getSelection().removeAllRanges()`);
  await emit('copy');
  await settled();
  assert.deepEqual(await calls(), [{ command: 'restore_clip', args: { request: { clip_id: '10000000-0000-4000-8000-000000000001', plain_text: false, paste: false } } }]);
  await close();
  await clear();
  await emit('undo');
  await settled();
  assert.equal((await calls())[0]?.command, 'undo_last_delete', 'closing restores the ordinary history menu path');

  await open('.kind-image');
  await clear();
  for (const action of ['undo', 'select_all', 'cut', 'paste']) await emit(action);
  await settled();
  assert.deepEqual(await calls(), []);
  await emit('copy');
  await settled();
  assert.equal((await calls())[0]?.args.request.clip_id, '10000000-0000-4000-8000-000000000003');
  await close();
  assert.equal(await evaluate(`JSON.stringify(window.sourceIconFixture.clips)`), history, 'preview menu operations cannot mutate history');

  await page('Page.navigate', { url: `${fixture}/?fixture=pdf` });
  await waitFor(`document.querySelector('.kind-pdf .card-media')?.naturalWidth>0`);
  await open('.kind-pdf');
  await waitFor(`document.querySelector('.preview-pdf')`);
  await evaluate(`document.querySelector('.preview-pdf').focus()`);
  assert.equal(await evaluate(`document.activeElement.tagName`), 'IFRAME');
  for (const action of ['undo', 'redo', 'cut', 'copy', 'paste', 'select_all']) await emit(action);
  await settled();
  assert.deepEqual(await calls(), ['undo', 'redo', 'cut', 'copy', 'paste', 'select_all'].map(action => ({ command: 'perform_native_text_action', args: { action } })), 'focused PDF commands stay with its native responder');
  assert.equal(await evaluate(`document.querySelectorAll('.clip-card.selected').length`), 1);
  await close();
  assert.deepEqual(await fingerprint(), before);
  const report = { result: 'passed', browser, nativeEndToEnd: false, clipboardAccess: false, assetSha256: before, checks: [
    'text preview undo/cut/paste cannot affect hidden history',
    'text select-all is bounded to the preview body; selected-text copy uses native responder',
    'whole-item copy explicitly targets the previewed text/image clip',
    'focused document menu actions never fall through to history',
    'closing restores ordinary history menu routing; original fixture records stay unchanged',
  ] };
  await writeFile(join(artifacts, 'preview-edit-report.json'), JSON.stringify(report, null, 2)+'\n');
  console.log(JSON.stringify(report));
});
