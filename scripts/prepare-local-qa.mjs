// Builds a distinct normal-mode app. Never opens it or starts clipboard services.
// Unlike native-ui-test, this app WILL capture real new copies when later opened.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { execFileSync, spawnSync } from 'node:child_process';
import { cp, lstat, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function createOverlay(base, session) {
  assert.match(session, /^[a-f0-9]{32}$/);
  assert.equal(base.identifier, 'io.pasters.desktop', 'Review the new production identity first');
  assert.equal(base.productName, 'CopyRail');
  assert.equal(base.app.windows.filter(window => window.label === 'main').length, 1);
  const suffix = session.slice(0, 8);
  return {
    identifier: `io.pasters.localqa.${session}`,
    productName: `CopyRail Local QA ${suffix}`,
    // JSON Merge Patch replaces arrays, so retain every production window field.
    app: { windows: base.app.windows.map(window => ({
      ...window,
      ...(window.label === 'main' ? { title: `CopyRail — 本机核心验收 ${suffix}` } : {}),
    })) },
    bundle: { macOS: { entitlements: null, signingIdentity: null } },
  };
}

export function buildArguments(configPath) {
  return ['tauri', 'build', '--debug', '--no-sign', '--bundles', 'app',
    '--config', configPath, '--', '--no-default-features', '--locked'];
}

export function verifyBuildInfo(info, overlay) {
  assert.equal(info.formatVersion, 1);
  assert.equal(info.appIdentifier, overlay.identifier, 'Compiled identity differs from QA bundle');
  assert.equal(info.productName, overlay.productName);
  assert.equal(info.mainWindowTitle, overlay.app.windows.find(window => window.label === 'main').title);
  assert.equal(info.debugBuild, true);
  assert.equal(info.cloudTransportCompiled, false, 'Cloud transport must not be compiled');
}

const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
async function hashFile(path) { return sha256(await readFile(path)); }
async function exists(path) {
  try { await lstat(path); return true; }
  catch (error) { if (error.code === 'ENOENT') return false; throw error; }
}

export function ensureBuildEnvironment(env) {
  // Prevent an inherited overlay/target from silently selecting a different build.
  for (const name of ['TAURI_CONFIG', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_TARGET_DIR']) {
    assert.ok(!env[name], `Unset ${name} before preparing the local QA bundle`);
  }
}

async function main() {
  if (process.argv.slice(2).join(' ') === '--help') {
    console.log('Usage: node scripts/prepare-local-qa.mjs\nBuilds and checks a fresh local QA bundle; never opens it.\nLater opening it uses the REAL clipboard and default global shortcut, with a separate persistent history.');
    return;
  }
  assert.equal(process.argv.length, 2, 'No launch, profile-path or feature overrides are accepted');
  assert.equal(process.platform, 'darwin', 'macOS only');
  ensureBuildEnvironment(process.env);
  const project = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const desktop = join(project, 'apps/desktop');
  const base = JSON.parse(await readFile(join(desktop, 'src-tauri/tauri.conf.json'), 'utf8'));
  const overlay = createOverlay(base, randomUUID().replaceAll('-', ''));
  const dataDir = join(homedir(), 'Library/Application Support', overlay.identifier);
  const cacheDir = join(homedir(), 'Library/Caches', overlay.identifier);
  for (const path of [dataDir, cacheDir]) assert.equal(await exists(path), false, `Not a fresh namespace: ${path}`);
  const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--no-deps', '--locked', '--format-version=1'], { cwd: project, encoding: 'utf8' }));
  const bundleDir = join(metadata.target_directory, 'debug/bundle/macos');
  const builtApp = join(bundleDir, `${overlay.productName}.app`);
  assert.equal(await exists(builtApp), false, `Refusing to overwrite an existing QA bundle: ${builtApp}`);
  const normalBundle = join(bundleDir, 'CopyRail.app');
  const normalBinary = join(normalBundle, 'Contents/MacOS/pasters-desktop');
  const normalBefore = await exists(normalBinary) ? await hashFile(normalBinary) : null;
  const root = await mkdtemp('/private/tmp/pasters-local-qa-');
  const configPath = join(root, 'tauri.local-qa.json');
  await writeFile(configPath, JSON.stringify(overlay, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  console.log(`Preparation directory: ${root}`);
  console.log('Building a separate app; no app launch, permission prompt or clipboard operation.');
  const args = buildArguments(configPath);
  const result = spawnSync('cargo', args, { cwd: desktop, stdio: 'inherit' });
  assert.ifError(result.error);
  assert.equal(result.status, 0, `QA build failed; preparation files retained at ${root}`);
  const app = join(root, `${overlay.productName}.app`);
  await cp(builtApp, app, { recursive: true, force: false, errorOnExist: true });
  const plist = JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', join(app, 'Contents/Info.plist')], { encoding: 'utf8' }));
  assert.equal(plist.CFBundleIdentifier, overlay.identifier);
  assert.equal(plist.CFBundleExecutable, 'pasters-desktop');
  assert.equal(plist.CFBundleName, overlay.productName);
  const binary = join(app, 'Contents/MacOS/pasters-desktop');
  // This sole executable call exits through the metadata branch BEFORE run().
  // No normal-mode process, application builder, plugin or platform service starts.
  const info = JSON.parse(execFileSync(binary, ['--build-info'], { encoding: 'utf8', timeout: 15000 }));
  verifyBuildInfo(info, overlay);
  if (normalBefore !== null) assert.equal(await hashFile(normalBinary), normalBefore, 'Normal candidate was modified');
  for (const path of [dataDir, cacheDir]) assert.equal(await exists(path), false, `Metadata check unexpectedly created ${path}`);
  const manifest = {
    formatVersion: 1,
    preparedAt: new Date().toISOString(),
    app, buildInfo: info, configPath,
    build: { executable: 'cargo', arguments: args, cwd: desktop },
    binarySha256: await hashFile(binary),
    normalCandidate: { path: normalBundle, binarySha256: normalBefore, unchanged: normalBefore !== null },
    expectedDataDirectory: dataDir,
    expectedCacheDirectory: cacheDir,
    dataDirectoryCreated: false,
    desktopServicesStarted: false,
    systemClipboardUsed: false,
    nativeEndToEnd: false,
    warning: 'This is NORMAL capture mode, not synthetic isolation. Opening it later uses the real system clipboard and default global shortcut. Obtain approval first. Do not copy sensitive content while it runs. Reopen this same bundle for restart acceptance; preparing again creates a different empty identity. Permissions are identity-specific. No installation, login startup, MCP or cloud setup is part of preparation.',
  };
  const manifestPath = join(root, 'preparation.json');
  await writeFile(manifestPath, JSON.stringify(manifest, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  console.log(`Prepared and checked (not opened): ${app}`);
  console.log(`Evidence: ${manifestPath}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
