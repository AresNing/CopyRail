// Runs the real compiled Leptos UI in a fresh headless Chrome profile. The
// server injects only synthetic IPC; no desktop process or user profile runs.
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const root = fileURLToPath(new URL('../', import.meta.url));
const profile = await mkdtemp(join(tmpdir(), 'pasters-headless-conflicts-'));
const artifacts = join(root, 'target/ui-verification');
const children = [];
let socket;
function launch(command, args, pattern) {
  const child = spawn(command, args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
  children.push(child);
  return new Promise((resolve, reject) => {
    let output = '';
    const timeout = setTimeout(() => reject(new Error('Timed out starting isolated UI verifier')), 30_000);
    const listen = chunk => {
      output = (output + chunk.toString()).slice(-16_384);
      const match = output.match(pattern);
      if (match) { clearTimeout(timeout); resolve(match[1]); }
    };
    child.stdout.on('data', listen);
    child.stderr.on('data', listen);
    child.on('error', error => { clearTimeout(timeout); reject(error); });
    child.on('exit', (code, signal) => { clearTimeout(timeout); reject(new Error(`Isolated verifier exited before ready (${code}, ${signal}): ${output.slice(-3000)}`)); });
  });
}

try {
  const fixture = await launch(process.execPath, ['scripts/ui-fixture-server.mjs'], /Isolated compiled UI fixture: (http:\/\/127\.0\.0\.1:\d+)/);
  const endpoint = await launch('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new', `--user-data-dir=${profile}`, '--remote-debugging-port=0',
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
    '--disable-component-update', '--disable-sync', '--disable-extensions',
    '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1', 'about:blank',
  ], /DevTools listening on (ws:\/\/127\.0\.0\.1:\d+\/devtools\/browser\/[^\s]+)/);
  socket = new WebSocket(endpoint);
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }); });
  let serial = 0;
  const pending = new Map();
  socket.addEventListener('message', event => {
    const message = JSON.parse(String(event.data));
    const request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id);
    clearTimeout(request.timeout);
    if (message.error) request.reject(new Error(JSON.stringify(message.error)));
    else request.resolve(message.result);
  });
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const id = ++serial;
    const timeout = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 30_000);
    pending.set(id, { resolve, reject, timeout });
    socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
  });
  const browserVersion = await send('Browser.getVersion');
  const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
  const page = (method, params) => send(method, params, sessionId);
  const evaluate = async expression => {
    const result = await page('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  const waitFor = expression => evaluate(`(async () => { for (let n = 0; n < 150; n++) { if (${expression}) return true; await new Promise(r => setTimeout(r, 100)); } throw new Error('UI condition timed out'); })()`);
  await page('Page.enable');
  await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await page('Page.navigate', { url: `${fixture}/?fixture=shared-conflicts` });
  await waitFor(`document.querySelector('.settings-button')`);
  await evaluate(`document.querySelector('.settings-button').click()`);
  await waitFor(`document.querySelectorAll('[aria-label="共享内容冲突"] article').length === 3`);
  await evaluate(`(() => {
    const version = document.querySelector('.shared-conflict-version');
    version.open = true; version.querySelector('summary').focus();
    version.scrollIntoView({ block: 'center' });
  })()`);
  // The 2-second status polling must not recreate unchanged keyed cards or
  // collapse their open previews / move keyboard focus.
  await evaluate(`new Promise(r => setTimeout(r, 2300))`);
  assert.equal(await evaluate(`document.querySelector('.shared-conflict-version').open && document.activeElement === document.querySelector('.shared-conflict-version summary')`), true);
  assert.equal(await evaluate(`document.querySelector('.shared-conflict-version p').textContent.includes('<img') && !document.querySelector('.shared-conflict-version img')`), true);
  assert.equal(await evaluate(`Array.from(document.querySelectorAll('[aria-label="共享内容冲突"] article'))[1].querySelectorAll('button:disabled').length`), 3);
  assert.equal(await evaluate(`Array.from(document.querySelectorAll('[aria-label="共享内容冲突"] article')).every(card => card.scrollWidth <= card.clientWidth + 1)`), true);
  await mkdir(artifacts, { recursive: true });
  for (const theme of ['light', 'dark']) {
    await page('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    const screenshot = await page('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(artifacts, `shared-conflicts-${theme}.png`), Buffer.from(screenshot.data, 'base64'));
  }
  await evaluate(`document.querySelector('[aria-label="共享内容冲突"] article .shared-conflict-actions button:nth-child(2)').click()`);
  await waitFor(`document.querySelector('[aria-label="共享内容冲突"] article').getAttribute('aria-busy') === 'true'`);
  assert.equal(await evaluate(`document.querySelector('[aria-label="共享内容冲突"] article').querySelectorAll('button:disabled').length`), 3);
  await waitFor(`document.querySelectorAll('[aria-label="共享内容冲突"] article').length === 2`);
  assert.equal(await evaluate(`document.body.textContent.includes('选择已保存并加入共享待发送队列')`), true);
  await evaluate(`Array.from(document.querySelectorAll('[aria-label="共享内容冲突"] article'))[1].querySelector('button').click()`);
  await waitFor(`document.body.textContent.includes('未覆盖任何内容') && document.body.textContent.includes('另一端刚刚编辑，需重新选择')`);
  await evaluate(`Array.from(document.querySelectorAll('[aria-label="共享内容冲突"] article'))[1].querySelector('button').click()`);
  await waitFor(`document.querySelectorAll('[aria-label="共享内容冲突"] article').length === 1`);
  assert.equal(await evaluate(`document.querySelector('[aria-label="共享内容冲突"] article').textContent.includes('只读共享板')`), true);
  console.log(JSON.stringify({ result: 'passed', browser: browserVersion.product, checks: ['compiled UI', 'preview text escaping', 'polling focus preservation', 'read-only buttons', 'busy buttons', 'choice request and removal', 'stale preview refresh and retry', 'no horizontal card overflow'], screenshots: ['shared-conflicts-light.png', 'shared-conflicts-dark.png'].map(name => join(artifacts, name)), nativeEndToEnd: false }));
  await send('Browser.close').catch(() => {});
} finally {
  socket?.close();
  await Promise.all(children.map(child => new Promise(resolve => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    child.once('exit', resolve); child.kill('SIGTERM');
    setTimeout(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); }, 2000).unref();
  })));
  await rm(profile, { recursive: true, force: true });
}
