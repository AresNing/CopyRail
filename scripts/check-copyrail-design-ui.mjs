// CopyRail layout, palette and empty-state actions on compiled WASM with synthetic IPC.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort().map(async n => [n, createHash('sha256').update(await readFile(new URL(n, root))).digest('hex')])));
const assets = await fingerprint();
await withCompiledUiTest(async ({page,evaluate,waitFor,fixture,screenshot,artifacts,browser}) => {
  const checks = [];
  for (const theme of ['light','dark']) {
    await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:theme}]});
    for (const compact of [false,true]) for (const width of [900,1100,1440]) {
      await page('Emulation.setDeviceMetricsOverride',{width,height:compact?148:248,deviceScaleFactor:1,mobile:false});
      await page('Page.navigate',{url:fixture+'/?fixture=visual'+(compact?'&compact=1':'')});
      await waitFor(`document.querySelectorAll('.clip-card').length===8 && document.querySelector('.card-media')?.naturalWidth===320`);
      const layout = await evaluate(`(() => {
        const rect=s=>document.querySelector(s).getBoundingClientRect();
        const b=rect('.rail-brand'),s=rect('.search-wrap'),a=rect('.toolbar-actions'),n=rect('.pinboards'),t=rect('.toolbar'),c=rect('.clip-card');
        const controls=[...document.querySelectorAll('.toolbar-actions > *')].filter(e=>getComputedStyle(e).display!=='none');
        return {toolbarFits:a.right<=innerWidth && b.left>=0 && s.right<=a.left && document.querySelector('.toolbar-actions').scrollWidth<=document.querySelector('.toolbar-actions').clientWidth+1,
          actionsFit:controls.every(e=>{const r=e.getBoundingClientRect();return r.right<=innerWidth && r.left>=s.right;}),
          separateRows:n.top>=s.bottom, cardsFit:c.top>=t.bottom && c.bottom<=innerHeight,
          contentsFit:[...document.querySelectorAll('.clip-card')].every(e=>e.scrollHeight<=e.clientHeight+1 && e.scrollWidth<=e.clientWidth+1),
          sourceVisible:getComputedStyle(document.querySelector('.clip-card footer')).display!=='none'};
      })()`);
      assert.ok(layout.toolbarFits && layout.actionsFit && layout.cardsFit && layout.contentsFit,JSON.stringify({theme,compact,width,layout}));
      assert.equal(layout.separateRows,!compact);
      assert.equal(layout.sourceVisible,!compact);
      assert.deepEqual(await evaluate(`[...document.querySelectorAll('.card-kind')].map(e=>getComputedStyle(e).backgroundColor)`),['rgb(237, 194, 77)','rgb(35, 115, 195)','rgb(201, 54, 64)','rgb(121, 80, 167)','rgb(237, 194, 77)','rgb(237, 194, 77)','rgb(237, 194, 77)','rgb(121, 80, 167)']);
      if(width===1440 || width===900) await screenshot(`copyrail-design-${theme}-${compact?'compact':'normal'}-${width}.png`);
      checks.push({theme,compact,width,layout});
    }
    await page('Emulation.setDeviceMetricsOverride',{width:1100,height:148,deviceScaleFactor:1,mobile:false});
    await page('Page.navigate',{url:fixture+'/?fixture=visual'});
    await waitFor(`document.querySelectorAll('.clip-card').length===8`);
    assert.equal(await evaluate(`document.querySelector('.clip-card').getBoundingClientRect().bottom<=innerHeight && document.querySelector('.toolbar').getBoundingClientRect().height===42`),true,'manually shortened normal window remains usable');
    await page('Emulation.setDeviceMetricsOverride',{width:1440,height:248,deviceScaleFactor:1,mobile:false});
    await page('Page.navigate',{url:fixture+'/?fixture=visual'});
    await waitFor(`document.querySelectorAll('.clip-card').length===8`);
    await evaluate(`(()=>{const input=document.querySelector('#history-search');input.value='CopyRail_no_matching_fixture';input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await waitFor(`document.querySelector('.empty-action')?.textContent==='清除搜索与筛选'`);
    await screenshot(`copyrail-design-empty-search-${theme}.png`);
    await evaluate(`document.querySelector('.empty-action').click()`);
    await waitFor(`document.querySelectorAll('.clip-card').length===8 && document.activeElement?.id==='history-search' && !document.querySelector('#history-search').value`);
    await evaluate(`[...document.querySelectorAll('.pinboard')].find(e=>e.textContent==='归档').click()`);
    await waitFor(`document.querySelector('.empty-action')?.textContent==='＋ 新建内容'`);
    await screenshot(`copyrail-design-empty-board-${theme}.png`);
    await evaluate(`document.querySelector('.empty-action').click()`);
    await waitFor(`document.querySelector('.content-editor')`);
  }
  assert.deepEqual(await fingerprint(),assets);
  await writeFile(join(artifacts,'copyrail-design-report.json'),JSON.stringify({result:'passed',browser,nativeEndToEnd:false,assetSha256:assets,checks,emptySearchClearAndNewContent:true},null,2)+'\n');
  console.log('CopyRail: 12 light/dark, normal/compact and width combinations; preserved type palette; clear search and create actions passed.');
});
