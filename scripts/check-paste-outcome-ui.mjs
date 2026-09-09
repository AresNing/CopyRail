// Real compiled Rust/WASM, synthetic outcomes only: no native clipboard or keys.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const dist = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(dist)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort().map(async n => [n,createHash('sha256').update(await readFile(new URL(n,dist))).digest('hex')])));
const assets = await fingerprint();
await withCompiledUiTest(async ({page,evaluate,waitFor,fixture,browser,artifacts,screenshot}) => {
  const key = async (key,code,virtual,modifiers=0) => {
    for (const type of ['keyDown','keyUp']) await page('Input.dispatchKeyEvent',{type,key,code,windowsVirtualKeyCode:virtual,modifiers});
  };
  const observations = [];
  for (const outcome of ['noTarget','clipboardChanged','permissionDenied','requested']) {
    await page('Emulation.setDeviceMetricsOverride',{width:1440,height:248,deviceScaleFactor:2,mobile:false});
    await page('Page.navigate',{url:`${fixture}/?fixture=pinboards`});
    await waitFor(`document.querySelectorAll('.clip-card').length===5`);
    await evaluate(`(() => {
      const invoke=window.__TAURI__.core.invoke;
      window.pasteOutcomeFixture={outcome:${JSON.stringify(outcome)},calls:[],permissionRequests:0};
      window.__TAURI__.core.invoke=async (command,args) => {
        if(command==='get_permission_status') return {accessibilityTrusted:false,appPath:'/Users/synthetic/Development/CopyRail/local-builds/CopyRail-0.1.0-local-beta.4/CopyRail.app'};
        if(command==='request_accessibility_permission') { window.pasteOutcomeFixture.permissionRequests++; return {accessibilityTrusted:false,appPath:'/Users/synthetic/Development/CopyRail/local-builds/CopyRail-0.1.0-local-beta.4/CopyRail.app'}; }
        if(!['restore_clip','restore_clips'].includes(command))return invoke(command,args);
        const state=window.pasteOutcomeFixture;state.calls.push({command,args:structuredClone(args)});
        if(args.request.paste && state.outcome==='clipboardChanged')throw {message:'系统剪贴板已变化，已取消自动粘贴；请重新选择内容。'};
        if(args.request.paste && state.outcome==='permissionDenied')throw {message:'内容已复制；自动粘贴需要在 macOS“系统设置 → 隐私与安全性 → 辅助功能”中允许 CopyRail。'};
        return {clipboardChangeCount:100,pasteRequested:args.request.paste && state.outcome==='requested'};
      };
      const cards=document.querySelectorAll('.clip-card');cards[0].click();
      cards[1].dispatchEvent(new MouseEvent('click',{bubbles:true,metaKey:true}));
    })()`);
    await waitFor(`document.querySelectorAll('.clip-card.selected').length===2`);
    await key('Enter','Enter',13,8);
    await waitFor(`window.pasteOutcomeFixture.calls.length===1`);
    let calls = await evaluate(`window.pasteOutcomeFixture.calls`);
    assert.equal(calls[0].command,'restore_clips');
    assert.equal(calls[0].args.request.clip_ids.length,2);
    assert.equal(calls[0].args.request.plain_text,true);
    assert.equal(calls[0].args.request.paste,true);
    if(outcome==='requested') {
      await waitFor(`!document.querySelector('.error-banner')`);
    } else {
      await waitFor(`document.querySelector('.error-banner')?.textContent.includes(${JSON.stringify(outcome==='noTarget'?'未确认当前目标':outcome==='permissionDenied'?'辅助功能':'剪贴板已变化')})`);
    }
    // A failed/copy-only Stack attempt must not consume its queued item.
    await evaluate(`document.querySelector('.clip-card').click();document.querySelector('.clip-card .stack-toggle').click();document.querySelector('#history-results').focus()`);
    await waitFor(`document.querySelector('.stack-button strong')?.textContent==='1'`);
    await key('Enter','Enter',13);
    await waitFor(`window.pasteOutcomeFixture.calls.length===2`);
    await waitFor(`document.querySelector('.stack-button strong')?.textContent===${JSON.stringify(outcome==='requested'?'0':'1')}`);
    calls = await evaluate(`window.pasteOutcomeFixture.calls`);
    assert.equal(calls[1].command,'restore_clip','one queued item uses the single-item route');
    assert.equal(calls[1].args.request.clip_id,calls[0].args.request.clip_ids[0]);
    assert.equal(calls[1].args.request.plain_text,false);
    // Quick Paste still uses the single-item command, with unchanged failure semantics.
    await key('1','Digit1',49,4);
    await waitFor(`window.pasteOutcomeFixture.calls.length===3`);
    assert.equal((await evaluate(`window.pasteOutcomeFixture.calls`))[2].command,'restore_clip');
    if(outcome==='permissionDenied') {
      await evaluate(`document.querySelector('.permission-help-button').click()`);
      await page('Emulation.setDeviceMetricsOverride',{width:1440,height:608,deviceScaleFactor:2,mobile:false});
      await waitFor(`document.activeElement?.getAttribute('aria-label')==='直接粘贴权限'`);
      await waitFor(`document.querySelector('.permission-app-path')?.textContent.includes('CopyRail-0.1.0-local-beta.4/CopyRail.app')`);
      assert.equal(await evaluate(`document.querySelector('.permission-help')?.textContent.includes('先退出 CopyRail') && document.querySelector('.permission-help')?.textContent.includes('“−”移除')`),true,'Stale grants need remove-then-add instructions, not another duplicate Add');
      assert.equal(await evaluate(`window.pasteOutcomeFixture.permissionRequests`),0,'Opening help must never grant/request permission or replay a paste');
      assert.equal((await evaluate(`window.pasteOutcomeFixture.calls`)).length,3);
      assert.equal(await evaluate(`scrollY===0 && scrollX===0 && document.querySelector('.settings-popover').scrollWidth <= document.querySelector('.settings-popover').clientWidth + 1`),true,'Permission help must stay inside the scrollable settings panel');
      await screenshot('paste-permission-help.png');
      await evaluate(`document.querySelector('.permission-actions button').scrollIntoView({block:'nearest'})`);
      assert.equal(await evaluate(`(() => {const b=document.querySelector('.permission-actions button').getBoundingClientRect(),p=document.querySelector('.settings-popover').getBoundingClientRect();return b.top>=p.top && b.bottom<=p.bottom && scrollY===0;})()`),true,'Permission controls remain reachable after long-path guidance');
      await evaluate(`document.querySelector('.settings-button').click();document.querySelector('#history-results').focus()`);
      await waitFor(`!window.previewFrameFixture.open`);
      await page('Emulation.setDeviceMetricsOverride',{width:1440,height:248,deviceScaleFactor:2,mobile:false});
    }
    // Ordinary Copy does not ask the OS to paste, regardless of the paste outcome.
    await evaluate(`window.dragEventFixture.emit('pasters-edit-action','copy')`);
    await waitFor(`window.pasteOutcomeFixture.calls.length===4`);
    assert.equal((await evaluate(`window.pasteOutcomeFixture.calls`))[3].args.request.paste,false);
    await waitFor(`document.querySelector('.notice-banner')?.textContent.includes('已复制')`);
    observations.push({outcome,calls:await evaluate(`window.pasteOutcomeFixture.calls`)});
  }
  assert.deepEqual(await fingerprint(),assets);
  const report={result:'passed',browser,nativeEndToEnd:false,systemClipboardUsed:false,assetSha256:assets,observations,checks:['batch plain paste request','no-target, permission-denied and changed-clipboard messages','permission help focuses settings and shows the running app path without requesting access or replaying paste','Stack retained on abort and consumed only on submission','single-item Quick Paste','ordinary Copy never requests automatic paste']};
  await writeFile(join(artifacts,'paste-outcome-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
