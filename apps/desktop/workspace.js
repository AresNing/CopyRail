// Owned window bridge; no clipboard access or application data is stored here.
window.copyrailWindowRole = function() {
  return window.__TAURI__?.window?.getCurrentWindow?.().label || window.__copyrailWorkspaceRole || 'embedded';
}
window.copyrailWorkspacePaint = function() {
  return new Promise(resolve => {
    let done=false; const finish=()=>{if(!done){
      done=true;
      const dialog=document.querySelector('.auxiliary-workspace [role="dialog"]');
      if(dialog && !dialog.contains(document.activeElement)) {
        dialog.tabIndex=-1;
        dialog.focus({preventScroll:true});
      }
      resolve();
    }};
    requestAnimationFrame(()=>requestAnimationFrame(finish));
    // Hidden WKWebViews may suspend animation frames. The auxiliary window
    // can be shown independently without moving or exposing the rail backing.
    setTimeout(finish,120);
  });
}
window.copyrailDispatchKey = function(key, shift, meta) {
  document.querySelector('.paste-shell')?.dispatchEvent(new KeyboardEvent('keydown', {key,shiftKey:shift,metaKey:meta,bubbles:true,cancelable:true}));
}
