// Compiled Rust/WASM and synthetic IPC only. Native screenshots motivated
// this check; Chrome results do not prove native WebKit rendering parity.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';

const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort().map(async n => [n, createHash('sha256').update(await readFile(new URL(n, root))).digest('hex')])));
const assets = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const observations = [];
  const tabs = `Array.from(document.querySelectorAll('.pinboard')).filter(e => !e.classList.contains('add-pinboard') && !e.classList.contains('manage-pinboard'))`;
  const inspect = () => evaluate(`(${tabs}).map(e => ({name:e.textContent.trim(), active:e.classList.contains('active'), current:e.getAttribute('aria-current'), hovered:e.matches(':hover'), background:getComputedStyle(e).backgroundColor}))`);
  const click = name => evaluate(`(${tabs}).find(e => e.textContent.trim() === ${JSON.stringify(name)}).click()`);
  for (const compact of [false, true]) for (const theme of ['light', 'dark']) {
    await page('Emulation.setDeviceMetricsOverride', { width:1439, height:compact ? 148 : 248, deviceScaleFactor:2, mobile:false });
    await page('Emulation.setEmulatedMedia', { features:[{name:'prefers-color-scheme',value:theme},{name:'prefers-reduced-motion',value:'reduce'}] });
    await page('Page.navigate', { url:`${fixture}/?fixture=pinboards${compact ? '&compact=1' : ''}` });
    await waitFor(`document.querySelectorAll('.clip-card').length === 5 && document.querySelector('.paste-shell').classList.contains('compact') === ${compact}`);
    // AX-style activation does not require the mouse to leave the old tab.
    // Keep hover over Clipboard while changing boards via their click action.
    for (const [name,count] of [['工作',1],['收件箱',4],['归档',0],['Clipboard',5]]) {
      await click(name);
      await waitFor(`document.querySelectorAll('.clip-card').length === ${count}`);
      // The manage button changes nav geometry: target the new bounds,
      // never silently compare a non-hovered transparent background.
      const point = await evaluate(`(() => {const r=(${tabs})[0].getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
      await page('Input.dispatchMouseEvent', {type:'mouseMoved',...point,button:'none',buttons:0});
      const state = await inspect();
      observations.push({compact,theme,name,state});
      console.log(JSON.stringify(observations.at(-1)));
      assert.deepEqual(state.filter(t => t.active).map(t => t.name),[name], 'one actual board selected');
      assert.equal(state[0].hovered,true,'the test must actually hover Clipboard after navigation reflow');
      if (name === '工作') {
        await screenshot(`pinboard-state-${compact ? 'compact' : 'expanded'}-${theme}.png`);
        assert.notEqual(state[0].background,state.find(t => t.active).background,'hovering Clipboard must not look identical to the selected Work board');
      }
      assert.deepEqual(state.filter(t => t.current === 'true').map(t => t.name),[name], 'current board must be exposed independently of pointer or keyboard focus');
    }
    await click('工作');
    await waitFor(`document.querySelectorAll('.clip-card').length === 1`);
    // A drop target is a third state, not a pointer-hover variant. Exercise
    // the existing drag bridge while the native session is simulated.
    const p = await evaluate(`(() => {const r=document.querySelector('.clip-card').getBoundingClientRect();return {x:r.x+70,y:r.y+40};})()`);
    await page('Input.dispatchMouseEvent',{type:'mousePressed',...p,button:'left',buttons:1,clickCount:1});
    await page('Input.dispatchMouseEvent',{type:'mouseMoved',x:p.x+15,y:p.y,button:'left',buttons:1});
    await waitFor(`window.dragEventFixture.current() !== null`);
    const drag = await evaluate(`window.dragEventFixture.current()`);
    await evaluate(`window.dragEventFixture.clear()`);
    await page('Input.dispatchMouseEvent',{type:'mouseReleased',...p,button:'left',buttons:0,clickCount:1});
    const inbox = `[data-drop-board="10000000-0000-4000-8000-000000000010"]`;
    const target = await evaluate(`(() => {const r=document.querySelector('${inbox}').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await page('Input.dispatchMouseEvent',{type:'mouseMoved',...target,button:'none',buttons:0});
    await evaluate(`window.dragEventFixture.emit('pasters-internal-drag',${JSON.stringify({...drag,phase:'enter',...target})})`);
    await waitFor(`document.querySelector('${inbox}').classList.contains('drop-target')`);
    assert.equal(await evaluate(`document.querySelector('${inbox}').matches(':hover')`),true);
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('${inbox}')).backgroundColor`),'rgb(0, 84, 196)','hover must not override native drop-target blue');
    assert.deepEqual((await inspect()).filter(t => t.current === 'true').map(t => t.name),['工作'],'hovering a drop target does not navigate');
    await evaluate(`window.dragEventFixture.emit('pasters-drag-ended',${JSON.stringify({sessionId:drag.sessionId,cancelled:true})})`);
    await waitFor(`!document.querySelector('${inbox}').classList.contains('drop-target')`);
    await evaluate(`(() => {const e=document.querySelector('#history-search');e.value='合成';e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await waitFor(`document.querySelector('#history-results')?.getAttribute('aria-label') === '搜索结果'`);
    let state = await inspect();
    assert.equal(state.filter(t => t.active || t.current === 'true').length,0,'global search must not pretend to be the remembered board');
    await evaluate(`(() => {const e=document.querySelector('#history-search');e.value='';e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await waitFor(`document.querySelectorAll('.clip-card').length === 1`);
    state = await inspect();
    assert.deepEqual(state.filter(t => t.current === 'true').map(t => t.name),['工作'],'clearing search restores the remembered board');
  }
  assert.deepEqual(await fingerprint(),assets);
  const report = {result:'passed',browser,nativeEndToEnd:false,voiceOverSpeech:false,assetSha256:assets,observations,checks:['one selected board across all four destinations','verified real hover does not impersonate selection','hover does not override drop-target blue or navigate; cancellation clears target','aria-current follows navigation but not global search','clearing search restores board','light/dark and Compact/expanded']};
  await writeFile(join(artifacts,'pinboard-state-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
