// Execute the same bounded read-only probe embedded in the debug native
// backend. This validates metadata collection, not WebKit behavior.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';
const probe = await readFile(new URL('../apps/desktop/src-tauri/src/native_tab_probe.js',import.meta.url),'utf8');
await withCompiledUiTest(async ({page,evaluate,waitFor,fixture,browser,artifacts}) => {
  await page('Emulation.setDeviceMetricsOverride',{width:1439,height:148,deviceScaleFactor:2,mobile:false});
  await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'dark'},{name:'prefers-reduced-motion',value:'no-preference'}]});
  await page('Page.navigate',{url:`${fixture}/?fixture=pinboards&compact=1`});
  await waitFor(`document.querySelectorAll('[data-drop-board]').length===3`);
  await page('Input.dispatchMouseEvent',{type:'mouseMoved',x:1400,y:135,button:'none',buttons:0});
  await waitFor(`document.querySelectorAll('.pinboard.active').length===1`);
  const before = await evaluate(probe);
  assert.equal(before.tab_count,4);
  assert.deepEqual(before.tabs.filter(t=>t.active).map(t=>t.ordinal),[0]);
  const begin = await evaluate(`(() => {document.querySelectorAll('[data-drop-board]')[1].click();return (${probe.trim()});})()`);
  await waitFor(`document.querySelectorAll('.clip-card').length===1`);
  await waitFor(`Array.from(document.querySelectorAll('.pinboard')).every(e=>e.getAnimations().length===0)`);
  const settled = await evaluate(probe);
  assert.deepEqual(settled.tabs.filter(t=>t.active).map(t=>t.ordinal),[2]);
  assert.deepEqual(settled.tabs.filter(t=>t.current).map(t=>t.ordinal),[2]);
  assert.deepEqual(settled.tabs[2].background,[55,55,55,1]);
  assert.deepEqual(settled.tabs[0].background,[0,0,0,0]);
  assert.ok(settled.timeline_ms > before.timeline_ms);
  const allowed = new Set(['visible','focused','now_ms','timeline_ms','tab_count','tabs','ordinal','active','current','hovered','background','animation_count','animations','background_transition','state','pending','current_ms','start_ms']);
  function checkShape(value) {
    if (Array.isArray(value)) return value.forEach(checkShape);
    if (value && typeof value==='object') return Object.entries(value).forEach(([k,v])=>{assert.ok(allowed.has(k),k);checkShape(v);});
    assert.ok(value===null || typeof value==='boolean' || (typeof value==='number' && Number.isFinite(value)),'no arbitrary strings in diagnostic payload');
  }
  [before,begin,settled].forEach(checkShape);
  const markup = await evaluate(`document.querySelector('.pinboards').outerHTML`);
  await evaluate(probe);
  assert.equal(await evaluate(`document.querySelector('.pinboards').outerHTML`),markup,'probe must not mutate DOM or selected classes');
  const report={result:'passed',browser,nativeEndToEnd:false,probeSha256:createHash('sha256').update(probe).digest('hex'),samples:{before,begin,settled},checks:['exact embedded probe returns bounded numeric/boolean metadata','current and active agree after selection','animation timeline advances and transition settles','probe does not mutate navigation DOM','native WebKit result remains unverified']};
  await writeFile(join(artifacts,'native-tab-probe-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
