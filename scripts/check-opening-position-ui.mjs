// Real compiled UI with synthetic IPC. No system clipboard or user history.
import assert from 'node:assert/strict';
import {readFile,readdir,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {withCompiledUiTest} from './compiled-ui-test.mjs';
const root=new URL('../apps/desktop/dist/',import.meta.url);
const fingerprint=async()=>Object.fromEntries(await Promise.all((await readdir(root)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,root))).digest('hex')])));
const assets=await fingerprint();
await withCompiledUiTest(async({page,evaluate,waitFor,fixture,screenshot,artifacts,browser})=>{
 const key=async(key,code,virtual)=>{for(const type of ['keyDown','keyUp'])await page('Input.dispatchKeyEvent',{type,key,code,windowsVirtualKeyCode:virtual});};
 const active=`document.querySelector('#history-results')?.getAttribute('aria-activedescendant')`;
 const saved=`JSON.parse(localStorage.getItem('fixture-rail-position') ?? 'null')`;
 const open=()=>evaluate(`window.dragEventFixture.emit('pasters-rail-opened',null)`);
 await page('Emulation.setDeviceMetricsOverride',{width:760,height:348,deviceScaleFactor:1,mobile:false});
 await page('Page.navigate',{url:fixture+'/?fixture=visual&window_role=workspace'});
 await waitFor(`window.workspaceFixture`);
 await evaluate(`localStorage.clear();(()=>{const w=window.workspaceFixture;w.state={revision:1,content:{kind:'settings',tab:'general'}};window.dragEventFixture.emit('pasters-workspace',w.state)})()`);
 await waitFor(`document.querySelector('.opening-position-select')`);
 assert.equal(await evaluate(`document.querySelector('.opening-position-select').value`),'latest');
 const choose=async(value)=>evaluate(`(()=>{const e=document.querySelector('.opening-position-select');e.value='${value}';e.dispatchEvent(new Event('change',{bubbles:true}));})()`);
 await evaluate(`document.querySelector('.toggle-setting input').click()`);
 await choose('last');await waitFor(`localStorage.getItem('fixture-opening')==='last' && !document.querySelector('.opening-position-select').disabled`);
 assert.equal(await evaluate(`document.querySelector('.toggle-setting input').checked`),true,'preserve unrelated settings draft');
 await evaluate(`window.openingFixture.fail=true`);await choose('latest');
 await waitFor(`document.querySelector('.language-error') && !document.querySelector('.opening-position-select').disabled`);
 assert.equal(await evaluate(`localStorage.getItem('fixture-opening')`),'last');
 assert.equal(await evaluate(`document.querySelector('.opening-position-select').value`),'last');
 // Chinese and English use the same control and persisted mode.
 await evaluate(`window.openingFixture.fail=false;localStorage.setItem('fixture-language','en');window.dragEventFixture.emit('pasters-preferences-changed',null)`);
 await waitFor(`document.querySelector('.opening-position-select').getAttribute('aria-label')==='Selection on open'`);
 await screenshot('opening-position-settings.png');
 await page('Emulation.setDeviceMetricsOverride',{width:900,height:260,deviceScaleFactor:1,mobile:false});
 const url=fixture+'/?fixture=visual&window_role=main';
 await page('Page.navigate',{url});await waitFor(`document.querySelectorAll('.clip-card').length===8 && document.activeElement?.id==='history-results'`);
 await key('End','End',35);await waitFor(`${saved}?.clip_id?.endsWith('000000000008')`);
 const last=await evaluate(active);
 await open();await waitFor(`${active}===${JSON.stringify(last)} && document.activeElement?.id==='history-results'`);
 const visible=await evaluate(`(()=>{const c=document.getElementById(${active}).getBoundingClientRect(),r=document.querySelector('.card-track').getBoundingClientRect();return c.left>=r.left && c.right<=r.right})()`);
 assert.equal(visible,true,'restored card must be visible');
 await page('Page.reload');await waitFor(`${active}===${JSON.stringify(last)}`);
 // New captures move the bookmark past the first 200 rows.
 await evaluate(`(()=>{const items=window.sourceIconFixture.clips;const template=structuredClone(items[0]);items.unshift(...Array.from({length:210},(_,i)=>({...template,id:'00000000-0000-4000-8000-'+String(1000+i).padStart(12,'0'),title:'New synthetic '+i})));})()`);
 await open();await waitFor(`${active}===${JSON.stringify(last)} && ${saved}?.context.history_offset>0`);
 // Latest clears a previous search/offset and consistently selects first.
 await evaluate(`localStorage.setItem('fixture-opening','latest')`);await open();
 await waitFor(`${active}?.endsWith('000000001000') && document.querySelector('.search-wrap input')?.value===''`);
 await key('ArrowRight','ArrowRight',39);await open();await waitFor(`${active}?.endsWith('000000001000')`);
 // Saved bookmark can include a search context; reload preserves it.
 await evaluate(`localStorage.setItem('fixture-opening','last');document.querySelector('.search-wrap input').value='Rust';document.querySelector('.search-wrap input').dispatchEvent(new Event('input',{bubbles:true}))`);
 await waitFor(`${saved}?.context.text==='Rust'`);
 const searched=await evaluate(active);await page('Page.reload');
 await waitFor(`document.querySelector('.search-wrap input')?.value==='Rust' && ${active}===${JSON.stringify(searched)}`);
 // Deletion before opening falls back to latest and clears stale filtering.
 await evaluate(`(()=>{const items=window.sourceIconFixture.clips;const id=${saved}.clip_id;items.splice(items.findIndex(c=>c.id===id),1)})()`);await open();
 await waitFor(`document.querySelector('.search-wrap input')?.value==='' && ${active}?.endsWith('000000000001')`);
 // A superseded, delayed open response cannot restore a stale mode/selection.
 await key('End','End',35);await evaluate(`window.openingFixture.delayMs=200;localStorage.setItem('fixture-opening','last');window.beforeOpenCalls=window.openingFixture.calls.filter(c=>c.command==='get_rail_opening').length`);await open();
 await waitFor(`window.openingFixture.calls.filter(c=>c.command==='get_rail_opening').length>window.beforeOpenCalls`);
 await evaluate(`localStorage.setItem('fixture-opening','latest')`);await open();
 await waitFor(`${active}?.endsWith('000000000001')`);
 // Settings/preview close events do not count as reopening the rail.
 await key('ArrowRight','ArrowRight',39);const before=await evaluate(active);
 await evaluate(`window.dragEventFixture.emit('pasters-close-preview',null)`);
 assert.equal(await evaluate(active),before);
 assert.deepEqual(await fingerprint(),assets);
 await writeFile(join(artifacts,'opening-position-report.json'),JSON.stringify({result:'passed',browser,assetSha256:assets,nativeEndToEnd:false,checks:['independent setting save and failure','Chinese and English','latest on each invocation','stable ID and scrolling','reload bookmark and search','more than 200 newer captures','deleted bookmark fallback','superseded open response','workspace close preserves selection']},null,2)+'\n');
 console.log('Opening position checks passed.');
});
