// Test-only server for the actual compiled Leptos UI. It never opens a
// database, launches the desktop backend, accesses the clipboard or calls
// CloudKit. Fixture code is injected in memory, never into release assets.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve, basename } from 'node:path';

const assetRoot = fileURLToPath(new URL('../apps/desktop/dist/', import.meta.url));
const mime = { js: 'text/javascript', wasm: 'application/wasm', css: 'text/css', html: 'text/html' };
const tauriConfig = JSON.parse(await readFile(new URL('../apps/desktop/src-tauri/tauri.conf.json', import.meta.url)));
const imagePolicy = tauriConfig.app.security.csp.split(';').map(part => part.trim()).find(part => part.startsWith('img-src '));
const framePolicy = tauriConfig.app.security.csp.split(';').map(part => part.trim()).find(part => part.startsWith('frame-src '));
if (!imagePolicy) throw new Error('The real desktop image CSP directive is required for image tests');

function fixtureBootstrap(iconAssets, pdfAssets) {
  const pdfMode = new URLSearchParams(location.search).get('fixture') === 'pdf';
  const readmeMode = new URLSearchParams(location.search).get('fixture') === 'readme';
  const visualMode = readmeMode || pdfMode || new URLSearchParams(location.search).get('fixture') === 'visual';
  const pinboardMode = new URLSearchParams(location.search).get('fixture') === 'pinboards';
  const sharedMode = new URLSearchParams(location.search).get('fixture') === 'shared-conflicts';
  const uuid = index => `10000000-0000-4000-8000-${String(index).padStart(12, '0')}`;
  const now = new Date().toISOString();
  const clips = 'ABCDE'.split('').map((letter, index) => ({
    id: uuid(index + 1), captured_at: now, last_copied_at: now,
    source: { bundle_identifier: 'test.synthetic', display_name: 'Synthetic' },
    device: { id: uuid(99), display_name: 'Synthetic Mac' },
    content_kind: 'text', title: `合成便签 ${letter}`, searchable_text: `用于拖放验证的内容 ${letter}，不来自真实剪贴板。`,
    content_hash: Array(32).fill(index), representations: [],
  }));
  if (visualMode) {
    const template = clips[0];
    const samples = [
      ['text', '合成排版样本', '中文与 English 混排 🦀\n内容、来源、复制时间应分层展示。\n这不是用户剪贴板。\n最后一行用于检查底部裁切。'],
      ['link', 'Documentation', 'https://example.invalid/reference?fixture=visual'],
      ['image', 'Synthetic landscape.png', '合成图片，不读取本机照片。'],
      ['color', 'Sage green', '58aD97'],
      ['text', 'Rust code', 'fn main() {\n    println!("Hello, Rust 🦀");\n}\n// local fixture only'],
      ['text', 'RTL 与长单词', 'مرحبا بالعالم\nשלום עולם\nSupercalifragilisticexpialidocious'],
      ['text', '长标题应省略而不挤坏卡片的宽度 🦀', '<img src="never-load" onerror="throw 1">\n' + '长文本🦀'.repeat(80)],
      ['color', 'Midnight', '#17243B'],
    ];
    if (readmeMode) samples.splice(0, samples.length,
      ['text', 'Launch checklist', 'A small release, ready to ship.\n\n• Review the changelog\n• Check keyboard navigation\n• Prepare the screenshots\n• Publish the source'],
      ['link', 'Project handbook', 'https://example.com/handbook'],
      ['image', 'Mountain study', 'Generated illustration for the demo.'],
      ['color', 'Seafoam', '#58AD97'],
      ['text', 'Rust snippet', 'fn main() {\n    println!("Hello, CopyRail!");\n}'],
      ['html', 'Release notes', 'CopyRail workspace\n\nKeep useful snippets close.\nOrganize by project.\nPick up where you left off.'],
    );
    clips.splice(0, clips.length, ...samples.map(([content_kind, title, searchable_text], index) => ({
      ...template, id: uuid(index + 1), content_kind, title, searchable_text,
      captured_at: new Date(Date.now() - (index + 1) * 60_000).toISOString(),
      last_copied_at: new Date(Date.now() - (index + 1) * 60_000).toISOString(),
      content_hash: Array(32).fill(index),
    })));
    const sources = [
      ['com.apple.TextEdit', 'TextEdit'], ['com.apple.TextEdit', 'TextEdit'],
      ['com.apple.finder', 'Finder'], ['io.pasters.not-installed-fixture', 'Missing app'],
      ['io.pasters.broken-icon', 'Broken icon'], ['com.apple.TextEdit', 'TextEdit'],
      ['com.apple.finder', 'Finder'], ['com.apple.TextEdit', 'TextEdit'],
    ];
    clips.forEach((clip, i) => { clip.source = readmeMode ? { bundle_identifier: 'test.synthetic', display_name: 'Demo workspace' } : { bundle_identifier: sources[i][0], display_name: sources[i][1] }; });
  }
  window.sourceIconFixture = { calls: [], clips, delayMs: Number(new URLSearchParams(location.search).get('icon_delay') ?? 0), fail: new URLSearchParams(location.search).has('icon_fail'), iconAssets };
  if (pdfMode) {
    for (const [i, name] of ['portrait', 'rotated', 'unavailable'].entries()) {
      Object.assign(clips[i], { content_kind: 'pdf', title: `${name}.pdf`, searchable_text: '合成 PDF，完整文档有两页。' });
    }
  }
  window.pdfPreviewFixture = { calls: [], delayMs: 150, failDocument: false, assets: pdfAssets };
  window.previewFrameFixture = { calls: [], delayMs: 0, open: false };
  window.previewEditFixture = { calls: [] };
  const shortcutConflict = new URLSearchParams(location.search).has('shortcut_conflict');
  window.shortcutFixture = { isolated: visualMode, registered: !visualMode && !shortcutConflict, failRegistration: shortcutConflict, retryDelayMs: 0, readDelayMs: 0, retries: 0, reads: 0, failRead: false, failRequest: false };
  const boards = (readmeMode ? ['Inbox', 'Work', 'Archive'] : ['收件箱', '工作', '归档']).map((name, index) => ({
    id: uuid(index + 10), name, color: ['#ff9500', '#34c759', '#af52de'][index],
    sort_order: index, created_at: now, updated_at: now, is_shared: false, item_count: 0,
  }));
  const members = new Map([[uuid(10), clips.slice(0, 4).map(clip => clip.id)], [uuid(11), [uuid(5)]], [uuid(12), []]]);
  if (pinboardMode && new URLSearchParams(location.search).has('many_boards')) {
    for (let index = 3; index < 80; index++) {
      const board = { ...boards[0], id: uuid(index + 100), name: `分类 ${index + 1}`, sort_order: index, item_count: 0 };
      boards.push(board);
      members.set(board.id, []);
    }
  }
  window.__copyrailWorkspaceRole = new URLSearchParams(location.search).get('window_role') ?? 'embedded';
  window.workspaceFixture = { calls: [], state: { revision: 0, content: { kind: 'closed' } }, presented: false, delayMs: 0 };
  const listeners = new Map();
  const emit = (name, payload) => { for (const handler of listeners.get(name) ?? []) handler({ payload }); };
  const state = {
    sharedConflicts: sharedMode ? ['可编辑共享板', '只读共享板', '过期预览测试板'].map((pinboardName, index) => {
      const version = (title, deleted = false) => ({ title, preview: '<img src="never-load" onerror="throw 1">\n这只是合成文本，必须按文字显示。', deviceName: 'Synthetic Mac', deleted, timestampMs: Date.now() - 60_000 });
      return { id: `${uuid(40 + index)}/${uuid(50 + index)}/${uuid(60 + index)}`, pinboardName, canResolve: index !== 1,
        currentOperationId: uuid(70 + index), current: version('当前内容'), first: version('保留版本 A'), second: version('删除版本 B', true) };
    }) : [],
    sharedStaleInjected: false,
    conflicts: (sharedMode || visualMode ? [] : ['本机最近编辑的方案', '本机已删除的便签']).map((title, index) => ({
      id: `00000000-0000-4000-8000-00000000000${index + 1}`,
      localTitle: title,
      remoteTitle: index === 0 ? '另一台设备修改后的方案' : '另一台设备保留的便签',
      remoteWouldWin: index === 0,
      createdAtMs: Date.now() - (index + 1) * 60_000,
    })),
    enabled: !visualMode,
    pendingChanges: visualMode ? 0 : 3,
    pendingDependencies: visualMode ? 0 : 2,
    drag: null,
  };
  // A browser has no AppKit drag session. Only in this opt-in fixture, mouse
  // movement feeds the native bridge's event contract to the real Rust UI.
  window.dragEventFixture = { emit, current: () => structuredClone(state.drag), clear: () => { state.drag = null; } };
  document.addEventListener('mousemove', event => {
    if (state.drag) emit('pasters-internal-drag', { ...state.drag, phase: 'over', x: event.clientX, y: event.clientY });
  });
  document.addEventListener('mouseup', event => {
    if (!state.drag) return;
    emit('pasters-internal-drag', { ...state.drag, phase: 'drop', x: event.clientX, y: event.clientY });
    emit('pasters-drag-ended', { sessionId: state.drag.sessionId, cancelled: false });
    state.drag = null;
  });
  window.openingFixture = { calls: [], fail: false, delayMs: 0 };
  window.languageFixture = { calls: [], fail: false, delayMs: 0, systemLocale: new URLSearchParams(location.search).get('system_locale') ?? 'zh-CN' };
  const languageSettings = preference => ({ preference, effective: preference === 'system' ? (/^zh(?:[-_]|$)/i.test(window.languageFixture.systemLocale) ? 'zh-CN' : 'en') : preference });
  window.__TAURI__ = { event: { listen: async (name, handler) => {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name).add(handler);
    return () => listeners.get(name).delete(handler);
  } }, core: { invoke: async (command, args) => {
    switch (command) {
      case 'update_workspace': {
        window.workspaceFixture.calls.push({command,args:structuredClone(args)});
        if(window.workspaceFixture.delayMs)await new Promise(r=>setTimeout(r,window.workspaceFixture.delayMs));
        window.workspaceFixture.state={revision:window.workspaceFixture.state.revision+1,content:structuredClone(args.request)};
        if(args.request.kind==='closed')window.workspaceFixture.presented=false;
        return null;
      }
      case 'get_workspace': return structuredClone(window.workspaceFixture.state);
      case 'present_workspace': {
        window.workspaceFixture.calls.push({command,args:structuredClone(args)});
        if(args.revision===window.workspaceFixture.state.revision && window.workspaceFixture.state.content.kind!=='closed')window.workspaceFixture.presented=true;
        return null;
      }
      case 'dismiss_workspace': {
        window.workspaceFixture.calls.push({command,args});
        window.workspaceFixture.state={revision:window.workspaceFixture.state.revision+1,content:{kind:'closed'}};
        window.workspaceFixture.presented=false;
        emit('pasters-workspace',window.workspaceFixture.state);
        return null;
      }
      case 'workspace_key': window.workspaceFixture.calls.push({command,args:structuredClone(args)});return null;

      case 'perform_native_text_action':
      case 'undo_last_delete':
      case 'restore_clip':
      case 'restore_clips': {
        // Record routing only. These fixture responses cannot touch a system
        // clipboard or native responder, or mutate a real history database.
        window.previewEditFixture.calls.push({ command, args: structuredClone(args) });
        if (command === 'perform_native_text_action') return true;
        if (command === 'undo_last_delete') return [];
        throw { message: '合成复制路由已记录；不写入系统剪贴板。' };
      }
      case 'set_preview_window': {
        const control = window.previewFrameFixture;
        control.calls.push(args.open);
        if (control.delayMs) await new Promise(resolve => setTimeout(resolve, control.delayMs));
        control.open = args.open;
        return null;
      }
      case 'get_source_icons': {
        const control = window.sourceIconFixture;
        control.calls.push(structuredClone(args.request.clip_ids));
        const bundleIds = [...new Set(args.request.clip_ids.map(id => clips.find(clip => clip.id === id)?.source.bundle_identifier))];
        if (control.delayMs) await new Promise(resolve => setTimeout(resolve, control.delayMs));
        if (control.fail) throw { message: 'Synthetic icon lookup unavailable' };
        return bundleIds.map(bundleIdentifier => ({ bundleIdentifier, dataUrl: bundleIdentifier === 'io.pasters.broken-icon' ? 'data:image/png;base64,aGVsbG8=' : iconAssets[bundleIdentifier] ?? null }));
      }
      case 'get_capture_preferences':
        return JSON.parse(localStorage.getItem('fixture-capture') ?? 'null') ?? { retention: { max_age_days: null, max_unpinned_items: null }, excluded_bundle_ids: [] };
      case 'set_desktop_option':
      case 'update_capture_preferences': {
        const c=window.autoSaveFixture ??= {calls:[],delayMs:0,failDesktop:false,failCapture:false};
        c.calls.push({command,request:structuredClone(args.request)});
        if(c.delayMs)await new Promise(r=>setTimeout(r,c.delayMs));
        const desktop=command==='set_desktop_option';
        if(desktop?c.failDesktop:c.failCapture)throw {message:'Synthetic autosave failure'};
        if(desktop){
          const value=JSON.parse(localStorage.getItem('fixture-options') ?? '{}');
          value[args.request.option]=args.request.enabled;
          localStorage.setItem('fixture-options',JSON.stringify(value));
          emit('pasters-preferences-changed',null);
          return window.__TAURI__.core.invoke('get_desktop_preferences',{});
        }
        localStorage.setItem('fixture-capture',JSON.stringify(args.request));return args.request;
      }
      case 'set_opening_position':
        window.openingFixture.calls.push({command,args});
        if(window.openingFixture.delayMs)await new Promise(r=>setTimeout(r,window.openingFixture.delayMs));
        if(window.openingFixture.fail)throw {message:'Synthetic selection preference failure'};
        localStorage.setItem('fixture-opening',args.request);
        emit('pasters-preferences-changed',null);return args.request;
      case 'get_rail_opening': {
        window.openingFixture.calls.push({command,args});
        const value=[localStorage.getItem('fixture-opening') ?? 'latest',JSON.parse(localStorage.getItem('fixture-rail-position') ?? 'null')];
        if(window.openingFixture.delayMs)await new Promise(r=>setTimeout(r,window.openingFixture.delayMs));
        return value;
      }
      case 'save_rail_position':
        window.openingFixture.calls.push({command,args:structuredClone(args)});
        localStorage.setItem('fixture-rail-position',JSON.stringify(args.request));return null;
      case 'history_position': {
        const position=clips.findIndex(c=>c.id===args.request.clip_id);
        if(position<0)throw {message:'Synthetic missing item'};
        return {position};
      }
      case 'set_background_transparency':
        if(window.appearanceFixture?.delayMs)await new Promise(r=>setTimeout(r,window.appearanceFixture.delayMs));
        if(window.appearanceFixture?.fail)throw {message:'Synthetic appearance failure'};
        localStorage.setItem('fixture-transparency',String(args.request));
        emit('pasters-preferences-changed',null);return args.request;
      case 'get_desktop_preferences':
        return { opening_position: localStorage.getItem('fixture-opening') ?? 'latest', background_transparency: Number(localStorage.getItem('fixture-transparency') ?? 50), language: localStorage.getItem('fixture-language') ?? 'system', launch_at_login: false, screen_share_protection: false, compact_mode: (visualMode || pinboardMode) && new URLSearchParams(location.search).get('compact') === '1', ...JSON.parse(localStorage.getItem('fixture-options') ?? '{}') };
      case 'get_language_settings': return languageSettings(localStorage.getItem('fixture-language') ?? 'system');
      case 'set_language': {
        const control = window.languageFixture;
        control.calls.push(args.request);
        if (control.delayMs) await new Promise(resolve => setTimeout(resolve, control.delayMs));
        if (control.fail) throw { message: 'Synthetic language save failure' };
        localStorage.setItem('fixture-language', args.request);
        return languageSettings(args.request);
      }
      case 'get_permission_status': return { accessibilityTrusted: false };
      case 'get_shortcut_status': {
        const control = window.shortcutFixture;
        control.reads++;
        const result = { isolated: control.isolated, registered: control.registered, error: !control.isolated && !control.registered ? 'Synthetic shortcut registration failure' : null };
        if (control.readDelayMs) await new Promise(resolve => setTimeout(resolve, control.readDelayMs));
        if (control.failRead) throw { message: 'Synthetic status unavailable' };
        return result;
      }
      case 'retry_shortcut': {
        const control = window.shortcutFixture;
        control.retries++;
        if (control.isolated) throw { message: 'Isolated mode cannot register global shortcuts' };
        if (control.retryDelayMs) await new Promise(resolve => setTimeout(resolve, control.retryDelayMs));
        if (control.failRequest) throw { message: 'Synthetic retry request failed' };
        control.registered = !control.failRegistration;
        return { isolated: false, registered: control.registered, error: control.registered ? null : 'Synthetic shortcut registration failure' };
      }
      case 'list_history': return pinboardMode || visualMode ? structuredClone(clips.slice(args.request.offset ?? 0,(args.request.offset ?? 0)+(args.request.limit ?? 200))) : [];
      case 'search_history': {
        const ids = members.get(args.request.pinboard_id) ?? clips.map(clip => clip.id);
        return pinboardMode || visualMode ? ids.map(id => clips.find(clip => clip.id === id)).filter(clip => clip && (clip.title + clip.searchable_text).includes(args.request.text)) : [];
      }
      case 'list_pinboards': return pinboardMode || visualMode ? boards.map(board => ({ ...board, item_count: members.get(board.id).length })) : [];
      case 'get_clip_thumbnail':
      case 'get_clip_preview': {
        if (!visualMode) throw { message: '图片样本仅用于视觉测试。' };
        const clip = clips.find(clip => clip.id === args.request.clip_id);
        if (clip?.content_kind === 'pdf') {
          const control = window.pdfPreviewFixture;
          control.calls.push({ command, id: clip.id });
          await new Promise(resolve => setTimeout(resolve, control.delayMs));
          const name = clip.title.replace('.pdf', '');
          if (!pdfAssets[name]) throw { message: '合成 PDF 无法生成预览。' };
          if (command === 'get_clip_preview') {
            if (control.failDocument) throw { message: '合成 PDF 加载失败，可重试。' };
            return { mediaType: 'application/pdf', dataUrl: pdfAssets[name].pdf, pixelWidth: 0, pixelHeight: 0 };
          }
          return { mediaType: 'image/png', dataUrl: pdfAssets[name].png, pixelWidth: name === 'rotated' ? 600 : 400, pixelHeight: name === 'rotated' ? 400 : 600 };
        }
        // Code-generated fixture, not a photo or data from the user profile.
        const canvas = document.createElement('canvas');
        canvas.width = 320; canvas.height = 200;
        const context = canvas.getContext('2d');
        const sky = context.createLinearGradient(0, 0, 320, 200);
        sky.addColorStop(0, '#94d8f1'); sky.addColorStop(1, '#f9d7a4');
        context.fillStyle = sky; context.fillRect(0, 0, 320, 200);
        context.fillStyle = '#315b5a'; context.beginPath(); context.moveTo(0, 200); context.lineTo(125, 60); context.lineTo(220, 200); context.fill();
        context.fillStyle = '#567a68'; context.beginPath(); context.moveTo(100, 200); context.lineTo(270, 85); context.lineTo(320, 140); context.lineTo(320, 200); context.fill();
        return { mediaType: 'image/png', dataUrl: canvas.toDataURL('image/png'), pixelWidth: 320, pixelHeight: 200 };
      }
      case 'start_clip_drag': {
        if (!pinboardMode) throw { message: '拖放仅在 Pinboard 合成数据场景开放。' };
        state.drag = { sessionId: crypto.randomUUID(), clipIds: [...args.request.clip_ids] };
        return { itemCount: state.drag.clipIds.length };
      }
      case 'update_clip_drag_feedback': return true; // Native acceptance is covered separately.
      case 'place_pinboard_clips': {
        if (!pinboardMode) throw { message: '移动仅在 Pinboard 合成数据场景开放。' };
        const { pinboard_id: board, clip_ids: ids, anchor, after } = args.request;
        if (!members.has(board) || !ids.length || ids.some(id => !clips.some(clip => clip.id === id)) || (anchor && !members.get(board).includes(anchor))) throw { message: '无效的测试落点' };
        if (ids.includes(anchor)) return false;
        await new Promise(resolve => setTimeout(resolve, 400));
        for (const [key, value] of members) members.set(key, value.filter(id => !ids.includes(id)));
        const order = members.get(board);
        order.splice(anchor ? order.indexOf(anchor) + Number(after) : order.length, 0, ...ids);
        return true;
      }
      case 'reorder_pinboards': {
        if (!pinboardMode) throw { message: '排序仅在 Pinboard 合成数据场景开放。' };
        const ids = args.request.pinboard_ids;
        if (ids.length !== boards.length || new Set(ids).size !== boards.length || ids.some(id => !members.has(id))) throw { message: '无效的测试排序' };
        boards.sort((a, b) => ids.indexOf(a.id) - ids.indexOf(b.id));
        return boards.map((board, index) => ({ ...board, sort_order: index, item_count: members.get(board.id).length }));
      }
      case 'list_search_facets': return { sources: [], devices: [] };
      case 'capture_status': return { isolated: visualMode, paused: false, pausedUntilMs: null, lastError: null };
      case 'get_mcp_access_status': return { enabled: false, clients: [] };
      case 'list_sync_conflicts': return structuredClone(state.conflicts);
      case 'list_shared_conflicts': return structuredClone(state.sharedConflicts);
      case 'resolve_shared_conflict': {
        const { conflict_id: id, current_operation_id: revision, resolution } = args.request;
        const card = state.sharedConflicts.find(card => card.id === id);
        if (!card || !card.canResolve || card.currentOperationId !== revision || !['keep_current', 'use_first', 'use_second'].includes(resolution)) throw { message: '共享合成请求未通过身份、版本或权限检查。' };
        await new Promise(resolve => setTimeout(resolve, 1_500));
        if (card.pinboardName === '过期预览测试板' && !state.sharedStaleInjected) {
          state.sharedStaleInjected = true;
          card.currentOperationId = uuid(90);
          card.current.title = '另一端刚刚编辑，需重新选择';
          throw { message: '共享内容在预览后已改变，请刷新后重新选择；未覆盖任何内容。' };
        }
        state.sharedConflicts = state.sharedConflicts.filter(card => card.id !== id);
        return structuredClone(state.sharedConflicts);
      }
      case 'set_cloud_sync_enabled': state.enabled = args.request.enabled; break;
      case 'get_sync_status': break;
      case 'resolve_sync_conflict': {
        const { conflict_id: id, resolution } = args.request;
        if (!['keep_local', 'accept_remote'].includes(resolution)) throw { message: '无效的测试选择' };
        await new Promise(resolve => setTimeout(resolve, 1_500));
        state.conflicts = state.conflicts.filter(conflict => conflict.id !== id);
        state.pendingChanges += 1;
        return structuredClone(state.conflicts);
      }
      default: throw { message: `隔离验证未开放此操作：${command}` };
    }
    return {
      enabled: state.enabled, localOutboxReady: true, cloudTransportConfigured: false,
      syncing: false, lastSuccessAtMs: null, pendingChanges: state.pendingChanges,
      pendingConflicts: state.conflicts.length, pendingDependencies: state.pendingDependencies,
      pendingSharedDownloads: 0, pendingSharedConflicts: state.sharedConflicts.length,
      blockedReason: '隔离测试，不连接 iCloud。',
    };
  } } };
}

