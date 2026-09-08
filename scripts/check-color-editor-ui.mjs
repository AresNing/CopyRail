// Compiled editor validation + synthetic IPC. Backend color normalization and
// raw capture fidelity are independently covered by Rust storage tests.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const root = new URL('../apps/desktop/dist/', import.meta.url);
async function fingerprint() {
  const names = (await readdir(root)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort();
  return Object.fromEntries(await Promise.all(names.map(async n => [n, createHash('sha256').update(await readFile(new URL(n, root))).digest('hex')])));
}
const assets = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 2, mobile: false });
  await page('Page.navigate', { url: `${fixture}/?fixture=visual` });
  await waitFor(`document.querySelectorAll('.clip-card').length === 8`);
  await evaluate(`(() => {
    window.colorWrites = [];
    const invoke = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      if (!['create_textual_item','update_textual_item'].includes(command)) return invoke(command, args);
      const request = args.request;
      window.colorWrites.push({command, request});
      const clips = window.sourceIconFixture.clips;
      const normalized = '#' + request.value.replace(/^#/, '').toLowerCase();
      const item = {...structuredClone(clips[0]), id: request.clip_id ?? '10000000-0000-4000-8000-000000000130', content_kind: request.kind, title: request.title ?? normalized, searchable_text: normalized};
      const index = clips.findIndex(c => c.id === item.id);
      if (index < 0) clips.unshift(item); else clips[index] = item;
      return structuredClone(item);
    };
    document.querySelector('.clip-card:nth-child(4)').click();
    document.querySelector('[title="编辑选中内容"]').click();
  })()`);
  await waitFor(`document.querySelector('.color-edit-row input[type=text]')?.value === '58aD97'`);
  assert.equal(await evaluate(`document.querySelector('.color-edit-row input[type=color]').value.toLowerCase()`), '#58ad97');
  const fill = async value => evaluate(`(() => { const input=document.querySelector('.color-edit-row input[type=text]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,${JSON.stringify(value)}); input.dispatchEvent(new Event('input',{bubbles:true})); })()`);
  await fill('235442');
  await evaluate(`document.querySelector('.save-content').click()`);
  await waitFor(`document.querySelector('.error-banner')?.textContent.includes('六位色值')`);
  assert.equal(await evaluate(`window.colorWrites.length`), 0);
  await fill('58aD97');
  await evaluate(`document.querySelector('.save-content').click()`);
  await waitFor(`!document.querySelector('.content-editor') && window.colorWrites.length === 1`);
  assert.equal(await evaluate(`window.colorWrites[0].command`), 'update_textual_item');
  assert.equal(await evaluate(`window.colorWrites[0].request.value`), '58aD97');
  await evaluate(`document.querySelector('[title="新建文本、链接或颜色"]').click()`);
  await waitFor(`document.querySelector('.content-editor textarea')`);
  await evaluate(`(() => {const input=document.querySelector('.content-editor textarea'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(input,'a1B2c3'); input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await evaluate(`Array.from(document.querySelectorAll('.content-editor button')).find(b=>b.textContent.trim()==='颜色').click()`);
  await waitFor(`document.querySelector('.color-edit-row input[type=text]')?.value === 'a1B2c3'`);
  await evaluate(`document.querySelector('.save-content').click()`);
  await waitFor(`!document.querySelector('.content-editor') && window.colorWrites.length === 2 && Array.from(document.querySelectorAll('.card-swatch')).some(el=>el.textContent==='#A1B2C3')`);
  assert.equal(await evaluate(`window.colorWrites[1].command`), 'create_textual_item');
  assert.equal(await evaluate(`window.colorWrites[1].request.value`), 'a1B2c3');
  const image = await screenshot('color-editor-saved-2x.png');
  assert.deepEqual(await fingerprint(), assets);
  const report = {result:'passed', browser, nativeEndToEnd:false, assetSha256:assets, checks:['existing bare hex preserved when editor opens','color well uses canonical RGB','numeric-only value rejected before IPC','switching from text to color keeps a valid bare code','bare hex edit and create reach the correct commands unchanged','returned color is rendered and editor closes'], screenshots:[image]};
  await writeFile(join(artifacts,'color-editor-report.json'), JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
