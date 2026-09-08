// Compiled Rust/WASM event routing only. No native drag session, desktop
// backend, real clipboard, database, external site, or user's browser profile.
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';

await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, browser, artifacts }) => {
  const results = [];
  await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 1, mobile: false });
  const load = async () => {
    await page('Page.navigate', { url: `${fixture}/?fixture=visual` });
    await waitFor(`document.querySelectorAll('.clip-card').length === 8 && document.querySelector('.native-test-badge')`);
    await evaluate(`(() => {
      window.gestureAttempts = []; window.gestureTraces = [];
      const invoke = window.__TAURI__.core.invoke;
      window.__TAURI__.core.invoke = async (command, args) => {
        if (command === 'trace_native_gesture') { window.gestureTraces.push(...args.request.events); return; }
        if (command === 'start_clip_drag') { window.gestureAttempts.push(args); throw { message: 'Synthetic native boundary: intentionally not started' }; }
        return invoke(command, args);
      };
    })()`);
    return evaluate(`(() => { const r = document.querySelector('.clip-card').getBoundingClientRect(); return { x: r.x + 70, y: r.y + 125 }; })()`);
  };
  const mouse = (type, p, buttons, button = 'left') => page('Input.dispatchMouseEvent', { type, ...p, button, buttons, clickCount: 1 });

  const point = await load();
  await mouse('mouseMoved', point, 0, 'none');
  await mouse('mousePressed', point, 1);
  await evaluate(`window.heldCard = document.querySelector('.clip-card')`);
  await mouse('mouseMoved', { x: point.x + 3, y: point.y + 3 }, 1);
  // Cross a background-poll interval while holding. This delay is local to the
  // test browser; it is not an attempt to synthesize a native macOS gesture.
  await evaluate(`new Promise(resolve => setTimeout(resolve, 1100))`);
  assert.equal(await evaluate(`window.gestureAttempts.length`), 0, 'sub-threshold movement must not invoke drag');
  assert.equal(await evaluate(`window.heldCard === document.querySelector('.clip-card') && window.heldCard.isConnected`), true, 'held card must survive background polling');
  await mouse('mouseMoved', { x: point.x + 12, y: point.y }, 1);
  await waitFor(`window.gestureAttempts.length === 1`);
  await mouse('mouseReleased', { x: point.x + 12, y: point.y }, 0);
  await waitFor(`window.gestureTraces.some(event => event.phase === 'threshold')`);
  assert.equal(await evaluate(`window.gestureTraces.find(event => event.phase === 'threshold').buttons`), 1);
  assert.equal(await evaluate(`window.gestureAttempts[0].request.clip_ids.length`), 1);
  results.push('held primary pointer crosses threshold once; card survives a poll interval');

  await load();
  await waitFor(`document.querySelector('.source-app-icon')?.naturalWidth === 64`);
  const iconPoint = await evaluate(`(() => { const r = document.querySelector('.source-app-icon').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
  await mouse('mouseMoved', iconPoint, 0, 'none');
  await mouse('mousePressed', iconPoint, 1);
  await mouse('mouseMoved', { x: iconPoint.x + 12, y: iconPoint.y }, 1);
  await waitFor(`window.gestureAttempts.length === 1`);
  await mouse('mouseReleased', { x: iconPoint.x + 12, y: iconPoint.y }, 0);
  assert.equal(await evaluate(`window.gestureAttempts[0].request.clip_ids.length`), 1);
  results.push('dragging over the real source image uses the card gesture, not browser image drag');

  await load();
  await evaluate(`(() => {
    const card = document.querySelector('.clip-card');
    for (const [type, x] of [['pointerdown', 95], ['pointermove', 125], ['pointerup', 125]]) {
      card.dispatchEvent(new PointerEvent(type, { bubbles: true, cancelable: true, pointerId: 7, pointerType: 'mouse', isPrimary: true, button: type === 'pointermove' ? -1 : 0, buttons: 0, clientX: x, clientY: 177 }));
    }
  })()`);
  await waitFor(`window.gestureTraces.some(event => event.phase === 'card_up')`);
  assert.equal(await evaluate(`window.gestureAttempts.length`), 0, 'movement with no held button must remain rejected');
  results.push('zero-buttons sequence does not invoke native drag');
  const report = { result: 'passed', browser, nativeEndToEnd: false, checks: results };
  await writeFile(join(artifacts, 'card-gesture-report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
});
