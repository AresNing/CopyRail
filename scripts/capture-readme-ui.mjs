// Documentation-only English presentation of the compiled UI. This is not a
// shipped locale or a native acceptance test. All content and IPC are synthetic.
import assert from 'node:assert/strict';
import {readFile,readdir,mkdir,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {withCompiledUiTest} from './compiled-ui-test.mjs';
const dist=new URL('../apps/desktop/dist/',import.meta.url);
const fingerprint=async()=>Object.fromEntries(await Promise.all((await readdir(dist)).filter(n=>/\.(wasm|css|js|html)$/.test(n)).sort().map(async n=>[n,createHash('sha256').update(await readFile(new URL(n,dist))).digest('hex')])));
const assets=await fingerprint();
function englishDemo() {
  const samples=[
    ['text','Launch checklist','A small release, ready to ship.\n\n• Review the changelog\n• Check keyboard navigation\n• Prepare the screenshots\n• Publish the source'],
    ['link','Project handbook','https://example.com/handbook'],
    ['image','Mountain study','Generated illustration for the demo.'],
    ['color','Seafoam','#58AD97'],
    ['text','Rust snippet','fn main() {\n    println!("Hello, CopyRail!");\n}'],
    ['html','Release notes','CopyRail workspace\n\nKeep useful snippets close.\nOrganize by project.\nPick up where you left off.'],
  ];
  const f=window.sourceIconFixture;
  const template=f.clips[0];
  f.clips.splice(0,f.clips.length,...samples.map(([content_kind,title,searchable_text],i)=>({
    ...template,id:`10000000-0000-4000-8000-${String(i+1).padStart(12,'0')}`,
    content_kind,title,searchable_text,content_hash:Array(32).fill(i),
    source:{bundle_identifier:'test.synthetic',display_name:'Demo workspace'},
    captured_at:new Date(Date.now()-(i+1)*60000).toISOString(),
    last_copied_at:new Date(Date.now()-(i+1)*60000).toISOString(),
  })));
  const invoke=window.__TAURI__.core.invoke;
  window.__TAURI__.core.invoke=async(command,args)=>{
    const result=await invoke(command,args);
    if(command==='list_pinboards') return result.map((board,i)=>({...board,name:['Inbox','Work','Archive'][i]}));
    return result;
  };
  const strings={
    '收件箱':'Inbox','工作':'Work','归档':'Archive','搜索复制过的内容':'Search clipboard history','筛选':'Filter','编辑':'Edit','命名':'Rename','归类':'Organize','顺序粘贴':'Paste queue','隔离验证':'Demo',
    '设置':'Settings','通用':'General','快捷键':'Shortcuts','历史与隐私':'History & privacy','备份':'Backup','高级':'Advanced',
    '启动、显示与粘贴':'Startup, appearance and paste','登录时自动启动':'Launch at login','紧凑卡片布局':'Compact cards',
    '点卡片上的「＋」按顺序加入待粘贴列表。列表有内容时，回车优先粘贴第一条；再次唤起后可继续下一条。':'Use + on each card to build a paste queue. Return pastes the first item; reopen CopyRail for the next one.',
    '主界面的数字表示剩余条数。点击「顺序粘贴」清空列表，不会删除历史；仅复制或粘贴请求失败时不会移出该条。':'The count shows remaining items. Click Paste queue to clear the list without deleting history. Copy-only results and failed requests keep the item queued.',
    '直接粘贴':'Direct paste','复制无需授权；向目标应用发送 ⌘V 需要 macOS 辅助功能权限。':'Copying needs no permission. Direct paste requires macOS Accessibility access.',
    '待授权':'Not granted','授权故障排查':'Troubleshooting','授权直接粘贴':'Allow direct paste','重新检测':'Check again',
    '通用与隐私选项修改后保存':'Save after changing general or privacy preferences','保存设置':'Save changes',
    '保留范围与忽略规则':'Retention and excluded apps','剪贴板采集':'Clipboard capture',
    '暂停期间不保存新复制的内容，15 分钟后自动恢复。':'Pause stops saving new copies and resumes automatically after 15 minutes.',
    '隔离验证不采集系统剪贴板。':'Demo mode does not access the system clipboard.',
    '最多保留天数':'Keep history for (days)','永久':'Forever','最多保留未固定项目':'Maximum unpinned items','不限':'Unlimited',
    '忽略这些应用（每行一个 Bundle ID）':'Excluded apps (one bundle ID per line)','屏幕共享时隐藏内容':'Hide content while screen sharing',
    '固定到 Pinboard 的内容不会被保留策略清理。机密和瞬态剪贴板类型始终默认跳过。':'Pinned items are kept regardless of retention limits. Confidential and transient clipboard types are skipped by default.',
    'Esc 关闭 · Return 粘贴':'Esc to close · Return to paste','提取文字':'Extract text',
    '快速打开与键盘操作':'Open quickly and navigate with the keyboard','全局快捷键 ⇧⌘V':'Global shortcut ⇧⌘V',
    '隔离验证不会注册系统快捷键。':'Demo mode does not register a system shortcut.','隔离未注册':'Demo only','重新启用':'Enable again',
    '选择内容':'Select an item','预览 / 关闭预览':'Toggle preview','粘贴':'Paste','以纯文本粘贴':'Paste as plain text','复制':'Copy',
    '加入 / 移出顺序粘贴':'Add to / remove from queue','关闭预览、设置或主界面':'Close preview, settings or panel',
  };
  const translate=s=>strings[s] ?? s.replace(/(\d+) 字符/g,'$1 characters');
  const localize=()=>{
    const walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT);
    while(walker.nextNode()) {const n=walker.currentNode;if(n.parentElement?.closest('script,style'))continue;const value=translate(n.data);if(value!==n.data)n.data=value;}
    for(const e of document.querySelectorAll('[placeholder]')) {const old=e.getAttribute('placeholder'),value=translate(old);if(value!==old)e.setAttribute('placeholder',value);}
  };
  localize();new MutationObserver(localize).observe(document.body,{subtree:true,childList:true,characterData:true});
  document.documentElement.lang='en';
}
await withCompiledUiTest(async({page,evaluate,waitFor,fixture,artifacts,browser})=>{
  const out=new URL('../docs/images/',import.meta.url);await mkdir(out,{recursive:true});
  const frames=[];
  const size=(height)=>page('Emulation.setDeviceMetricsOverride',{width:1440,height,deviceScaleFactor:2,mobile:false});
  const capture=async(name,selector)=>{
    const untranslated=await evaluate(`(()=>{const scope=document.querySelector(${JSON.stringify(selector??'.paste-shell')});return [...scope.querySelectorAll('*')].filter(e=>e.checkVisibility() && !e.closest('[aria-hidden="true"],.sr-only')).flatMap(e=>[...e.childNodes].filter(n=>n.nodeType===3 && /[\\u3400-\\u9fff]/.test(n.data)).map(n=>n.data))})()`);
    assert.deepEqual(untranslated,[],'visible screenshot copy must be English');
    let clip;
    if(selector) clip=await evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height,scale:1}})()`);
    const image=await page('Page.captureScreenshot',{format:'png',...(clip?{clip}:{})});
    await writeFile(new URL(name,out),Buffer.from(image.data,'base64'));frames.push({name,clip:clip??null});
  };
  await size(248);
  await page('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'dark'}]});
  await page('Page.navigate',{url:fixture+'/?fixture=visual'});
  await waitFor(`document.querySelectorAll('.clip-card').length===8`);
  await evaluate(`(${englishDemo.toString()})()`);
  await waitFor(`document.querySelectorAll('.clip-card').length===6 && document.querySelector('.card-title').textContent==='Launch checklist' && [...document.querySelectorAll('.pinboard')].some(e=>e.textContent==='Work')`);
  await waitFor(`document.querySelector('.clip-card img')?.complete`);
  await capture('clipboard-dark.png');
  await evaluate(`document.querySelector('#history-results').focus()`);
  for(const type of ['keyDown','keyUp'])await page('Input.dispatchKeyEvent',{type,key:' ',code:'Space',windowsVirtualKeyCode:32});
  await waitFor(`window.previewFrameFixture.open`);await size(608);
  await waitFor(`document.querySelector('.workspace-ready') && document.activeElement?.classList.contains('preview-overlay')`);
  await page('Emulation.setDefaultBackgroundColorOverride',{color:{r:20,g:20,b:20,a:1}});
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
  const report={result:'passed',browser,assetSha256:assets,frames,syntheticData:true,englishPresentationOnly:true,nativeEndToEnd:false,systemClipboardUsed:false};
  await writeFile(join(artifacts,'readme-screenshots-report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify({result:report.result,frames}));
});