const iconAssets = {};
for (const [bundle, name] of [['com.apple.TextEdit', 'textedit.png'], ['com.apple.finder', 'finder.png']]) {
  try {
    const png = await readFile(new URL(`../target/native-source-icons/${name}`, import.meta.url));
    iconAssets[bundle] = `data:image/png;base64,${png.toString('base64')}`;
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
}
const pdfAssets = {};
for (const name of ['portrait', 'rotated']) {
  try {
    const png = await readFile(new URL(`../target/pdf-preview-verification/${name}.png`, import.meta.url));
    const pdf = await readFile(new URL(`../tmp/pdfs/synthetic-${name}.pdf`, import.meta.url));
    pdfAssets[name] = { png: `data:image/png;base64,${png.toString('base64')}`, pdf: `data:application/pdf;base64,${pdf.toString('base64')}` };
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
}
const server = createServer(async (request, response) => {
  try {
    if (request.method !== 'GET' || !/^127\.0\.0\.1:\d+$/.test(request.headers.host ?? '')) {
      response.writeHead(403).end(); return;
    }
    const pathname = new URL(request.url, 'http://127.0.0.1').pathname;
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('X-Content-Type-Options', 'nosniff');
    response.setHeader('Content-Security-Policy', [imagePolicy, framePolicy].filter(Boolean).join('; '));
    if (pathname === '/__fixture.js') {
      response.writeHead(200, { 'Content-Type': 'text/javascript' });
      response.end(`(${fixtureBootstrap.toString()})(${JSON.stringify(iconAssets)}, ${JSON.stringify(pdfAssets)});`); return;
    }
    const name = pathname === '/' ? 'index.html' : pathname.slice(1);
    const extension = name.split('.').at(-1);
    if (basename(name) !== name || !mime[extension] || !/^[\w.-]+$/.test(name)) {
      response.writeHead(404).end(); return;
    }
    let content = await readFile(resolve(assetRoot, name));
    if (name === 'index.html') {
      content = content.toString().replace('<head>', '<head><script src="/__fixture.js"></script>');
    }
    response.writeHead(200, { 'Content-Type': mime[extension] });
    response.end(content);
  } catch {
    response.writeHead(404).end('Build the desktop UI before running this fixture.');
  }
});

server.listen(0, '127.0.0.1', () => {
  console.log(`Isolated compiled UI fixture: http://127.0.0.1:${server.address().port}`);
});
process.on('SIGINT', () => server.close());
