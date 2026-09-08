// Repository test harness only: compiled WASM, synthetic IPC, fresh browser
// profile. It cannot exercise AppKit or validate the user's native window.
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export async function withCompiledUiTest(run) {
  const root = fileURLToPath(new URL('../', import.meta.url));
  const profile = await mkdtemp(join(tmpdir(), 'pasters-compiled-ui-'));
  const artifacts = join(root, 'target/ui-verification');
  const children = [];
  let socket;
  let send;
  function launch(command, args, pattern) {
    const child = spawn(command, args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
    children.push(child);
    return new Promise((resolve, reject) => {
      let output = '';
      const timeout = setTimeout(() => reject(new Error('Isolated UI verifier startup timed out')), 30_000);
      const listen = chunk => {
        output = (output + chunk.toString()).slice(-16_384);
        const match = output.match(pattern);
        if (match) { clearTimeout(timeout); resolve(match[1]); }
      };
      child.stdout.on('data', listen); child.stderr.on('data', listen);
      child.on('error', error => { clearTimeout(timeout); reject(error); });
      child.on('exit', (code, signal) => { clearTimeout(timeout); reject(new Error(`Verifier exited (${code}, ${signal}): ${output.slice(-3000)}`)); });
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
      pending.delete(message.id); clearTimeout(request.timeout);
      if (message.error) request.reject(new Error(JSON.stringify(message.error)));
      else request.resolve(message.result);
    });
    send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
      const id = ++serial;
      const timeout = setTimeout(() => { pending.delete(id); reject(new Error(`Verifier protocol timeout: ${method}`)); }, 30_000);
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
    const waitFor = expression => evaluate(`(async () => { for (let n = 0; n < 150; n++) { if (${expression}) return true; await new Promise(r => setTimeout(r, 100)); } throw new Error('UI condition timed out: ' + ${JSON.stringify(expression)}); })()`);
    await mkdir(artifacts, { recursive: true });
    const screenshot = async name => {
      if (!/^[\w-]+\.png$/.test(name)) throw new Error('Invalid screenshot name');
      const shot = await page('Page.captureScreenshot', { format: 'png' });
      const path = join(artifacts, name);
      await writeFile(path, Buffer.from(shot.data, 'base64'));
      return path;
    };
    await page('Page.enable');
    await run({ page, evaluate, waitFor, fixture, screenshot, browser: browserVersion.product, artifacts });
  } finally {
    if (socket?.readyState === WebSocket.OPEN) await send('Browser.close').catch(() => {});
    socket?.close();
    await Promise.all(children.map(child => new Promise(resolve => {
      if (child.exitCode !== null || child.signalCode !== null) return resolve();
      child.once('exit', resolve); child.kill('SIGTERM');
      setTimeout(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); }, 2000).unref();
    })));
    await rm(profile, { recursive: true, force: true });
  }
}
