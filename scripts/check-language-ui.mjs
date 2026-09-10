// Compiled UI, fresh browser profile and synthetic data; no native clipboard access.
import assert from 'node:assert/strict';
import {writeFile,readFile,readdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {withCompiledUiTest} from './compiled-ui-test.mjs';
const dist=new URL('../apps/desktop/dist/',import.meta.url);
const fingerprint=async()=>Object.fromEntries(await Promise.all((await readdir(dist)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,dist))).digest('hex')])));
const assets=await fingerprint();
await withCompiledUiTest(async({page,evaluate,waitFor,fixture,artifacts,screenshot,browser})=>{
  const size=height=>page('Emulation.setDeviceMetricsOverride',{width:1100,height,deviceScaleFactor:2,mobile:false});
  const openSettings=async()=>{await evaluate(`document.querySelector('.settings-button').click()`);await waitFor(`window.previewFrameFixture.open`);await size(608);await waitFor(`document.querySelector('.workspace-ready') && document.querySelector('.language-select')`);};
  const choose=async(code)=>evaluate(`(()=>{const s=document.querySelector('.language-select');s.value=${JSON.stringify(code)};s.dispatchEvent(new Event('change',{bubbles:true}))})()`);
  const englishCopy=async(selector)=>{
    const remaining=await evaluate(`(()=>{const scope=document.querySelector(${JSON.stringify(selector)});return [...scope.querySelectorAll('*')].filter(e=>e.checkVisibility() && !e.closest('option,.sr-only,[aria-hidden="true"]')).flatMap(e=>[...e.childNodes].filter(n=>n.nodeType===3 && /[\\u3400-\\u9fff]/.test(n.data)).map(n=>n.data))})()`);
    assert.deepEqual(remaining,[],`English app copy: ${selector}`);
  };
  const observations=[];
  for(const theme of ['dark','light']) {
    await size(248);await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:theme}]});
    await page('Page.navigate',{url:fixture+'/?fixture=readme'});
    await waitFor(`document.querySelectorAll('.clip-card').length===6`);
    await openSettings();
    if(await evaluate(`document.documentElement.lang==='en'`)) {await choose('zh-CN');await waitFor(`document.documentElement.lang==='zh-CN'`);}
    // Preserve settings drafts and DOM identity across a successful locale change.
    await evaluate(`(()=>{window.oldLanguageSelect=document.querySelector('.language-select');window.oldSettings=document.querySelector('.settings-popover');if(!document.querySelector('[data-settings-page="general"] input[type="checkbox"]').checked)document.querySelector('[data-settings-page="general"] input[type="checkbox"]').click();window.languageFixture.delayMs=400;})()`);
    await choose('en');
    await waitFor(`document.querySelector('.language-select').disabled`);
    await waitFor(`document.documentElement.lang==='en' && !document.querySelector('.language-select').disabled`);
    assert.equal(await evaluate(`document.querySelector('.language-select')===window.oldLanguageSelect && document.querySelector('.settings-popover')===window.oldSettings`),true);
    assert.equal(await evaluate(`document.querySelector('[data-settings-page="general"] input[type="checkbox"]').checked`),true);
    assert.equal(await evaluate(`document.querySelector('.search-field input, input[type="search"]').placeholder`),'');
    for(let tab=0;tab<5;tab++){
      await evaluate(`document.querySelectorAll('.settings-nav button')[${tab}].click()`);
      await englishCopy(`.settings-page:not([hidden])`);
    }
    await evaluate(`document.querySelectorAll('.settings-nav button')[0].click()`);
    // A rejected write restores the select and leaves the previous language saved.
    await evaluate(`window.languageFixture.fail=true`);await choose('zh-CN');
    await waitFor(`document.querySelector('.language-error') && !document.querySelector('.language-select').disabled`);
    assert.equal(await evaluate(`document.querySelector('.language-select').value`),'en');
    assert.equal(await evaluate(`localStorage.getItem('fixture-language')`),'en');
    await evaluate(`window.languageFixture.fail=false`);
    await page('Page.reload');await waitFor(`document.documentElement.lang==='en' && document.querySelectorAll('.clip-card').length===6`);
    await size(248);await englishCopy('.paste-shell');
    // Runtime translations must not alter user content even when it matches a label.
    await evaluate(`window.sourceIconFixture.clips[0].title='设置';window.sourceIconFixture.clips[0].searchable_text='通用';`);
    await waitFor(`document.querySelector('.card-title').textContent==='设置'`);
    assert.equal(await evaluate(`document.querySelector('.card-title').textContent`),'设置');
    await openSettings();await choose('zh-CN');await waitFor(`document.documentElement.lang==='zh-CN' && !document.querySelector('.language-select').disabled`);
    assert.equal(await evaluate(`document.querySelector('.settings-nav button').textContent`),'通用');
    assert.equal(await evaluate(`document.querySelector('.card-title').textContent`),'设置');
    await screenshot(`language-${theme}-zh.png`);
    observations.push({theme,immediate:true,persistedAcrossReload:true,draftPreserved:true,failedSaveRolledBack:true,userContentPreserved:true});
    assert.equal(await evaluate(`window.previewEditFixture.calls.length`),0);
  }
  // First launch and pre-language installations inherit the PRIMARY OS language.
  for(const [systemLocale,effective] of [['en-US','en'],['zh-Hans-CN','zh-CN'],['zh-Hant-TW','zh-CN'],['ja-JP','en']]) {
    await evaluate(`localStorage.removeItem('fixture-language')`);
    await size(248);
    await page('Page.navigate',{url:fixture+'/?fixture=readme&system_locale='+systemLocale});
    await waitFor(`document.querySelectorAll('.clip-card').length===6 && document.documentElement.lang===${JSON.stringify(effective)}`);
    await openSettings();
    assert.equal(await evaluate(`document.querySelector('.language-select').value`),'system');
    assert.equal(await evaluate(`localStorage.getItem('fixture-language')`),null,'resolving the OS language must not pin an explicit override');
    const override=effective==='en'?'zh-CN':'en';
    await choose(override);await waitFor(`document.documentElement.lang===${JSON.stringify(override)} && !document.querySelector('.language-select').disabled`);
    await page('Page.reload');await waitFor(`document.querySelectorAll('.clip-card').length===6 && document.documentElement.lang===${JSON.stringify(override)}`);
    await size(248);await openSettings();
    assert.equal(await evaluate(`document.querySelector('.language-select').value`),override);
    await choose('system');await waitFor(`document.documentElement.lang===${JSON.stringify(effective)} && !document.querySelector('.language-select').disabled`);
    assert.equal(await evaluate(`localStorage.getItem('fixture-language')`),'system');
    // A changed system language takes effect on the next launch, without a rewrite.
    const changed=effective==='en'?'zh-CN':'en-US';
    await page('Page.navigate',{url:fixture+'/?fixture=readme&system_locale='+changed});
    await waitFor(`document.querySelectorAll('.clip-card').length===6 && document.documentElement.lang===${JSON.stringify(override)}`);
    assert.equal(await evaluate(`localStorage.getItem('fixture-language')`),'system');
    observations.push({systemLocale,effective,defaultIsSystem:true,manualOverridePersisted:true,returnToSystem:true,systemChangeOnRelaunch:true});
  }
  assert.deepEqual(await fingerprint(),assets);
  await writeFile(join(artifacts,'language-report.json'),JSON.stringify({result:'passed',browser,assetSha256:assets,observations,syntheticData:true,nativeEndToEnd:false},null,2)+'\n');
  console.log(JSON.stringify({result:'passed',observations}));
});
