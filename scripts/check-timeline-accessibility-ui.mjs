// Compiled Rust/WASM, synthetic data, real browser accessibility tree. This
// does not claim native WebKit/VoiceOver speech or OS-key delivery acceptance.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const root = new URL('../apps/desktop/dist/', import.meta.url);
const fingerprint = async () => Object.fromEntries(await Promise.all((await readdir(root)).filter(n => /\.(wasm|css|js|html)$/.test(n)).sort().map(async n => [n, createHash('sha256').update(await readFile(new URL(n, root))).digest('hex')])));
const assets = await fingerprint();
await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const key = async (key, code, virtual, modifiers = 0) => {
    await page('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode: virtual, modifiers });
    await page('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: virtual, modifiers });
  };
  const selected = () => evaluate(`Array.from(document.querySelectorAll('.clip-card[aria-selected="true"]'), e=>e.id)`);
  const active = () => evaluate(`document.querySelector('#history-results').getAttribute('aria-activedescendant')`);
  const ax = async () => (await page('Accessibility.getFullAXTree')).nodes.filter(n => !n.ignored && ['grid','row','gridcell','button'].includes(n.role?.value));
  await page('Emulation.setDeviceMetricsOverride', { width: 1439, height: 248, deviceScaleFactor: 2, mobile: false });
  await page('Page.navigate', { url: `${fixture}/?fixture=visual` });
  await waitFor(`document.querySelectorAll('.clip-card').length===8`);
  await evaluate(`document.querySelector('.kind-text').click()`);
  assert.equal(await evaluate(`document.querySelector('#history-results').getAttribute('role')`), 'grid');
  assert.equal(await evaluate(`document.querySelector('.card-track').getAttribute('role')`), 'row');
  assert.equal(await evaluate(`document.querySelectorAll('.clip-card[role="gridcell"]').length`), 8);
  assert.equal(await evaluate(`document.querySelector('#history-results').getAttribute('aria-multiselectable')`), 'true');
  assert.equal((await selected()).length, 1);
  assert.equal(await active(), (await selected())[0]);
  const initialAx = await ax();
  assert.equal(initialAx.filter(n => n.role.value==='gridcell').length, 8);
  assert.ok(initialAx.some(n => n.role.value==='gridcell' && n.name?.value.includes('合成排版样本') && n.properties?.some(p => p.name==='selected' && p.value?.value===true)), 'selection and title must be exposed beyond CSS');
  assert.ok(initialAx.some(n => n.role.value==='gridcell' && n.description?.value.includes('来源：TextEdit') && n.description?.value.includes('字符')), 'source and summary must reach the accessibility tree');
  await key('ArrowRight', 'ArrowRight', 39, 8);
  await waitFor(`document.querySelectorAll('.clip-card[aria-selected="true"]').length===2`);
  assert.equal((await selected()).length, 2);
  assert.notEqual(await active(), (await selected())[0]);
  const selectionAx = await ax();
  assert.equal(selectionAx.filter(n => n.role.value==='gridcell' && n.properties?.some(p=>p.name==='selected'&&p.value?.value===true)).length, 2);
  await key('End', 'End', 35);
  assert.equal(await active(), await evaluate(`document.querySelector('.clip-card:last-child').id`));
  await key('Home', 'Home', 36, 8);
  assert.equal((await selected()).length, 8);
  const beforeVoKey = await active();
  await key('ArrowRight', 'ArrowRight', 39, 3); // Control+Option is reserved for VoiceOver.
  assert.equal(await active(), beforeVoKey);
  assert.equal((await selected()).length, 8);
  await key('Tab', 'Tab', 9);
  await waitFor(`document.activeElement?.id==='history-search'`);
  assert.equal(await active(), null, 'input focus must not leave a virtual card focus');
  await key('Tab', 'Tab', 9);
  await waitFor(`document.activeElement?.id==='history-results'`);
  assert.ok(await active());

  // A new clipboard item / order refresh must not silently retarget the
  // keyboard to a different record at the old array index.
  await evaluate(`document.querySelector('.kind-text').click()`);
  const reorderCard = await active();
  await key('F2', 'F2', 113);
  await waitFor(`document.activeElement?.classList.contains('stack-toggle')`);
  await evaluate(`window.sourceIconFixture.clips.reverse()`);
  await waitFor(`document.querySelector('.clip-card:last-child').id===${JSON.stringify(reorderCard)}`);
  await waitFor(`document.activeElement?.closest('.clip-card')?.id===${JSON.stringify(reorderCard)}`);
  await key('Escape', 'Escape', 27);
  assert.equal(await active(), reorderCard, 'order refresh preserves the focused record, not its old array index');
  assert.deepEqual(await selected(), [reorderCard]);
  await evaluate(`window.sourceIconFixture.clips.reverse()`);
  await waitFor(`document.querySelector('.clip-card').id===${JSON.stringify(reorderCard)}`);

  // Pinboard cards expose both locate and Stack actions, reached from F2.
  await evaluate(`Array.from(document.querySelectorAll('.pinboard')).find(e=>e.textContent==='收件箱').click()`);
  await waitFor(`document.querySelectorAll('.clip-card').length===4 && document.querySelector('.locate-button')`);
  await evaluate(`document.querySelector('.clip-card').click()`);
  const actionCard = await active();
  await key('F2', 'F2', 113);
  await waitFor(`document.activeElement?.classList.contains('locate-button')`);
  assert.equal(await active(), null);
  await key('ArrowRight', 'ArrowRight', 39);
  await waitFor(`document.activeElement?.classList.contains('stack-toggle')`);
  assert.equal((await selected())[0], actionCard, 'button arrows must not select a different card');
  await key(' ', 'Space', 32);
  await waitFor(`document.activeElement?.getAttribute('aria-pressed')==='true'`);
  assert.equal(await evaluate(`!!document.querySelector('.preview-overlay')`), false, 'Space activates the button, not Quick Look');
  await key('Tab', 'Tab', 9);
  await waitFor(`document.activeElement?.id==='history-results'`);
  assert.equal(await active(), actionCard);
  await key('F2', 'F2', 113);
  await key('Tab', 'Tab', 9, 8);
  await waitFor(`document.activeElement?.id==='history-results'`);
  await key('F2', 'F2', 113);
  await key('Escape', 'Escape', 27);
  await waitFor(`document.activeElement?.id==='history-results'`);
  assert.equal(await active(), actionCard);
  assert.equal(await evaluate(`Array.from(document.querySelectorAll('.pinboard.active')).some(e=>e.textContent==='收件箱')`), true);
  const buttonAx = await ax();
  assert.ok(buttonAx.some(n => n.role.value==='button' && n.name?.value.includes('顺序粘贴') && n.name?.value.includes('合成排版样本')));

  // Polling a title does not recreate the focused action. Removing that card
  // restores results focus instead of stranding keyboard focus on BODY.
  await key('F2', 'F2', 113);
  await evaluate(`window.a11yFocusedButton=document.activeElement; window.sourceIconFixture.clips[0].title='更新后的合成标题 🦀'`);
  await waitFor(`document.querySelector('.card-title').textContent==='更新后的合成标题 🦀'`);
  assert.equal(await evaluate(`document.activeElement===window.a11yFocusedButton`), true);
  assert.ok((await ax()).some(n=>n.role.value==='gridcell'&&n.name?.value.includes('更新后的合成标题 🦀')));
  await evaluate(`window.sourceIconFixture.clips.splice(0,1)`);
  try {
    await waitFor(`!document.getElementById(${JSON.stringify(actionCard)}) && document.activeElement?.id==='history-results'`);
  } catch (error) {
    console.error('Card-removal focus:', await evaluate(`({active:document.activeElement?.outerHTML, previousConnected:window.a11yFocusedButton?.isConnected, cards:Array.from(document.querySelectorAll('.clip-card'),e=>e.id), fixture:window.sourceIconFixture.clips.map(e=>e.id)})`));
    throw error;
  }
  assert.ok(await evaluate(`!!document.getElementById(document.querySelector('#history-results').getAttribute('aria-activedescendant'))`));
  const shot = await screenshot('timeline-accessibility-grid.png');
  await evaluate(`window.sourceIconFixture.clips.splice(0)`);
  await waitFor(`document.querySelector('.empty-state') && !document.querySelector('.clip-card')`);
  assert.equal(await active(), null);
  assert.equal(await evaluate(`document.querySelector('#history-results').getAttribute('role')`), 'region');
  assert.equal(await evaluate(`document.querySelector('#history-results').hasAttribute('aria-multiselectable')`), false);

  // Compact cards keep their small footprint, but F2 must reveal real,
  // visible actions instead of trying to focus a display:none button.
  const compactShots = [];
  for (const theme of ['light','dark']) {
    await page('Emulation.setEmulatedMedia', { features: [{ name:'prefers-color-scheme', value:theme }] });
    await page('Emulation.setDeviceMetricsOverride', { width:1439, height:148, deviceScaleFactor:2, mobile:false });
    await page('Page.navigate', { url:`${fixture}/?fixture=visual&compact=1` });
    await waitFor(`document.querySelectorAll('.clip-card').length===8 && document.querySelector('.paste-shell.compact')`);
    await evaluate(`document.querySelector('.clip-card').click()`);
    const size = await evaluate(`({width:document.querySelector('.clip-card').offsetWidth,height:document.querySelector('.clip-card').offsetHeight})`);
    await key('F2','F2',113);
    await waitFor(`document.activeElement?.classList.contains('stack-toggle') && document.activeElement.getClientRects().length>0`);
    assert.equal(await evaluate(`(() => { const b=document.activeElement.getBoundingClientRect(),c=document.activeElement.closest('.clip-card').getBoundingClientRect();return b.top>=c.top&&b.bottom<=c.bottom&&b.bottom<=innerHeight; })()`), true);
    assert.deepEqual(await evaluate(`({width:document.querySelector('.clip-card').offsetWidth,height:document.querySelector('.clip-card').offsetHeight})`), size);
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.clip-card .card-preview')).display`), 'none', 'action mode must not leave half-clipped preview lines');
    compactShots.push(await screenshot(`timeline-accessibility-compact-${theme}.png`));
    await key(' ', 'Space', 32);
    await waitFor(`document.activeElement?.getAttribute('aria-pressed')==='true'`);
    await key('Escape','Escape',27);
    await waitFor(`document.activeElement?.id==='history-results' && getComputedStyle(document.querySelector('.clip-card footer')).display==='none'`);
    assert.notEqual(await evaluate(`getComputedStyle(document.querySelector('.clip-card .card-preview')).display`), 'none', 'preview content returns after leaving action mode');
  }
  assert.deepEqual(await fingerprint(), assets);
  const report = { result:'passed', browser, nativeEndToEnd:false, voiceOverSpeech:false, assetSha256:assets,
    checks:['grid structure, labels, selection and active descendant','Home/End/Shift navigation and reserved VoiceOver modifiers','F2 action mode; button arrows, Space, Tab and Escape','focus survives metadata and reordering and restores after removal','empty results never expose dangling active descendants','Compact actions reveal on F2 and collapse on Escape without resizing cards'],
    accessibilityTrees:{initial:initialAx,selection:selectionAx,buttons:buttonAx}, screenshots:[shot,...compactShots] };
  await writeFile(join(artifacts,'timeline-accessibility-report.json'), JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify({result:report.result, browser, checks:report.checks, assetSha256:assets}));
});
