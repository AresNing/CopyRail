import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { buildArguments, createOverlay, ensureBuildEnvironment, verifyBuildInfo } from './prepare-local-qa.mjs';

const base = JSON.parse(await readFile(new URL('../apps/desktop/src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
const session = '12abcdef'.repeat(4);
const overlay = createOverlay(base, session);
const expectedInfo = {
  formatVersion: 1, appIdentifier: overlay.identifier, productName: overlay.productName,
  mainWindowTitle: overlay.app.windows[0].title, debugBuild: true, cloudTransportCompiled: false,
};

test('fresh compile-time identity; no production config mutation or window geometry drift', () => {
  const before = structuredClone(base);
  const created = createOverlay(base, session);
  assert.deepEqual(base, before);
  assert.notEqual(created.identifier, base.identifier);
  assert.notEqual(created.identifier, createOverlay(base, 'fa987654'.repeat(4)).identifier);
  assert.deepEqual(created.app.windows.map(({ title, ...window }) => window), base.app.windows.map(({ title, ...window }) => window));
  assert.equal(created.bundle.macOS.entitlements, null);
  assert.equal(created.bundle.macOS.signingIdentity, null);
  assert.equal(created.app.windows[0].visible, base.app.windows[0].visible);
});

test('invalid identities and changed production assumptions fail closed', () => {
  for (const id of ['', '../history', 'ab'.repeat(15), 'AB'.repeat(16), session + '/']) {
    assert.throws(() => createOverlay(base, id));
  }
  assert.throws(() => createOverlay({ ...base, identifier: 'io.pasters.localqa' }, session));
  assert.throws(() => createOverlay({ ...base, app: { windows: [] } }, session));
});

test('build is locked, debug, no signing and has no CloudKit/default features', () => {
  assert.deepEqual(buildArguments('/tmp/a b/config.json'), [
    'tauri', 'build', '--debug', '--no-sign', '--bundles', 'app', '--config',
    '/tmp/a b/config.json', '--', '--no-default-features', '--locked',
  ]);
});

test('compiled metadata must match the bundle, not just a renamed Info.plist', () => {
  verifyBuildInfo(expectedInfo, overlay);
  for (const [key, value] of Object.entries({
    appIdentifier: base.identifier, productName: base.productName,
    mainWindowTitle: 'CopyRail', debugBuild: false, cloudTransportCompiled: true, formatVersion: 2,
  })) assert.throws(() => verifyBuildInfo({ ...expectedInfo, [key]: value }, overlay));
});

test('inherited config/target overrides cannot silently select another artifact', () => {
  ensureBuildEnvironment({});
  for (const key of ['TAURI_CONFIG', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_TARGET_DIR']) {
    assert.throws(() => ensureBuildEnvironment({ [key]: 'unexpected' }));
  }
});
