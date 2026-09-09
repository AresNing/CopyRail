// Compiled Rust/WASM + synthetic IPC. Never register a real global shortcut.
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
  const key = async (key, code, virtual) => {
    // CDP leaves generated text empty by default. Native HTML button Return
    // activation needs the character-producing key event, not raw key codes only.
    for (const type of ['keyDown','keyUp']) await page('Input.dispatchKeyEvent',{type,key,code,windowsVirtualKeyCode:virtual,...(type==='keyDown' && key==='Enter' ? {text:'\r',unmodifiedText:'\r'} : {})});
  };
  for (const compact of [true, false]) for (const theme of ['dark', 'light']) {
    await page('Emulation.setDeviceMetricsOverride', {width:1440,height:compact ? 148 : 248,deviceScaleFactor:2,mobile:false});
    await page('Emulation.setEmulatedMedia', {features:[{name:'prefers-color-scheme',value:theme}]});
    await page('Page.navigate', {url:`${fixture}/?fixture=pinboards&shortcut_conflict=1${compact ? '&compact=1' : ''}`});
    await waitFor(`document.querySelectorAll('.clip-card').length === 5 && window.shortcutFixture.reads > 0`);
    assert.equal(await evaluate(`window.shortcutFixture.retries`),0,'startup status must not cause automatic re-registration');
    await evaluate(`document.querySelector('#history-search').focus()`);
    await page('Input.insertText',{text:'合成便签'});
    await waitFor(`document.querySelectorAll('.clip-card').length === 5 && document.querySelector('#history-search').value === '合成便签'`);
    assert.equal(await evaluate(`document.activeElement === document.querySelector('#history-search')`),true);
    await evaluate(`(() => {const input=document.querySelector('#history-search');input.value='';input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await waitFor(`document.querySelectorAll('.clip-card').length === 5`);
    await evaluate(`[...document.querySelectorAll('.pinboard')].find(e=>e.textContent.includes('工作')).click()`);
    await waitFor(`document.querySelectorAll('.clip-card').length === 1 && document.querySelector('.clip-card')?.textContent.includes('合成便签 E')`);
    await evaluate(`document.querySelector('.settings-button').focus()`);
    if(process.env.PASTERS_SHORTCUT_DIAGNOSTIC) await evaluate(`(() => {window.shortcutKeyTrace=[]; for(const type of ['keydown','keypress','keyup','click','focusin']) document.addEventListener(type,e=>window.shortcutKeyTrace.push({type,key:e.key,target:e.target.className,active:document.activeElement?.className,prevented:e.defaultPrevented}));})()`);
    await key('Enter','Enter',13);
    await page('Emulation.setDeviceMetricsOverride', {width:1440,height:compact ? 508 : 608,deviceScaleFactor:2,mobile:false});
    if(process.env.PASTERS_SHORTCUT_DIAGNOSTIC) {
      await evaluate(`new Promise(resolve=>setTimeout(resolve,250))`);
      console.log(JSON.stringify(await evaluate(`({trace:window.shortcutKeyTrace,settings:!!document.querySelector('.shortcut-settings'),active:document.activeElement?.outerHTML,commands:window.previewEditFixture.calls})`)));
      await screenshot('shortcut-keyboard-diagnostic.png');
    }
    await waitFor(`document.querySelector('.paste-shell.workspace-ready')`);
    await evaluate(`document.querySelectorAll('.settings-nav button')[1].click()`);
    await waitFor(`document.querySelector('.shortcut-settings') && document.activeElement === document.querySelector('.shortcut-settings')`);
    assert.equal(await evaluate(`document.querySelector('.shortcut-settings').textContent.includes('菜单栏「显示 CopyRail」')`),true);
    const geometry = await evaluate(`(() => {
      const hint=document.querySelector('.settings-button').getBoundingClientRect();const panel=document.querySelector('.settings-popover').getBoundingClientRect();
      return {hint:{x:hint.x,y:hint.y,right:hint.right,bottom:hint.bottom},panel:{x:panel.x,y:panel.y,right:panel.right,bottom:panel.bottom},width:innerWidth,height:innerHeight,scrollX,scrollY};
    })()`);
    await screenshot(`shortcut-fallback-${compact ? 'compact' : 'expanded'}-${theme}.png`);
    assert.ok(geometry.hint.x>=0 && geometry.hint.right<=geometry.width && geometry.hint.y>=0 && geometry.hint.bottom<=geometry.height,`status entry stays inside toolbar: ${JSON.stringify(geometry)}`);
    assert.ok(geometry.panel.x>=0 && geometry.panel.right<=geometry.width && geometry.panel.y>=0 && geometry.panel.bottom<=geometry.height,'settings panel stays in viewport');
    assert.equal(geometry.scrollY,0,'opening shortcut settings must not scroll the app offscreen');
    // In Compact the settings panel scrolls. Reach the actual retry button with
    // keyboard navigation, not a DOM click on a clipped, invisible control.
    for (let n=0;n<5 && !(await evaluate(`document.activeElement === document.querySelector('.shortcut-retry')`));n++) await key('Tab','Tab',9);
    assert.equal(await evaluate(`document.activeElement === document.querySelector('.shortcut-retry')`),true,'retry is keyboard reachable');
    const actionGeometry = await evaluate(`(() => {const r=document.querySelector('.shortcut-retry').getBoundingClientRect(),p=document.querySelector('.settings-popover').getBoundingClientRect();return {top:r.top,bottom:r.bottom,panelTop:p.top,panelBottom:p.bottom,scrollY};})()`);
    assert.ok(actionGeometry.top>=actionGeometry.panelTop && actionGeometry.bottom<=actionGeometry.panelBottom,'keyboard navigation exposes the full action button');
    assert.equal(actionGeometry.scrollY,0);
    await screenshot(`shortcut-fallback-action-${compact ? 'compact' : 'expanded'}-${theme}.png`);
    await key('Enter','Enter',13);
    await waitFor(`window.shortcutFixture.retries === 1 && !document.querySelector('.shortcut-retry')?.disabled`);
    assert.equal(await evaluate(`document.querySelector('.shortcut-state').textContent`),'未启用','conflict remains explicit after a failed attempt');
    assert.equal(await evaluate(`window.previewEditFixture.calls.filter(c=>c.command.startsWith('restore_clip')).length`),0,'toolbar/retry Enter must not paste the selected card');
    // A delayed old read must not overwrite the result of a newer retry.
    const reads = await evaluate(`window.shortcutFixture.reads`);
    await evaluate(`window.shortcutFixture.readDelayMs=1200;document.querySelector('.shortcut-refresh').click()`);
    await waitFor(`window.shortcutFixture.reads > ${reads}`);
    await evaluate(`(() => {const control=window.shortcutFixture;control.readDelayMs=0;control.failRegistration=false;control.retryDelayMs=350;const button=document.querySelector('.shortcut-retry');button.click();button.dispatchEvent(new MouseEvent('click',{bubbles:true}));})()`);
    await waitFor(`document.querySelector('.shortcut-retry')?.disabled && document.querySelector('.shortcut-state')?.textContent === '处理中'`);
    assert.equal(await evaluate(`window.shortcutFixture.retries`),2,'pending retry is not submitted twice');
    await waitFor(`document.querySelector('.shortcut-state')?.textContent === '已注册'`);
    await evaluate(`new Promise(resolve=>setTimeout(resolve,1400))`);
    assert.equal(await evaluate(`document.querySelector('.shortcut-state').textContent`),'已注册','stale failed status cannot replace successful retry');
    assert.equal(await evaluate(`document.querySelector('.shortcut-retry').disabled`),true);
    assert.equal(await evaluate(`!!document.querySelector('.shortcut-details')`),false);
    // A failed IPC request stays recoverable and does not remove the current board.
    await evaluate(`window.shortcutFixture.registered=false;window.shortcutFixture.failRequest=true;document.querySelector('.shortcut-refresh').click()`);
    await waitFor(`document.querySelector('.shortcut-state')?.textContent === '未启用' && !document.querySelector('.shortcut-retry')?.disabled`);
    await evaluate(`document.querySelector('.shortcut-retry').click()`);
    await waitFor(`document.querySelector('.shortcut-request-error')?.textContent.includes('Synthetic retry request failed') && !document.querySelector('.shortcut-retry')?.disabled`);
    assert.equal(await evaluate(`document.querySelectorAll('.clip-card').length`),1);
    await evaluate(`window.shortcutFixture.failRequest=false;document.querySelector('.shortcut-retry').click()`);
    await waitFor(`document.querySelector('.shortcut-state')?.textContent === '已注册' && !document.querySelector('.shortcut-request-error')`);
    observations.push({compact,theme,geometry,actionGeometry,retries:await evaluate(`window.shortcutFixture.retries`),checks:['history/search/board remain usable','keyboard-reachable fallback and retry do not paste','explicit fallback instructions','duplicate retry prevented','stale read rejected','registration and request failures recover']});
  }
  await page('Emulation.setDeviceMetricsOverride',{width:1440,height:148,deviceScaleFactor:2,mobile:false});
  await page('Page.navigate',{url:`${fixture}/?fixture=visual&compact=1`});
  await waitFor(`document.querySelector('.native-test-badge') && window.shortcutFixture.reads > 0`);
  await evaluate(`document.querySelector('.settings-button').click()`);
  await page('Emulation.setDeviceMetricsOverride',{width:1440,height:508,deviceScaleFactor:2,mobile:false});
  await waitFor(`document.querySelector('.workspace-ready')`);
  await evaluate(`document.querySelectorAll('.settings-nav button')[1].click()`);
  await waitFor(`document.querySelector('.shortcut-state')?.textContent === '隔离未注册'`);
  assert.equal(await evaluate(`document.querySelector('.shortcut-retry').disabled`),true);
  await evaluate(`document.querySelector('.shortcut-retry').dispatchEvent(new MouseEvent('click',{bubbles:true}))`);
  assert.equal(await evaluate(`window.shortcutFixture.retries`),0,'isolation must not request registration');
  assert.deepEqual(await fingerprint(),assets);
  const report={result:'passed',browser,nativeEndToEnd:false,systemClipboardUsed:false,systemShortcutRegistered:false,assetSha256:assets,observations,isolatedRegistrationDenied:true};
  await writeFile(join(artifacts,'shortcut-fallback-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
