import assert from 'node:assert/strict';
import {readFile,readdir,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {withCompiledUiTest} from './compiled-ui-test.mjs';
const root=new URL('../apps/desktop/dist/',import.meta.url);
const fingerprint=async()=>Object.fromEntries(await Promise.all((await readdir(root)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,root))).digest('hex')])));
const assets=await fingerprint();
await withCompiledUiTest(async({page,evaluate,waitFor,fixture,screenshot,artifacts,browser})=>{
 await page('Emulation.setDeviceMetricsOverride',{width:760,height:348,deviceScaleFactor:2,mobile:false});
 const open=async()=>{
  await waitFor(`document.querySelector('.auxiliary-workspace')`);
  await evaluate(`(()=>{const w=window.workspaceFixture;w.state={revision:w.state.revision+1,content:{kind:'settings',tab:'general'}};window.dragEventFixture.emit('pasters-workspace',w.state)})()`);
  await waitFor(`document.querySelector('.appearance-slider input') && window.workspaceFixture.presented`);
 };
 for(const theme of ['dark','light']) {
  await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:theme}]});
  await page('Page.navigate',{url:fixture+'/?fixture=visual&window_role=workspace'});await open();
  const set=async(value)=>evaluate(`(()=>{const e=document.querySelector('.appearance-slider input');e.value=${value};e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  for(const value of [0,50,100]) {
   await set(value);await waitFor(`Number(localStorage.getItem('fixture-transparency'))===${value} && !document.querySelector('.save-settings').disabled`);
   assert.equal(await evaluate(`getComputedStyle(document.querySelector('.paste-shell')).getPropertyValue('--background-opacity').trim()`),String(1-value/100));
   assert.equal(await evaluate(`getComputedStyle(document.querySelector('.settings-popover')).opacity`),'1');
   const actualAlpha=await evaluate(`(()=>{const color=getComputedStyle(document.querySelector('.settings-popover')).backgroundColor;return color.startsWith('rgba')?Number(color.split(',').at(-1).replace(')','')):1})()`);
   assert.equal(actualAlpha,1-value/100,'actual panel background, not only its CSS variable, must follow the slider');
   await screenshot(`appearance-${theme}-${value}.png`);
  }
  // Keyboard edits remain range input edits, rather than rail navigation.
  await evaluate(`document.querySelector('.appearance-slider input').focus()`);
  for(const type of ['keyDown','keyUp'])await page('Input.dispatchKeyEvent',{type,key:'ArrowLeft',code:'ArrowLeft',windowsVirtualKeyCode:37});
  await waitFor(`localStorage.getItem('fixture-transparency')==='99'`);
  await evaluate(`window.appearanceFixture={delayMs:150,fail:false};document.querySelector('.toggle-setting input').click()`);
  await set(25);await set(75);await set(35);await waitFor(`localStorage.getItem('fixture-transparency')==='35' && !document.querySelector('.save-settings').disabled`);
  assert.equal(await evaluate(`document.querySelector('.toggle-setting input').checked`),true,'appearance save preserves unrelated local draft');
  await evaluate(`window.appearanceFixture.fail=true`);await set(80);
  await waitFor(`document.querySelector('.appearance-error') && document.querySelector('.appearance-slider input').value==='35'`);
  assert.equal(await evaluate(`localStorage.getItem('fixture-transparency')`),'35');
  await page('Page.reload');await open();await waitFor(`document.querySelector('.appearance-slider input').value==='35'`);
  const sizes=await evaluate(`(()=>{const s=x=>getComputedStyle(document.querySelector(x)).fontSize;return [s('.settings-page:not([hidden]) h2'),s('.language-setting strong'),s('.toggle-setting span'),s('.language-setting small')]})()`);
  assert.deepEqual(sizes,['16px','13px','13px','12px']);
  await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:theme},{name:'prefers-reduced-transparency',value:'reduce'}]});
  assert.equal(await evaluate(`getComputedStyle(document.querySelector('.paste-shell')).getPropertyValue('--background-opacity').trim()`),'1');
 }
 // Main receives the saved setting without changing the rail frame.
 await page('Emulation.setDeviceMetricsOverride',{width:1440,height:148,deviceScaleFactor:1,mobile:false});
 await page('Emulation.setEmulatedMedia',{features:[]});
 await page('Page.navigate',{url:fixture+'/?fixture=visual&compact=1&window_role=main'});await waitFor(`document.querySelectorAll('.clip-card').length===8`);
 const bounds=await evaluate(`JSON.stringify(document.querySelector('.dock-surface').getBoundingClientRect())`);
 await evaluate(`localStorage.setItem('fixture-transparency','100');window.dragEventFixture.emit('pasters-preferences-changed',null)`);
 await waitFor(`getComputedStyle(document.querySelector('.paste-shell')).getPropertyValue('--background-opacity').trim()==='0'`);
 assert.equal(await evaluate(`JSON.stringify(document.querySelector('.dock-surface').getBoundingClientRect())`),bounds);
 assert.deepEqual(await fingerprint(),assets);
 await writeFile(join(artifacts,'appearance-report.json'),JSON.stringify({result:'passed',browser,assetSha256:assets,nativeEndToEnd:false,checks:['0/50/100 tint endpoints preserve foreground opacity','keyboard range edit','rapid edits coalesced','unrelated drafts retained','failed save rollback','reload persistence','shared preferences update rail without resize','settings text scale','reduced transparency override']},null,2)+'\n');
 console.log('Appearance checks passed.');
});
