// Compiled WASM with synthetic history only. No native clipboard writes.
import assert from 'node:assert/strict';
import {readFile,readdir,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {withCompiledUiTest} from './compiled-ui-test.mjs';
const dist=new URL('../apps/desktop/dist/',import.meta.url);
const fingerprint=async()=>Object.fromEntries(await Promise.all((await readdir(dist)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,dist))).digest('hex')])));
const assets=await fingerprint();
await withCompiledUiTest(async({page,evaluate,waitFor,fixture,screenshot,artifacts,browser})=>{
  const observations=[];
  for(const compact of [false,true]) for(const theme of ['light','dark']) {
    await page('Emulation.setDeviceMetricsOverride',{width:1100,height:compact?148:248,deviceScaleFactor:2,mobile:false});
    await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:theme}]});
    await page('Page.navigate',{url:`${fixture}/?fixture=pinboards${compact?'&compact=1':''}`});
    await waitFor(`document.querySelectorAll('.clip-card').length===5`);
    assert.equal(await evaluate(`!!document.querySelector('.toolbar .shortcut-hint,.toolbar .rail-guide,.toolbar .status-button,.toolbar .capture-status')`),false);
    assert.equal(await evaluate(`document.querySelector('.stack-button span').textContent`),'顺序粘贴');
    await evaluate(`document.querySelector('.stack-toggle').click()`);
    assert.equal(await evaluate(`document.querySelector('.stack-button strong').textContent`),'1');
    await evaluate(`document.querySelector('.stack-button').click()`);
    assert.equal(await evaluate(`document.querySelector('.stack-button strong').textContent`),'0');
    assert.equal(await evaluate(`document.querySelectorAll('.clip-card').length`),5,'clearing paste list never deletes history');
    const inspect=async(selector)=>{
      const g=await evaluate(`(()=>{const panel=document.querySelector('${selector}'),name=panel.querySelector('input[type=text]'),color=panel.querySelector('input[type=color]');const n=name.getBoundingClientRect(),c=color.getBoundingClientRect(),p=panel.getBoundingClientRect();return {name:{x:n.x,right:n.right,top:n.top,bottom:n.bottom},color:{x:c.x,right:c.right,top:c.top,bottom:c.bottom},panel:{x:p.x,right:p.right,bottom:p.bottom},nameLabel:name.labels[0].textContent,colorLabel:color.labels[0].textContent,width:innerWidth,height:innerHeight,overflow:panel.scrollWidth>panel.clientWidth+1}})()`);
      assert.ok(g.name.right+6<=g.color.x,`separate name and color controls: ${JSON.stringify(g)}`);
      assert.ok(g.color.right<=g.panel.right && g.panel.x>=0 && g.panel.right<=g.width && g.panel.bottom<=g.height);
      assert.equal(g.overflow,false);
      assert.equal(g.nameLabel,'名称');assert.equal(g.colorLabel,'颜色');
      return g;
    };
    await evaluate(`document.querySelector('.add-pinboard').click()`);
    await waitFor(`document.querySelector('.pinboard-creator')`);
    const creator=await inspect('.pinboard-creator');
    await screenshot(`toolbar-create-${compact?'compact':'normal'}-${theme}.png`);
    await evaluate(`document.querySelector('.add-pinboard').click();[...document.querySelectorAll('.pinboard')].find(e=>e.textContent==='工作').click()`);
    await waitFor(`document.querySelector('.manage-pinboard')`);
    await evaluate(`document.querySelector('.manage-pinboard').click()`);
    await waitFor(`document.querySelector('.pinboard-editor')`);
    const editor=await inspect('.pinboard-editor');
    assert.equal(await evaluate(`document.querySelector('.pinboard-editor input[type=text]').value`),'工作');
    assert.equal(await evaluate(`document.querySelector('.pinboard-editor input[type=color]').value`),'#34c759');
    await screenshot(`toolbar-editor-${compact?'compact':'normal'}-${theme}.png`);
    await evaluate(`document.querySelector('.pinboard-editor header button').click();document.querySelector('.settings-button').click()`);
    await page('Emulation.setDeviceMetricsOverride',{width:1100,height:compact?508:608,deviceScaleFactor:2,mobile:false});
    await waitFor(`document.querySelector('.workspace-ready')`);
    assert.ok(await evaluate(`document.querySelector('.queue-help').textContent.includes('不会删除历史')`));
    await evaluate(`document.querySelectorAll('.settings-nav button')[1].click()`);
    assert.ok(await evaluate(`document.querySelector('.keyboard-reference').textContent.includes('Space') && document.querySelector('.keyboard-reference').textContent.includes('Return')`));
    assert.equal(await evaluate(`document.querySelector('.keyboard-reference').textContent.includes('暂停')`),false);
    await screenshot(`toolbar-keyboard-${compact?'compact':'normal'}-${theme}.png`);
    observations.push({compact,theme,creator,editor});
  }
  assert.deepEqual(await fingerprint(),assets);
  const report={result:'passed',assetSha256:assets,browser,nativeEndToEnd:false,systemClipboardUsed:false,observations};
  await writeFile(join(artifacts,'toolbar-settings-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report));
});
