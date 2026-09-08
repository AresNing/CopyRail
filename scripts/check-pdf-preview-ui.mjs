// Actual compiled UI + PNGs from the production native renderer; synthetic IPC.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,root))).digest('hex')])));
const before = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 2, mobile: false });
  await page('Page.navigate', { url: `${fixture}/?fixture=pdf` });
  await waitFor(`document.querySelectorAll('.kind-pdf .card-media').length === 2 && Array.from(document.querySelectorAll('.kind-pdf .card-media')).every(i=>i.naturalWidth>0)`);
  const calls = command => evaluate(`window.pdfPreviewFixture.calls.filter(c=>c.command==='${command}')`);
  assert.equal((await calls('get_clip_preview')).length, 0, 'history prefetch must not load complete PDFs');
  assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('.kind-pdf .card-media')).map(i=>[i.naturalWidth,i.naturalHeight,i.alt,getComputedStyle(i).objectFit,i.draggable])`), [[400,600,'PDF 首页缩略图','contain',false],[600,400,'PDF 首页缩略图','contain',false]]);
  await screenshot('pdf-cards-light-2x.png');
  await page('Emulation.setEmulatedMedia', { features: [{name:'prefers-color-scheme',value:'dark'}] });
  await screenshot('pdf-cards-dark-2x.png');
  await evaluate(`document.querySelector('.kind-pdf').click()`);
  const key = async (key, code, virtual) => {
    await page('Input.dispatchKeyEvent',{type:'keyDown',key,code,windowsVirtualKeyCode:virtual});
    await page('Input.dispatchKeyEvent',{type:'keyUp',key,code,windowsVirtualKeyCode:virtual});
  };
  // Closing removes the dialog before the scheduled focus restoration and
  // native frame acknowledgement finish. Reopen only from the actual list.
  const closed = () => waitFor(`!document.querySelector('.preview-overlay') && !window.previewFrameFixture.open && document.activeElement?.id === 'history-results'`);
  await key(' ', 'Space', 32);
  await waitFor(`document.querySelector('.preview-pdf')`);
  assert.equal(await evaluate(`window.previewFrameFixture.open`), true, 'opening requests a real expanded native frame');
  // A browser cannot resize its own native host: simulate only the viewport
  // acknowledgement, and assert the production command independently above.
  await page('Emulation.setDeviceMetricsOverride', { width: 960, height: 760, deviceScaleFactor: 2, mobile: false });
  assert.ok(await evaluate(`document.querySelector('.preview-content').getBoundingClientRect().height >= 650`));
  assert.equal(await evaluate(`getComputedStyle(document.querySelector('.toolbar')).visibility`), 'hidden');
  await screenshot('pdf-expanded-reader-2x.png');
  const original = await readFile(new URL('../tmp/pdfs/synthetic-portrait.pdf', import.meta.url));
  const src = await evaluate(`document.querySelector('.preview-pdf').getAttribute('src')`);
  assert.deepEqual(Buffer.from(src.split(',')[1], 'base64'), original, 'full original two-page PDF remains available, not a first-page PNG');
  assert.equal(await evaluate(`document.querySelectorAll('.preview-actions button').length`), 0, 'PDF must not expose image rotation or OCR actions');
  await evaluate(`window.heldPdfFrame=document.querySelector('.preview-pdf'); new Promise(r=>setTimeout(r,2300))`);
  assert.equal(await evaluate(`window.heldPdfFrame===document.querySelector('.preview-pdf') && window.heldPdfFrame.isConnected`), true, 'history polling must not reload the PDF viewer or reset its page/scroll');
  assert.equal((await calls('get_clip_preview')).length, 1, 'only one full request per open');
  assert.equal((await calls('get_clip_thumbnail')).length, 3, 'failed thumbnail must not be retried on every poll');
  await key('Escape','Escape',27);
  await closed();
  await evaluate(`window.pdfPreviewFixture.failDocument=true`);
  await key(' ','Space',32);
  await waitFor(`document.querySelector('.preview-overlay')?.textContent.includes('可重试')`);
  await evaluate(`window.pdfPreviewFixture.failDocument=false; Array.from(document.querySelectorAll('.preview-overlay button')).find(b=>b.textContent.includes('重试')).click()`);
  await waitFor(`document.querySelector('.preview-pdf')`);
  await key('Escape','Escape',27);
  await closed();
  await evaluate(`window.pdfPreviewFixture.delayMs=1000`);
  await key(' ','Space',32);
  await waitFor(`document.querySelector('.preview-loading')`);
  await key('Escape','Escape',27);
  await closed();
  await evaluate(`new Promise(r=>setTimeout(r,1300))`);
  assert.equal(await evaluate(`document.querySelector('.preview-overlay')`), null, 'late IPC result must not reopen a closed PDF');
  await evaluate(`window.previewFrameFixture.delayMs=400; window.pdfPreviewFixture.delayMs=150`);
  await key(' ','Space',32);
  await waitFor(`window.previewFrameFixture.calls.at(-1) === true`);
  await evaluate(`window.dragEventFixture.emit('pasters-close-preview', null)`);
  await waitFor(`!document.querySelector('.preview-overlay') && window.previewFrameFixture.calls.at(-1) === false && !window.previewFrameFixture.open`);
  assert.equal(await evaluate(`document.activeElement?.id`), 'history-results', 'native close restores timeline focus');
  await evaluate(`window.previewFrameFixture.delayMs=0`);
  await evaluate(`window.pdfPreviewFixture.delayMs=150; window.sourceIconFixture.clips[0].title='rotated.pdf'; window.sourceIconFixture.clips[0].content_hash=Array(32).fill(88)`);
  await waitFor(`document.querySelector('.kind-pdf .card-media')?.naturalWidth === 600`);
  assert.equal((await calls('get_clip_thumbnail')).filter(c=>c.id.endsWith('000001')).length, 2, 'content revision change invalidates the cached thumbnail');
  assert.deepEqual(await fingerprint(), before, 'assets must remain frozen');
  const report = { result:'passed', browser, nativeEndToEnd:false, assetSha256:before, checks:[
    'native-rendered portrait/rotated first-page PNGs fit without crop',
    'full PDF is loaded only on open and preserves exact original bytes',
    'PDF frame survives history polls; failed thumbnail uses cooldown',
    'no image actions on PDF; failed full preview supports retry',
    'closing while pending cannot resurrect preview',
    'expanded layout requests are serialized; native Escape event closes and restores focus',
    'content hash change refreshes the card image rather than reusing stale bitmap',
  ]};
  await writeFile(join(artifacts,'pdf-preview-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
