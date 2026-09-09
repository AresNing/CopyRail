import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { cp, mkdtemp, mkdir, readFile, readdir, rm, symlink, utimes, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { assertIdle, maintain, planArtifacts, retainedVersions, safePath } from './maintain-artifacts.mjs';

const release = n => `PasteRS-0.1.0-local-beta.${n}`;
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'pasters-artifact-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}
async function file(root, rel, contents = 'fixture') {
  const path = join(root, rel);
  await mkdir(join(path, '..'), { recursive: true });
  await writeFile(path, contents);
}
async function version(root, n) {
  await file(root, `output/${release(n)}/manifest.json`, JSON.stringify({
    deliveryVersion: release(n).slice(8), app: 'PasteRS.app', buildInfo: { appIdentifier: 'io.pasters.desktop' },
  }));
  await file(root, `output/${release(n)}/PasteRS.app/Contents/MacOS/pasters-desktop`);
  await file(root, `output/${release(n)}.zip`);
  await file(root, `output/${release(n)}.zip.sha256`);
}

test('retains 3 versions total including current with numeric rather than lexical sorting', () => {
  const names = [1, 2, 3, 4, 9, 10].map(release);
  assert.deepEqual([...retainedVersions(names)], [10, 9, 4].map(release));
  assert.deepEqual([...retainedVersions(names, '0.1.0-local-beta.2')], [2, 10, 9].map(release));
  assert.equal(retainedVersions(names, undefined, [release(1)]).size, 4);
  assert.throws(() => retainedVersions(names, '../../history'));
  assert.throws(() => retainedVersions(names, '0.1.0-local-beta.99'));
  assert.deepEqual([...retainedVersions([])], []);
});

test('preview does not delete; apply prunes entire expired release group, not source/data/evidence', async t => {
  const root = await fixture(t);
  for (let n = 1; n <= 6; n++) await version(root, n);
  const preserved = ['src/main.rs', 'Library/Application Support/io.pasters.desktop/history.db',
    'target/ui-verification/report.json', 'target/ui-verification/screenshot.png',
    'target/debug/pasters-desktop', 'output/unrelated.zip'];
  for (const path of [...preserved, 'target/debug/deps/cache', 'target/debug/incremental/cache']) await file(root, path);
  const plan = await maintain(root, { processReader: () => [] });
  assert.equal(plan.remove.length, 11);
  assert.equal((await readdir(join(root, 'output'))).length, 19);
  await maintain(root, { apply: true, processReader: () => [] });
  assert.equal((await readdir(join(root, 'output'))).length, 10);
  for (const path of preserved) assert.equal(await readFile(join(root, path), 'utf8'), 'fixture');
  assert.equal((await planArtifacts(root)).remove.length, 0);
});

test('live release group and executable cache are protected', async t => {
  const root = await fixture(t);
  for (let n = 1; n <= 6; n++) await version(root, n);
  await file(root, 'target/debug/deps/test-binary');
  const processes = [
    { executable: join(root, `output/${release(1)}/PasteRS.app/Contents/MacOS/pasters-desktop`) },
    { executable: join(root, 'target/debug/deps/test-binary') },
  ];
  const plan = await planArtifacts(root, { processes });
  assert.ok(plan.keep.includes(release(1)));
  assert.equal(plan.remove.length, 6);
  assert.equal(plan.skipped.length, 1);
});

test('any active compiler aborts before writes', async t => {
  const root = await fixture(t);
  await file(root, 'target/debug/deps/cache');
  for (const name of ['cargo', 'cargo-tauri', 'rustc', 'trunk', 'wasm-opt', 'wasm-bindgen', 'history_benchmark']) {
    assert.throws(() => assertIdle([{ executable: `/tools/${name}` }]));
  }
  await assert.rejects(maintain(root, { apply: true, processReader: () => [{ executable: '/tools/cargo' }] }));
  assert.equal(await readFile(join(root, 'target/debug/deps/cache'), 'utf8'), 'fixture');
});

test('symlink leaf and symlink parent both fail closed before any deletion', async t => {
  const root = await fixture(t), outside = await fixture(t);
  await file(root, 'target/debug/incremental/keep');
  await file(outside, 'user-data');
  await symlink(outside, join(root, 'target/debug/deps'));
  await assert.rejects(maintain(root, { apply: true, processReader: () => [] }), /Symlink/);
  assert.equal(await readFile(join(root, 'target/debug/incremental/keep'), 'utf8'), 'fixture');
  assert.equal(await readFile(join(outside, 'user-data'), 'utf8'), 'fixture');
  await assert.rejects(safePath(root, join(root, 'target/debug/deps/missing')), /Symlink/);
  await assert.rejects(safePath(root, outside), /Outside/);
  await assert.rejects(safePath(root, root), /Outside/);
});

test('unknown/missing release metadata is preserved; mismatched identity aborts', async t => {
  const root = await fixture(t);
  await file(root, `output/${release(1)}/unknown-file`);
  assert.equal((await planArtifacts(root)).skipped.length, 1);
  await version(root, 2);
  await file(root, `output/${release(2)}/manifest.json`, JSON.stringify({ deliveryVersion: 'wrong' }));
  await assert.rejects(planArtifacts(root));
});

test('only identified QA bundles and benchmark DB bytes are removed, evidence remains', async t => {
  const root = await fixture(t);
  const qa = 'target/debug/bundle/macos/PasteRS Local QA abcdef12.app';
  await file(root, `${qa}/Contents/Info.plist`, '<key>CFBundleIdentifier</key><string>io.pasters.localqa.abcdef12abcdef12abcdef12abcdef12</string>');
  const bench = 'target/history-performance/synthetic-ABC123';
  await file(root, `${bench}/synthetic-fixture.json`, JSON.stringify({ kind: 'pasters-history-benchmark-v1' }));
  await file(root, `${bench}/history.sqlite3`);
  await file(root, `${bench}/report.json`);
  await file(root, 'target/history-performance/probe-compact-DEF456/probe.sqlite3');
  await file(root, 'target/history-performance/probe-compact-DEF456/report.json');
  await file(root, 'target/debug/bundle/macos/PasteRS.app/Contents/MacOS/pasters-desktop');
  await maintain(root, { apply: true, processReader: () => [] });
  assert.deepEqual((await readdir(join(root, bench))).sort(), ['report.json', 'synthetic-fixture.json']);
  assert.deepEqual(await readdir(join(root, 'target/history-performance/probe-compact-DEF456')), ['report.json']);
  assert.deepEqual(await readdir(join(root, 'target/debug/bundle/macos')), ['PasteRS.app']);
});

test('a process starting after plan prevents deleting its files', async t => {
  const root = await fixture(t);
  await file(root, 'target/debug/deps/binary');
  let calls = 0;
  await assert.rejects(maintain(root, { apply: true, processReader: () => ++calls === 1 ? [] : [
    { executable: join(root, 'target/debug/deps/binary') },
  ] }), /Now running/);
  assert.equal(await readFile(join(root, 'target/debug/deps/binary'), 'utf8'), 'fixture');
});

async function verifiedDelivery(root) {
  await version(root, 1);
  const manifestPath = join(root, `output/${release(1)}/manifest.json`);
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  const bytes = Buffer.from('fixture');
  manifest.binarySha256 = createHash('sha256').update(bytes).digest('hex');
  manifest.binaryBytes = bytes.length;
  await writeFile(manifestPath, JSON.stringify(manifest));
  await utimes(join(root, `output/${release(1)}/PasteRS.app/Contents/MacOS/pasters-desktop`), 1000, 1000);
}

test('sweep removes only older loose outputs, preserving new builds and unrelated files', async t => {
  const root = await fixture(t);
  await verifiedDelivery(root);
  for (const name of ['libpasters_desktop_lib.a', 'libpasters_desktop_lib.rlib', 'pasters-desktop']) {
    await file(root, `target/debug/${name}`);
    await utimes(join(root, `target/debug/${name}`), 900, 900);
  }
  await file(root, 'target/debug/libpasters_desktop_lib.dylib');
  await file(root, 'target/debug/unrelated.a');
  await file(root, 'apps/desktop/dist/index.html');
  const plan = await maintain(root, { sweep: true, apply: true, processReader: () => [] });
  assert.equal(plan.remove.length, 3);
  assert.equal(plan.skipped.length, 1);
  assert.equal(await readFile(join(root, 'target/debug/libpasters_desktop_lib.dylib'), 'utf8'), 'fixture');
  assert.equal(await readFile(join(root, 'target/debug/unrelated.a'), 'utf8'), 'fixture');
  assert.equal(await readFile(join(root, 'apps/desktop/dist/index.html'), 'utf8'), 'fixture');
});

test('sweep requires intact retained delivery before deleting even a cache', async t => {
  const root = await fixture(t);
  await file(root, 'target/debug/deps/cache');
  await assert.rejects(maintain(root, { sweep: true, apply: true, processReader: () => [] }), /requires a retained delivery/);
  await verifiedDelivery(root);
  await file(root, `output/${release(1)}/PasteRS.app/Contents/MacOS/pasters-desktop`, 'changed');
  await assert.rejects(maintain(root, { sweep: true, apply: true, processReader: () => [] }), /checksum mismatch/);
  assert.equal(await readFile(join(root, 'target/debug/deps/cache'), 'utf8'), 'fixture');
});

test('sweep removes an exact archived bundle but preserves any modified bundle', async t => {
  const root = await fixture(t);
  await verifiedDelivery(root);
  const app = join(root, 'target/debug/bundle/macos/PasteRS.app');
  await mkdir(join(app, '..'), { recursive: true });
  await cp(join(root, `output/${release(1)}/PasteRS.app`), app, { recursive: true });
  assert.equal((await planArtifacts(root, { sweep: true })).remove.length, 1);
  await file(root, 'target/debug/bundle/macos/PasteRS.app/unarchived-resource');
  const plan = await planArtifacts(root, { sweep: true });
  assert.equal(plan.remove.length, 0);
  assert.equal(plan.skipped[0].reason, 'bundle differs from retained releases');
});

test('sweep protects loose libraries while any binary in the same profile runs', async t => {
  const root = await fixture(t);
  await verifiedDelivery(root);
  await file(root, 'target/debug/libpasters_desktop_lib.a');
  await utimes(join(root, 'target/debug/libpasters_desktop_lib.a'), 900, 900);
  const processes = [{ executable: join(root, 'target/debug/pasters-desktop') }];
  const plan = await planArtifacts(root, { sweep: true, processes });
  assert.equal(plan.remove.length, 0);
  assert.equal(plan.skipped[0].reason, 'running process');
});

test('sweep inspects nested packaging freshness and never follows bundle symlinks', async t => {
  const root = await fixture(t);
  await verifiedDelivery(root);
  const rel = 'target/debug/bundle/dmg';
  await file(root, `${rel}/new-file`);
  await utimes(join(root, rel), 900, 900);
  assert.equal((await planArtifacts(root, { sweep: true })).remove.length, 0);
  await utimes(join(root, rel, 'new-file'), 900, 900);
  assert.equal((await planArtifacts(root, { sweep: true })).remove.length, 1);
  await symlink(join(root, `output/${release(1)}`), join(root, rel, 'redirect'));
  await assert.rejects(planArtifacts(root, { sweep: true }), /Symlink/);
});

test('CopyRail and PasteRS releases share one numeric retention budget', () => {
  const names = [1, 2, 3].map(release).concat('CopyRail-0.1.0-local-beta.4', 'CopyRail-0.1.0-local-beta.10');
  assert.deepEqual([...retainedVersions(names, '0.1.0-local-beta.4')], [
    'CopyRail-0.1.0-local-beta.4', 'CopyRail-0.1.0-local-beta.10', release(3),
  ]);
  assert.throws(() => retainedVersions([release(4), 'CopyRail-0.1.0-local-beta.4'], '0.1.0-local-beta.4'), /unambiguous/);
});

test('sweep validates renamed bundle and prunes only its archived duplicate', async t => {
  const root = await fixture(t);
  for (let n = 1; n <= 3; n++) await version(root, n);
  const name = 'CopyRail-0.1.0-local-beta.4';
  const binary = 'CopyRail.app/Contents/MacOS/pasters-desktop';
  await file(root, `output/${name}/${binary}`);
  await file(root, `output/${name}/manifest.json`, JSON.stringify({
    deliveryVersion: '0.1.0-local-beta.4', app: 'CopyRail.app',
    buildInfo: { appIdentifier: 'io.pasters.desktop' }, binaryBytes: 7,
    binarySha256: createHash('sha256').update('fixture').digest('hex'),
  }));
  const bundle = join(root, 'target/debug/bundle/macos/CopyRail.app');
  await mkdir(join(bundle, '..'), { recursive: true });
  await cp(join(root, `output/${name}/CopyRail.app`), bundle, { recursive: true });
  const plan = await planArtifacts(root, { sweep: true, current: '0.1.0-local-beta.4' });
  assert.equal(plan.keep.length, 3);
  assert.equal(plan.remove.length, 4);
  assert.ok(plan.remove.some(item => item.path === bundle));
  const protectedPlan = await planArtifacts(root, { sweep: true, processes: [{ executable: join(bundle, 'Contents/MacOS/pasters-desktop') }] });
  assert.equal(protectedPlan.remove.length, 3);
  assert.ok(protectedPlan.remove.every(item => !item.path.startsWith(bundle)));
});
