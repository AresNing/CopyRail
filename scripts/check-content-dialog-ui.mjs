// Real compiled Rust/WASM; synthetic command responses, never native/clipboard.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const dist = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(dist)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort().map(async n => [n, createHash('sha256').update(await readFile(new URL(n, dist))).digest('hex')])));
const assets = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const observations = [];
  const key = async (key, code, virtual, modifiers = 0) => {
    for (const type of ['keyDown', 'keyUp']) await page('Input.dispatchKeyEvent', {type, key, code, windowsVirtualKeyCode:virtual, modifiers});
  };
  const fill = value => evaluate(`(() => {const e=document.querySelector('.content-title-field input');e.value=${JSON.stringify(value)};e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  for (const compact of [true, false]) for (const theme of ['dark', 'light']) {
    await page('Emulation.setDeviceMetricsOverride', {width:1440,height:compact ? 148 : 248,deviceScaleFactor:2,mobile:false});
    await page('Emulation.setEmulatedMedia', {features:[{name:'prefers-color-scheme',value:theme}]});
    await page('Page.navigate', {url:`${fixture}/?fixture=pinboards${compact ? '&compact=1' : ''}`});
    await waitFor(`document.querySelectorAll('.clip-card').length===5`);
    await evaluate(`(() => {
      const invoke=window.__TAURI__.core.invoke;window.dialogFixture={writes:[],fail:false};
      window.__TAURI__.core.invoke=async (command,args) => {
        if(command==='show_clip_context_menu') return {choice:{action:'rename'},item:structuredClone(window.sourceIconFixture.clips.find(c=>c.id===args.request.clip_ids[0]))};
        if(command==='rename_clip') {if(window.dialogFixture.fail) throw 'Synthetic save failure';window.dialogFixture.writes.push(args.request);window.sourceIconFixture.clips.find(c=>c.id===args.request.clip_id).title=args.request.title;return null;}
        return invoke(command,args);
      };
      document.querySelectorAll('.clip-card')[3].click();
      document.querySelectorAll('.clip-card')[3].dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:570,clientY:80}));
    })()`);
    await waitFor(`document.querySelector('.content-title-field input')`);
    const initial = await evaluate(`({focused:document.activeElement===document.querySelector('.content-title-field input'),tag:document.activeElement.tagName,title:document.querySelector('.content-title-field input').value})`);
    observations.push({compact,theme,initial});
    await screenshot(`content-dialog-initial-${compact ? 'compact' : 'expanded'}-${theme}.png`);
    assert.equal(initial.focused,true,'rename opens with title focus, not the background card');
    assert.equal(await evaluate(`document.querySelector('dialog')?.matches(':modal')`),true,'semantic modal must make background inert');
    await evaluate(`document.querySelector('#history-search').focus()`);
    assert.equal(await evaluate(`document.querySelector('.content-editor').contains(document.activeElement)`),true,'background cannot steal focus');
    for (let n=0;n<10;n++) {
      await key('Tab','Tab',9,n<5 ? 0 : 8);
      const focus = await evaluate(`({inside:document.querySelector('.content-editor').contains(document.activeElement),tag:document.activeElement.tagName,label:document.activeElement.getAttribute('aria-label'),className:document.activeElement.className})`);
      assert.equal(focus.inside,true,`both Tab directions stay in modal: ${JSON.stringify({compact,theme,n,focus})}`);
    }
    await evaluate(`document.querySelector('.content-title-field input').focus()`);
    await fill('Unsaved draft');
    await key('Escape','Escape',27);
    await waitFor(`!document.querySelector('.content-editor')`);
    assert.equal(await evaluate(`document.activeElement?.matches('#history-results,.clip-card')`),true,'cancel returns focus to the invoking list');
    assert.equal(await evaluate(`window.dialogFixture.writes.length`),0);
    await evaluate(`document.querySelector('[title="重命名选中项目（⌘R）"]').click()`);
    await waitFor(`document.activeElement===document.querySelector('.content-title-field input')`);
    assert.equal(await evaluate(`document.querySelector('.content-title-field input').value`),initial.title,'cancel does not persist draft');
    await fill('');
    await evaluate(`document.querySelector('.save-content').click()`);
    await waitFor(`document.querySelector('.content-editor [role=alert]')?.textContent.includes('标题不能为空')`);
    await fill('Saved synthetic title');
    await evaluate(`window.dialogFixture.fail=true;document.querySelector('.save-content').click()`);
    await waitFor(`document.querySelector('.content-editor [role=alert]')?.textContent.includes('Synthetic save failure')`);
    assert.equal(await evaluate(`document.querySelector('dialog').matches(':modal')`),true,'failed save keeps modal and draft');
    await evaluate(`window.dialogFixture.fail=false;document.querySelector('.save-content').click()`);
    await waitFor(`!document.querySelector('.content-editor') && window.dialogFixture.writes.length===1`);
    assert.equal(await evaluate(`document.activeElement?.matches('#history-results,.clip-card,button')`),true);
    await evaluate(`document.querySelector('[title="新建文本、链接或颜色"]').click()`);
    await waitFor(`document.activeElement===document.querySelector('.content-editor textarea')`);
    await key('Escape','Escape',27);
    await waitFor(`!document.querySelector('.content-editor')`);
  }
  assert.deepEqual(await fingerprint(),assets);
  const report={result:'passed',browser,nativeEndToEnd:false,voiceOverSpeech:false,assetSha256:assets,observations,checks:['menu rename and new editor initial focus','browser modal background inertness','bidirectional tab containment','cancel discards draft and restores invoking list','validation and failed saves visible inside modal','successful rename closes and restores focus','light/dark and Compact/expanded']};
  await writeFile(join(artifacts,'content-dialog-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
