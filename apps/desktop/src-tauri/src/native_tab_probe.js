// Read-only, bounded metadata from the synthetic main webview. No text,
// identifiers, event dispatch, style mutation or animation manipulation.
(() => {
  const finite = value => typeof value === 'number' && Number.isFinite(value) ? value : null;
  const nodes = Array.from(document.querySelectorAll('.pinboards > .pinboard'))
    .filter(e => !e.classList.contains('add-pinboard') && !e.classList.contains('manage-pinboard'));
  return {
    visible: document.visibilityState === 'visible',
    focused: document.hasFocus(),
    now_ms: performance.now(),
    timeline_ms: finite(document.timeline?.currentTime),
    tab_count: nodes.length,
    tabs: nodes.slice(0, 16).map((e, ordinal) => {
      const style = getComputedStyle(e);
      const channels = (style.backgroundColor.match(/[\d.]+/g) ?? []).map(Number);
      const animations = e.getAnimations();
      return {
        ordinal,
        active: e.classList.contains('active'),
        current: e.getAttribute('aria-current') === 'true',
        hovered: e.matches(':hover'),
        background: channels.length === 3 ? [...channels, 1] : channels,
        animation_count: animations.length,
        animations: animations.slice(0, 8).map(a => ({
          background_transition: a.transitionProperty === 'background-color',
          state: ['idle','running','paused','finished'].indexOf(a.playState),
          pending: a.pending,
          current_ms: finite(a.currentTime),
          start_ms: finite(a.startTime),
          timeline_ms: finite(a.timeline?.currentTime),
        })),
      };
    }),
  };
})()
