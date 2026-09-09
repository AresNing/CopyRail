// Real compiled WASM with synthetic IPC. AppKit acceptance is recorded separately.
import assert from 'node:assert/strict';
import {readFile,readdir,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {withCompiledUiTest} from './compiled-ui-test.mjs';
const root=new URL('../apps/desktop/dist/',import.meta.url);
const fingerprint=async()=>Object.fromEntries(await Promise.all((await readdir(root)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,root))).digest('hex')])));
const assets=await fingerprint();
await withCompiledUiTest(async({page,evaluate,waitFor,fixture,screenshot,artifacts,browser})=>{
  const shots=[];
  const size=(width,height)=>page('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
  const key=async(key,code,virtual)=>{for(const type of ['keyDown','keyUp']) await page('Input.dispatchKeyEvent',{type,key,code,windowsVirtualKeyCode:virtual});};
  const click=async selector=>{
    const p=await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);
    await page('Input.dispatchMouseEvent',{type:'mousePressed',...p,button:'left',clickCount:1});
    await page('Input.dispatchMouseEvent',{type:'mouseReleased',...p,button:'left',clickCount:1});
  };
  const visibleCard=()=>evaluate(`(()=>{const c=document.querySelector('.keyboard-active').getBoundingClientRect(),r=document.querySelector('.card-track').getBoundingClientRect();return c.left>=r.left && c.right<=r.right})()`);
  for(const theme of ['light','dark']) {
    await size(1440,248);
    await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:theme}]});
    await page('Page.navigate',{url:fixture+'/?fixture=visual'});
    await waitFor(`document.querySelectorAll('.clip-card').length===8`);
    // Keep the destination in the middle of a longer track. A final-card-only
    // test clamps to scrollWidth and misses WebKit's adjacent snap-point case.
    await evaluate(`(()=>{const clips=window.sourceIconFixture.clips;for(let i=8;i<24;i++){const clip=structuredClone(clips[0]);clip.id='10000000-0000-4000-8000-'+String(i+1).padStart(12,'0');clip.title='Synthetic card '+(i+1);clips.push(clip);}})()`);
    await waitFor(`document.querySelectorAll('.clip-card').length===24`);
    await evaluate(`document.querySelector('#history-results').focus()`);
    for(let n=0;n<7;n++) await key('ArrowRight','ArrowRight',39);
    await waitFor(`document.querySelectorAll('.clip-card')[7].classList.contains('keyboard-active')`);
    await evaluate(`new Promise(r=>setTimeout(r,200))`);
    assert.equal(await visibleCard(),true,'right arrow fully exposes an interior card beyond the first viewport');
    await key('Home','Home',36);
    await waitFor(`document.querySelectorAll('.clip-card')[0].classList.contains('keyboard-active')`);
    await evaluate(`new Promise(r=>setTimeout(r,200))`);
    assert.equal(await visibleCard(),true,'Home scrolls back to the first card');
    await evaluate(`document.querySelector('.card-track').scrollLeft=400`);
    await evaluate(`new Promise(r=>setTimeout(r,250))`);
    const manualScroll=await evaluate(`document.querySelector('.card-track').scrollLeft`);
    await evaluate(`new Promise(r=>setTimeout(r,1600))`);
    assert.ok(manualScroll>0);
    assert.equal(await evaluate(`document.querySelector('.card-track').scrollLeft`),manualScroll,'background refresh must not snap manual scrolling back');
    await key('End','End',35);
    await waitFor(`document.querySelectorAll('.clip-card')[23].classList.contains('keyboard-active')`);
    await evaluate(`new Promise(r=>setTimeout(r,200))`);
    assert.equal(await visibleCard(),true);
    shots.push(await screenshot(`workspace-scroll-${theme}.png`));
    await click('.organize-button');
    await waitFor(`document.querySelector('.pin-menu')`);
    await click('.rail-brand');
    await waitFor(`!document.querySelector('.pin-menu')`);
    await click('.filter-toggle');
    await waitFor(`document.querySelector('.filter-popover')`);
    await click('.rail-brand');
    await waitFor(`!document.querySelector('.filter-popover')`);
    await evaluate(`document.querySelector('#history-results').focus();window.dragEventFixture.emit('pasters-edit-action','paste')`);
    await waitFor(`document.querySelector('.notice-banner')`);
    await waitFor(`!document.querySelector('.notice-banner')`);
    await key('Home','Home',36);
    await key(' ','Space',32);
    await waitFor(`document.querySelector('.preview-overlay') && window.previewFrameFixture.open`);
    await size(1100,608);
    await waitFor(`document.activeElement?.classList.contains('preview-overlay')`);
    assert.equal(await evaluate(`getComputedStyle(document.activeElement).outlineStyle`),'none','preview has no automatic WebKit-style ring');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.timeline')).visibility`),'visible');
    assert.equal(await evaluate(`document.querySelector('.preview-overlay').getAttribute('aria-modal')`),'false');
    const geometry=await evaluate(`(()=>{const p=document.querySelector('.preview-overlay').getBoundingClientRect(),t=document.querySelector('.toolbar').getBoundingClientRect();return {w:p.width,h:p.height,bottom:p.bottom,top:t.top}})()`);
    assert.ok(geometry.w<=720 && geometry.h<=360 && geometry.bottom<geometry.top,JSON.stringify(geometry));
    shots.push(await screenshot(`workspace-preview-${theme}.png`));
    await key('ArrowRight','ArrowRight',39);
    await waitFor(`document.querySelector('.preview-overlay header strong').textContent===document.querySelectorAll('.card-title')[1].textContent`);
    await key('Escape','Escape',27);
    await waitFor(`!document.querySelector('.preview-overlay') && !window.previewFrameFixture.open`);
    await size(1100,248);
    const railStyle = () => evaluate(`(()=>{const e=document.querySelector('.dock-surface'),s=getComputedStyle(e),r=e.getBoundingClientRect();return {height:r.height,bottom:innerHeight-r.bottom,background:s.backgroundColor,border:s.border,shadow:s.boxShadow}})()`);
    const stableRail=await railStyle();
    await click('.settings-button');
    await waitFor(`document.querySelector('.settings-popover') && window.previewFrameFixture.open`);
    // Hold the old native viewport after IPC completes: this is the frame that
    // used to squeeze the rail and flash the settings across its contents.
    await evaluate(`new Promise(r=>setTimeout(r,150))`);
    assert.deepEqual(await railStyle(),stableRail,'opening before WebKit resize keeps the same rail');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.settings-popover')).visibility`),'hidden');
    await size(1100,608);
    await waitFor(`document.querySelector('.paste-shell.workspace-ready')`);
    assert.deepEqual(await railStyle(),stableRail,'expanded rail keeps its original paint and bottom anchor');
    for(const [index,name] of ['general','shortcuts','history','backup','advanced'].entries()) {
      await click(`.settings-nav button:nth-child(${index+1})`);
      assert.deepEqual(await evaluate(`[...document.querySelectorAll('.settings-page')].filter(e=>!e.hidden).map(e=>e.dataset.settingsPage)`),[name]);
      assert.equal(await evaluate(`document.querySelector('.settings-content').scrollWidth<=document.querySelector('.settings-content').clientWidth+1`),true);
      shots.push(await screenshot(`workspace-settings-${name}-${theme}.png`));
    }
    // Native Escape event is shared by the expanded workspace, including settings.
    await evaluate(`window.dragEventFixture.emit('pasters-close-preview',null)`);
    await waitFor(`!document.querySelector('.settings-popover') && !window.previewFrameFixture.open`);
    await evaluate(`new Promise(r=>setTimeout(r,150))`);
    assert.deepEqual(await railStyle(),stableRail,'closing before native shrink must not fill the tall host');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.paste-shell')).backgroundColor`),'rgba(0, 0, 0, 0)');
    await size(1100,248);
    await click('.settings-button'); await waitFor(`document.querySelector('.settings-popover')`); await size(1100,608);
    await click('.rail-brand'); await waitFor(`!document.querySelector('.settings-popover') && !window.previewFrameFixture.open`);
    await size(1100,248);
    await evaluate(`window.previewFrameFixture.delayMs=180;window.previewFrameFixture.calls=[]`);
    await click('.settings-button');
    await waitFor(`document.querySelector('.settings-popover')`);
    await click('.settings-button');
    await waitFor(`!document.querySelector('.settings-popover') && !window.previewFrameFixture.open && window.previewFrameFixture.calls.at(-1)===false`);
    assert.deepEqual(await railStyle(),stableRail,'rapid open/close preserves the dock');
    await evaluate(`window.previewFrameFixture.delayMs=0`);
  }
  // Compact mode and a small display keep the rail and reader separate.
  await page('Page.navigate',{url:fixture+'/?fixture=visual&compact=1'});
  await size(900,148); await waitFor(`document.querySelectorAll('.clip-card').length===8`);
  await evaluate(`document.querySelector('.kind-image').click()`); await key(' ','Space',32);
  await waitFor(`document.querySelector('.preview-overlay')`); await size(900,508);
  assert.equal(await evaluate(`document.querySelector('.dock-surface').getBoundingClientRect().height`),148);
  shots.push(await screenshot('workspace-preview-compact.png'));
  await size(640,456);
  assert.equal(await evaluate(`(()=>{const p=document.querySelector('.preview-overlay').getBoundingClientRect(),d=document.querySelector('.dock-surface').getBoundingClientRect();return p.left>=0 && p.right<=innerWidth && p.bottom<d.top})()`),true);
  shots.push(await screenshot('workspace-preview-small.png'));
  await size(1100,148);
  await page('Page.navigate',{url:fixture+'/?fixture=visual'});
  await waitFor(`document.querySelectorAll('.clip-card').length===8`);
  assert.equal(await evaluate(`document.querySelector('.toolbar').getBoundingClientRect().height`),42);
  await click('.settings-button');
  await waitFor(`document.querySelector('.settings-popover') && window.previewFrameFixture.open`);
  await size(1100,508);
  await waitFor(`document.querySelector('.paste-shell.workspace-ready')`);
  assert.equal(await evaluate(`document.querySelector('.toolbar').getBoundingClientRect().height`),42,'short normal rail keeps its toolbar during expansion');
  assert.equal(await evaluate(`document.querySelector('.dock-surface').getBoundingClientRect().height`),148);
  shots.push(await screenshot('workspace-settings-short-normal.png'));
  assert.deepEqual(await fingerprint(),assets);
  const report={result:'passed',browser,nativeEndToEnd:false,assetSha256:assets,checks:['keyboard selection scrolls both directions','manual scroll survives refresh','outside pointer closes menus and settings','notice expires','compact preview preserves rail and switches items','five settings categories','native Escape event closes settings','compact and small viewport bounds','stable rail through delayed open/close resize'],screenshots:shots};
  await writeFile(join(artifacts,'workspace-interactions-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
