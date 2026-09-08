// The actual compiled Rust UI with synthetic IPC; no native window or clipboard.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,root))).digest('hex')])));
const before=await fingerprint();
await withCompiledUiTest(async ({page,evaluate,waitFor,fixture,screenshot,browser,artifacts})=>{
  await page('Emulation.setDeviceMetricsOverride',{width:1440,height:248,deviceScaleFactor:2,mobile:false});
  await page('Page.navigate',{url:`${fixture}/?fixture=pinboards`});
  await waitFor(`document.querySelectorAll('.clip-card').length === 5`);
  const id=n=>`10000000-0000-4000-8000-${String(n).padStart(12,'0')}`;
  const inbox=id(10);
  await evaluate(`(() => {window.layouts=[];window.placements=[];window.updates=[];const invoke=window.__TAURI__.core.invoke;window.__TAURI__.core.invoke=async(command,args)=>{if(command==='start_clip_drag')window.layouts.push(structuredClone(args.request.feedback));if(command==='place_pinboard_clips')window.placements.push(structuredClone(args.request));if(command==='update_clip_drag_feedback')window.updates.push(structuredClone(args.request));return invoke(command,args);};document.querySelector('[data-drop-board="${inbox}"]').click();})()`);
  await waitFor(`document.querySelectorAll('.clip-card').length === 4`);
  const start=async()=>{
    const p=await evaluate(`(()=>{const r=document.querySelector('.clip-card').getBoundingClientRect();return{x:r.x+70,y:r.y+110};})()`);
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
  const layout=await evaluate(`window.layouts.at(-1)`);
  assert.equal(layout.timeline?.pinboard_id,inbox,'native destination needs the real active-board reorder geometry');
  assert.equal(layout.timeline.cards.length,4);
  const card=n=>layout.timeline.cards.find(c=>c.id===id(n)).bounds;
  const B=card(2),C=card(3),A=card(1);
  const target=(anchor,after)=>({pinboard_id:inbox,anchor:id(anchor),after});
  const dragEvent=(drag,phase,x,y,t)=>({...drag,phase,x,y,placementFeedback:{target:t}});
  await emit('pasters-internal-drag',dragEvent(first,'enter',A.x+50,A.y+80,null));
  assert.equal(await evaluate(`document.querySelectorAll('.drop-before,.drop-after,.drop-append').length`),0,'hovering the dragged source is not an insertion');
  await emit('pasters-internal-drag',dragEvent(first,'over',B.x+30,B.y+80,target(2,false)));
  await waitFor(`document.querySelector('[data-drop-clip="${id(2)}"]').classList.contains('drop-before')`);
  await emit('pasters-internal-drag',dragEvent(first,'over',B.x+B.width-20,B.y+80,target(2,true)));
  await waitFor(`document.querySelector('[data-drop-clip="${id(2)}"]').classList.contains('drop-after')`);
  const gap=(B.x+B.width+C.x)/2;
  await emit('pasters-internal-drag',dragEvent(first,'over',gap,B.y+80,target(3,false)));
  await waitFor(`document.querySelector('[data-drop-clip="${id(3)}"]').classList.contains('drop-before')`);
  await waitFor(`getComputedStyle(document.querySelector('[data-drop-clip="${id(3)}"]')).boxShadow === 'none'`);
  assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-drop-clip="${id(3)}"]')).boxShadow`),'none','native marker must not have a second WebView marker underneath');
  await emit('pasters-internal-drag',{...first,phase:'over',x:gap,y:B.y+80});
  await waitFor(`getComputedStyle(document.querySelector('[data-drop-clip="${id(3)}"]')).boxShadow.includes('255, 149, 0')`);
  assert.notEqual(await evaluate(`getComputedStyle(document.querySelector('[data-drop-clip="${id(3)}"]')).boxShadow`),'none','non-native fallback retains its insertion feedback');
  await screenshot('drag-card-gap-insertion-2x.png');
  await emit('pasters-internal-drag',dragEvent(first,'over',100,25,null));
  assert.equal(await evaluate(`document.querySelectorAll('.drop-before,.drop-after,.drop-append,.drop-target').length`),0,'toolbar clears feedback');
  await emit('pasters-internal-drag',dragEvent(first,'drop',gap,B.y+80,target(3,false)));
  await waitFor(`window.placements.length === 1 && document.body.textContent.includes('已移动')`);
  assert.deepEqual(await evaluate(`window.placements[0]`),{...target(3,false),clip_ids:[id(1)]});
  await waitFor(`Array.from(document.querySelectorAll('[data-drop-clip]')).map(e=>e.dataset.dropClip).join(',') === '${[2,1,3,4].map(id).join(',')}'`);
  const second=await start();
  await emit('pasters-internal-drag',dragEvent(second,'enter',C.x+30,C.y+80,target(3,false)));
  await emit('pasters-internal-drag',dragEvent(second,'drop',C.x+30,C.y+80,target(3,true)));
  assert.equal(await evaluate(`window.placements.length`),1,'same board but mismatched before/after must not silently reorder');
  await waitFor(`document.body.textContent.includes('本次未移动')`);
  await page('Emulation.setDeviceMetricsOverride',{width:640,height:248,deviceScaleFactor:2,mobile:false});
  const third=await start();
  const edge=await evaluate(`(()=>{const el=document.querySelector('.card-track');const r=el.getBoundingClientRect();return{x:r.right-7,y:r.top+80,left:el.scrollLeft};})()`);
  await emit('pasters-internal-drag',{...third,phase:'enter',x:edge.x,y:edge.y});
  assert.ok(await evaluate(`document.querySelector('.card-track').scrollLeft > ${edge.left}`),'native drag edge scroll must not snap back to the old card');
  await waitFor(`window.updates.some(u=>u.session_id==='${third.sessionId}' && u.layout.width===640 && u.layout.timeline?.cards.some(c=>c.bounds.x<20))`);
  await emit('pasters-drag-ended',{sessionId:third.sessionId,cancelled:true});
  assert.notEqual(await evaluate(`getComputedStyle(document.querySelector('.card-track')).scrollSnapType`),'none','normal scroll snapping is restored after drag');
  await evaluate(`document.querySelector('.pinboards button').click()`);
  await waitFor(`document.querySelectorAll('.clip-card').length === 5`);
  const history=await start();
  assert.equal((await evaluate(`window.layouts.at(-1)`)).timeline ?? null,null,'history does not pretend to have a board reorder target');
  await emit('pasters-drag-ended',{sessionId:history.sessionId,cancelled:true});
  assert.deepEqual(await fingerprint(),before);
  const report={result:'passed',browser,nativeEndToEnd:false,assetSha256:before,checks:[
    'active board exports bounded visible card geometry; history has no reorder target',
    'source hover has no marker; card halves and gaps choose exact anchors',
    'toolbar clears feedback; gap drop produces B,A,C,D rather than append-at-end',
    'native/DOM before-after mismatch in the same board rejects placement',
    'edge scroll advances while dragging and restores normal snap afterwards',
    'AppKit-owned feedback suppresses duplicate DOM markers; fallback still draws',
  ]};
  await writeFile(join(artifacts,'drag-placement-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
