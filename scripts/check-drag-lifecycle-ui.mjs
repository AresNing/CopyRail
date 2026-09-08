// Tests compiled production Rust/WASM with deliberately reordered synthetic
// event delivery. Does not operate AppKit, a real database or the clipboard.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';

const root = new URL('../apps/desktop/dist/', import.meta.url);
async function fingerprint() {
  const names = (await readdir(root)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort();
  return Object.fromEntries(await Promise.all(names.map(async name => [name, createHash('sha256').update(await readFile(new URL(name, root))).digest('hex')])));
}
const before = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 2, mobile: false });
  await page('Page.navigate', { url: `${fixture}/?fixture=pinboards` });
  await waitFor(`document.querySelectorAll('.clip-card').length === 5 && window.dragEventFixture`);
  await evaluate(`(() => {
    window.placements = []; window.dragLayouts = [];
    const invoke = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      if (command === 'place_pinboard_clips') window.placements.push(structuredClone(args.request));
      if (command === 'start_clip_drag') window.dragLayouts.push(structuredClone(args.request.feedback));
      return invoke(command, args);
    };
  })()`);
  const board = '10000000-0000-4000-8000-000000000011';
  const start = async () => {
    const p = await evaluate(`(() => { const r=document.querySelector('.clip-card').getBoundingClientRect(); return {x:r.x+70,y:r.y+125}; })()`);
    await page('Input.dispatchMouseEvent', { type:'mousePressed', ...p, button:'left', buttons:1, clickCount:1 });
    await page('Input.dispatchMouseEvent', { type:'mouseMoved', x:p.x+15, y:p.y, button:'left', buttons:1 });
    await waitFor(`window.dragEventFixture.current() !== null`);
    const drag = await evaluate(`window.dragEventFixture.current()`);
    await evaluate(`window.dragEventFixture.clear()`);
    await page('Input.dispatchMouseEvent', { type:'mouseReleased', ...p, button:'left', buttons:0, clickCount:1 });
    return drag;
  };
  const point = await evaluate(`(() => { const r=document.querySelector('[data-drop-board="${board}"]').getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
  const emit = (name, payload) => evaluate(`window.dragEventFixture.emit(${JSON.stringify(name)}, ${JSON.stringify(payload)})`);
  const event = (drag, phase) => ({...drag, phase, ...point});
  const highlighted = `document.querySelector('[data-drop-board="${board}"]').classList.contains('drop-target')`;
  const first = await start();
  const layout = await evaluate(`window.dragLayouts[0]`);
  assert.equal(layout.width, 1440);
  assert.equal(layout.height, 248);
  assert.equal(layout.tabs.length, 3);
  assert.ok(layout.tabs.every(t=>t.x>=0 && t.y>=0 && t.width>=4 && t.height>=4 && t.x+t.width<=1440 && t.y+t.height<=248));
  assert.ok(layout.tabs.find(t=>t.id===board && point.x>=t.x && point.x<t.x+t.width && point.y>=t.y && point.y<t.y+t.height));
  await emit('pasters-internal-drag', event(first,'enter'));
  await waitFor(highlighted);
  await waitFor(`getComputedStyle(document.querySelector('[data-drop-board="${board}"]')).backgroundColor === 'rgb(0, 84, 196)'`);
  const style = await evaluate(`(() => { const el=document.querySelector('[data-drop-board="${board}"]'); const s=getComputedStyle(el); return {classes:el.className,background:s.backgroundColor,transform:s.transform}; })()`);
  assert.equal(style.background,'rgb(0, 84, 196)',JSON.stringify(style));
  await screenshot('drag-target-highlight-2x.png');
  await emit('pasters-drag-ended', { sessionId:first.sessionId, cancelled:false });
  assert.equal(await evaluate(highlighted), false, 'end clears the target');
  await emit('pasters-internal-drag', event(first,'over'));
  assert.equal(await evaluate(highlighted), false, 'a queued hover after end must not revive the highlight');

  const second = await start();
  await emit('pasters-internal-drag', event(second,'enter'));
  await waitFor(highlighted);
  await emit('pasters-drag-ended', { sessionId:first.sessionId, cancelled:false });
  assert.equal(await evaluate(highlighted), true, 'old completion must not clear a newer active target');
  await emit('pasters-internal-drag', event(first,'leave'));
  assert.equal(await evaluate(highlighted), true, 'old leave must not clear a newer active target');
  await emit('pasters-drag-ended', { sessionId:second.sessionId, cancelled:false });
  await emit('pasters-internal-drag', event(second,'drop'));
  await waitFor(`window.placements.length === 1 && document.body.textContent.includes('已移动')`);
  assert.equal(await evaluate(`window.placements[0].pinboard_id`), board);
  await emit('pasters-internal-drag', event(second,'drop'));
  await evaluate(`new Promise(r=>setTimeout(r,600))`);
  assert.equal(await evaluate(`window.placements.length`), 1, 'a delivered drop is consumed once');
  assert.equal(await evaluate(`document.querySelector('.notice-banner')?.textContent.includes('已移动')`), true);
  await emit('pasters-drag-ended', { sessionId:first.sessionId, cancelled:true });
  assert.equal(await evaluate(`document.querySelector('.notice-banner')?.textContent.includes('已移动')`), true, 'an old cancellation must not erase a later successful result');
  const cancelled = await start();
  assert.equal(await evaluate(`document.querySelector('.notice-banner')`), null, 'a new drag clears the previous move result');
  await emit('pasters-internal-drag', event(cancelled,'enter'));
  await emit('pasters-drag-ended', { sessionId:cancelled.sessionId, cancelled:true });
  await emit('pasters-internal-drag', event(cancelled,'drop'));
  await emit('pasters-internal-drag', event(cancelled,'over'));
  assert.equal(await evaluate(highlighted), false);
  assert.equal(await evaluate(`window.placements.length`), 1, 'cancelled session rejects late drop');
  assert.equal(await evaluate(`document.querySelector('.notice-banner')`), null, 'cancelled drag must not show an old success notice');
  await evaluate(`document.querySelector('[data-drop-board="${board}"]').click()`);
  await waitFor(`document.querySelector('[data-drop-board="${board}"]').classList.contains('active') && document.querySelectorAll('.clip-card').length === 2`);
  await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'dark'},{name:'prefers-reduced-motion',value:'reduce'}]});
  const sameBoard=await start();
  const activePoint=await evaluate(`(() => { const r=document.querySelector('[data-drop-board="${board}"]').getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
  await emit('pasters-internal-drag',{...event(sameBoard,'enter'),...activePoint});
  await waitFor(highlighted);
  assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-drop-board="${board}"]')).backgroundColor`),'rgb(0, 84, 196)','dark active tab must not override the target blue');
  assert.equal(await evaluate(`getComputedStyle(document.querySelector('[data-drop-board="${board}"]')).transform`),'none','reduced motion keeps static target feedback');
  await emit('pasters-drag-ended',{sessionId:sameBoard.sessionId,cancelled:true});
  assert.deepEqual(await evaluate(`(() => { const s=getComputedStyle(document.querySelector('.paste-shell')); return [s.borderTopLeftRadius,s.borderTopRightRadius,s.borderBottomLeftRadius,s.borderBottomRightRadius]; })()`),['16px','16px','16px','16px']);
  await page('Emulation.setDefaultBackgroundColorOverride',{ color:{r:0,g:0,b:0,a:0} });
  const transparent=await page('Page.captureScreenshot',{format:'png'});
  const alpha=await evaluate(`(async()=>{ const img=new Image(); img.src='data:image/png;base64,${transparent.data}'; await img.decode(); const c=document.createElement('canvas'); c.width=img.width;c.height=img.height; const ctx=c.getContext('2d'); ctx.drawImage(img,0,0); return [[0,0],[img.width-1,0],[0,img.height-1],[img.width-1,img.height-1]].map(([x,y])=>ctx.getImageData(x,y,1,1).data[3]); })()`);
  assert.deepEqual(alpha,[0,0,0,0],'four outer corners are transparent, not opaque square corners');
  await writeFile(join(artifacts,'floating-panel-corners-2x.png'),Buffer.from(transparent.data,'base64'));
  assert.deepEqual(await fingerprint(), before, 'compiled assets unchanged during test');
  const report = { result:'passed', browser, nativeEndToEnd:false, assetSha256:before, checks:[
    'end clears the target; queued hover cannot revive it',
    'stale completion and leave cannot disturb a newer drag',
    'a queued valid drop after source completion is preserved and consumed once',
    'cancelled session rejects both late hover and late drop',
    'new drag clears the previous success notice; cancellation does not revive it or erase a later result',
    'native feedback receives bounded view-point tab geometry at 2x backing',
    'target is blue and all four 16pt outer corners render transparent',
    'dark active tab retains target blue; reduced motion disables scale feedback',
  ] };
  await writeFile(join(artifacts,'drag-lifecycle-report.json'), JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
