// Compiled production Rust/WASM; synthetic IPC and 80 synthetic Pinboards.
// Does not manipulate the running native acceptance window or clipboard.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';

const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort().map(async name => [name, createHash('sha256').update(await readFile(new URL(name, root))).digest('hex')])));
const before = await fingerprint();
await withCompiledUiTest(async ({page, evaluate, waitFor, fixture, screenshot, browser, artifacts}) => {
  await page('Emulation.setDeviceMetricsOverride', {width:1440, height:248, deviceScaleFactor:2, mobile:false});
  await page('Page.navigate', {url:`${fixture}/?fixture=pinboards&many_boards=1`});
  await waitFor(`document.querySelectorAll('[data-drop-board]').length === 80 && document.querySelectorAll('.clip-card').length === 5`);
  await evaluate(`(() => {
    window.layouts=[]; window.updates=[]; window.placements=[]; window.pendingUpdates=[]; window.holdUpdates=false;
    const invoke=window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke=async(command,args)=>{
      if(command==='start_clip_drag') window.layouts.push(structuredClone(args.request.feedback));
      if(command==='place_pinboard_clips') window.placements.push(structuredClone(args.request));
      if(command==='update_clip_drag_feedback') {
        window.updates.push(structuredClone(args.request));
        if(window.holdUpdates) await new Promise(resolve=>window.pendingUpdates.push(resolve));
      }
      return invoke(command,args);
    };
  })()`);
  assert.equal(await evaluate(`(() => { const nav=document.querySelector('.pinboards'); const r=nav.getBoundingClientRect(); const first=nav.firstElementChild.getBoundingClientRect(); return first.left>=r.left-.5 && first.right<=r.right+.5; })()`),true,'overflow must not put the first tab into unreachable negative space');
  await evaluate(`document.querySelector('.pinboards').scrollLeft=100000`);
  const last = '10000000-0000-4000-8000-000000000179';
  const start = async () => {
    const p=await evaluate(`(() => { const r=document.querySelector('.clip-card').getBoundingClientRect();return {x:r.x+70,y:r.y+125}; })()`);
    await page('Input.dispatchMouseEvent',{type:'mousePressed',...p,button:'left',buttons:1,clickCount:1});
    await page('Input.dispatchMouseEvent',{type:'mouseMoved',x:p.x+15,y:p.y,button:'left',buttons:1});
    await waitFor(`window.dragEventFixture.current() !== null`);
    const drag=await evaluate(`window.dragEventFixture.current()`);
    await evaluate(`window.dragEventFixture.clear()`);
    await page('Input.dispatchMouseEvent',{type:'mouseReleased',...p,button:'left',buttons:0,clickCount:1});
    return drag;
  };
  const emit=(name,payload)=>evaluate(`window.dragEventFixture.emit(${JSON.stringify(name)},${JSON.stringify(payload)})`);
  const first=await start();
  assert.ok((await evaluate(`window.layouts[0].tabs`)).some(t=>t.id===last), 'visible tabs beyond DOM index 64 must reach native feedback');
  const point=await evaluate(`(() => {const r=document.querySelector('.pinboards').getBoundingClientRect();return {x:r.left+8,y:r.top+r.height/2};})()`);
  const event=(drag,phase,extra={})=>({...drag,phase,...point,...extra});
  const scrollBefore=await evaluate(`document.querySelector('.pinboards').scrollLeft`);
  await emit('pasters-internal-drag',event(first,'enter'));
  await waitFor(`window.updates.length > 0`);
  assert.ok(await evaluate(`document.querySelector('.pinboards').scrollLeft < ${scrollBefore}`), 'actual edge scroll ran');
  const firstUpdate=await evaluate(`window.updates.at(-1)`);
  assert.equal(firstUpdate.session_id, first.sessionId);
  assert.equal(firstUpdate.layout.width,1440);
  assert.ok(firstUpdate.layout.tabs.length > 0 && firstUpdate.layout.tabs.length <= 64);
  assert.notDeepEqual(firstUpdate.layout,await evaluate(`window.layouts[0]`), 'scroll publishes changed geometry');
  await evaluate(`window.holdUpdates=true`);
  await emit('pasters-internal-drag',event(first,'over'));
  await waitFor(`window.pendingUpdates.length === 1`);
  const heldCount=await evaluate(`window.updates.length`);
  for(let n=0;n<12;n++) await emit('pasters-internal-drag',event(first,'over'));
  assert.equal(await evaluate(`window.updates.length`),heldCount,'only one update in flight');
  await evaluate(`window.holdUpdates=false; window.pendingUpdates.shift()()`);
  await waitFor(`window.updates.length === ${heldCount+1}`);
  const updates=await evaluate(`window.updates`);
  assert.ok(updates.at(-1).revision > updates.at(-2).revision+1,'coalesces intermediate layouts into the latest revision');
  assert.ok(updates.every((u,i)=>!i || u.revision>updates[i-1].revision));
  await screenshot('drag-scrolled-target-2x.png');
  // Settled in the middle of the nav: no scroll, no transform feedback loop.
  const center=await evaluate(`(() => {const r=document.querySelector('.pinboards').getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height/2};})()`);
  await emit('pasters-internal-drag',event(first,'over',center));
  await evaluate(`new Promise(r=>setTimeout(r,200))`);
  const settled=await evaluate(`window.updates.length`);
  for(let n=0;n<8;n++) await emit('pasters-internal-drag',event(first,'over',center));
  assert.equal(await evaluate(`window.updates.length`),settled,'stable hover must not republish geometry due to highlight scaling');
  await emit('pasters-drag-ended',{sessionId:first.sessionId,cancelled:true});
  await emit('pasters-internal-drag',event(first,'over'));
  assert.equal(await evaluate(`window.updates.length`),settled,'late hover does not refresh closed session');
  // A native target and actual visible tab disagree: never silently move into
  // a different board. A following matching drag still commits exactly once.
  const visible=await evaluate(`(() => {const el=document.elementFromPoint(${center.x},${center.y}).closest('[data-drop-board]');return el?.dataset.dropBoard;})()`);
  assert.ok(visible,'test point is over a real visible tab');
  const second=await start();
  await emit('pasters-internal-drag',event(second,'enter',center));
  await emit('pasters-internal-drag',event(second,'over',{...center,tabFeedback:{target:null}}));
  assert.equal(await evaluate(`document.querySelectorAll('.pinboard.drop-target').length`),0,'native denied/unavailable target cannot leave a misleading DOM success highlight');
  await emit('pasters-internal-drag',event(second,'drop',{...center,tabFeedback:{target:last===visible?'10000000-0000-4000-8000-000000000010':last}}));
  assert.equal(await evaluate(`window.placements.length`),0,'stale native target cannot move into a different DOM target');
  await waitFor(`document.body.textContent.includes('分类位置已变化')`);
  const third=await start();
  await emit('pasters-internal-drag',event(third,'enter',center));
  await emit('pasters-internal-drag',event(third,'drop',{...center,tabFeedback:{target:visible}}));
  await waitFor(`window.placements.length === 1 && document.body.textContent.includes('已移动')`);
  assert.equal(await evaluate(`window.placements[0].pinboard_id`),visible);
  assert.deepEqual(await fingerprint(),before,'assets remain unchanged during verification');
  const report={result:'passed',browser,nativeEndToEnd:false,assetSha256:before,checks:[
    'visible labels beyond the first 64 DOM nodes are included',
    'edge scroll publishes fresh view-point geometry with increasing revisions',
    'one in-flight update with intermediate layouts coalesced; stable hover is deduplicated',
    'completed sessions cannot publish late hover geometry',
    'native/DOM target mismatch rejects placement; matching target commits once',
    'native denied/unavailable target suppresses the DOM success highlight',
  ]};
  await writeFile(join(artifacts,'drag-scroll-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
