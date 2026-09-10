// Production English locale and 100% transparency, synthetic backdrop/content, isolated IPC.
// This is a browser screenshot check, not native acceptance.
import assert from 'node:assert/strict';
import {readFile,readdir,mkdir,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {withCompiledUiTest} from './compiled-ui-test.mjs';
const dist=new URL('../apps/desktop/dist/',import.meta.url);
const fingerprint=async()=>Object.fromEntries(await Promise.all((await readdir(dist)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,dist))).digest('hex')])));
const assets=await fingerprint();
await withCompiledUiTest(async({page,evaluate,waitFor,fixture,artifacts,browser})=>{
  const out=new URL('../docs/images/',import.meta.url);await mkdir(out,{recursive:true});
  const frames=[];
  const size=(height)=>page('Emulation.setDeviceMetricsOverride',{width:1440,height,deviceScaleFactor:2,mobile:false});
  const capture=async(name,selector)=>{
    assert.equal(await evaluate(`localStorage.getItem('fixture-transparency')`),'100');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.paste-shell')).getPropertyValue('--background-opacity').trim()`),'0');
    const surface=selector??(name==='preview-dark.png'?'.preview-overlay':'.dock-surface');
    const surfaceColor=await evaluate(`getComputedStyle(document.querySelector(${JSON.stringify(surface)})).backgroundColor`);
    assert.match(surfaceColor,/^rgba\([^)]*,\s*0\)$/,'demo surface must actually be transparent');
    const untranslated=await evaluate(`(()=>{const scope=document.querySelector(${JSON.stringify(selector??'.paste-shell')});return [...scope.querySelectorAll('*')].filter(e=>e.checkVisibility() && !e.closest('[aria-hidden="true"],.sr-only,option')).flatMap(e=>[...e.childNodes].filter(n=>n.nodeType===3 && /[\\u3400-\\u9fff]/.test(n.data)).map(n=>n.data))})()`);
    assert.deepEqual(untranslated,[],'visible screenshot copy must be English');
    let clip;
    if(selector) clip=await evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height,scale:1}})()`);
    const image=await page('Page.captureScreenshot',{format:'png',...(clip?{clip}:{})});
    await writeFile(new URL(name,out),Buffer.from(image.data,'base64'));frames.push({name,clip:clip??null,surfaceColor,backgroundTransparency:100});
  };
  await size(248);
  await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'dark'}]});
  await page('Page.navigate',{url:fixture+'/?fixture=readme'});
  await waitFor(`document.querySelectorAll('.clip-card').length===6`);
  // Documentation-only backdrop behind the app, not a replacement for its surfaces
  // or a capture of macOS vibrancy. Never touch the user's desktop or preferences.
  await evaluate(`document.documentElement.style.background='radial-gradient(ellipse at 15% 10%, #3e4c60 0%, transparent 65%), radial-gradient(ellipse at 85% 80%, #46404f 0%, transparent 60%), linear-gradient(135deg, #243341, #242b36)'`);
  await evaluate(`document.querySelector('.settings-button').click()`);
  await waitFor(`window.previewFrameFixture.open`);await size(608);
  await waitFor(`document.querySelector('.workspace-ready')`);
  await evaluate(`(()=>{const select=document.querySelector('.language-select');select.value='en';select.dispatchEvent(new Event('change',{bubbles:true}))})()`);
  await waitFor(`document.documentElement.lang==='en' && !document.querySelector('.language-select').disabled`);
  await evaluate(`(()=>{const slider=document.querySelector('.appearance-slider input');slider.value='100';slider.dispatchEvent(new Event('input',{bubbles:true}))})()`);
  await waitFor(`localStorage.getItem('fixture-transparency')==='100' && document.querySelector('.appearance-slider input').value==='100'`);
  await capture('language-dark.png','.settings-popover');
  await evaluate(`document.querySelector('.settings-popover header button').click()`);
  await waitFor(`!window.previewFrameFixture.open`);await size(248);
  await waitFor(`document.querySelector('.clip-card img')?.complete`);
  await capture('clipboard-dark.png');
  await evaluate(`document.querySelector('#history-results').focus()`);
  for(const type of ['keyDown','keyUp'])await page('Input.dispatchKeyEvent',{type,key:' ',code:'Space',windowsVirtualKeyCode:32});
  await waitFor(`window.previewFrameFixture.open`);await size(608);
  await waitFor(`document.querySelector('.workspace-ready') && document.activeElement?.classList.contains('preview-overlay')`);
  await capture('preview-dark.png');
  await evaluate(`window.dragEventFixture.emit('pasters-close-preview',null)`);
  await waitFor(`!document.querySelector('.preview-overlay') && !window.previewFrameFixture.open`);await size(248);
  await evaluate(`document.querySelector('.settings-button').click()`);
  await waitFor(`window.previewFrameFixture.open`);await size(608);
  await waitFor(`document.querySelector('.workspace-ready')`);
  await evaluate(`document.querySelectorAll('.settings-nav button')[2].click()`);
  await capture('settings-dark.png','.settings-popover');
  assert.deepEqual(await fingerprint(),assets,'documentation rendering never modifies compiled application assets');
  assert.equal(await evaluate(`window.previewEditFixture.calls.length`),0);
  const report={result:'passed',browser,assetSha256:assets,frames,syntheticData:true,syntheticBackdrop:true,backgroundTransparency:100,productionTransparencySetting:true,productionLanguageSetting:true,nativeEndToEnd:false,systemClipboardUsed:false};
  await writeFile(join(artifacts,'readme-screenshots-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify({result:report.result,frames}));
});
