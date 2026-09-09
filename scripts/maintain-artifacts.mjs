// Local delivery housekeeping. No installation, process control or user-data access.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstat, mkdir, readdir, readFile, realpath, rm, rmdir } from 'node:fs/promises';
import { basename, dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const releasePattern = /^(?:PasteRS|CopyRail)-(\d+)\.(\d+)\.(\d+)-local-beta\.(\d+)$/;
const cachePaths = ['debug', 'release'].flatMap(profile =>
  ['incremental', 'deps', 'build', '.fingerprint', 'examples'].map(name => `target/${profile}/${name}`));
cachePaths.push('target/wasm32-unknown-unknown/debug', 'target/wasm32-unknown-unknown/release',
  'target/wasm-bindgen/debug', 'target/wasm-bindgen/release');

async function hashFile(root, path) {
  const entry = await safePath(root, path);
  assert.ok(entry?.isFile(), `Expected a regular file: ${path}`);
  return createHash('sha256').update(await readFile(path)).digest('hex');
}

// Bundle equality ignores timestamps but compares every relative filename,
// permission mode and byte. Do not delete an unarchived/modified candidate.
async function treeState(root, path) {
  const entry = await safePath(root, path);
  assert.ok(entry, `Missing tree: ${path}`);
  if (entry.isFile()) return { digest: `${entry.mode}:${await hashFile(root, path)}`, newest: entry.mtimeMs };
  assert.ok(entry.isDirectory(), `Unexpected file type: ${path}`);
  const children = [];
  let newest = entry.mtimeMs;
  for (const name of (await readdir(path)).sort()) {
    const child = await treeState(root, join(path, name));
    children.push([name, child.digest]);
    newest = Math.max(newest, child.newest);
  }
  return { digest: createHash('sha256').update(JSON.stringify([entry.mode, children])).digest('hex'), newest };
}

async function stat(path) {
  try { return await lstat(path); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
}

// Reject symlinks in EVERY component, including parents of an absent leaf.
export async function safePath(root, path) {
  const rel = relative(root, path);
  assert.ok(rel && rel !== '..' && !rel.startsWith(`..${sep}`) && !rel.startsWith(sep), 'Outside artifact root');
  let cursor = root;
  assert.equal((await lstat(root)).isSymbolicLink(), false, `Symlink root: ${root}`);
  for (const component of rel.split(sep)) {
    cursor = join(cursor, component);
    const entry = await stat(cursor);
    if (!entry) return null;
    assert.equal(entry.isSymbolicLink(), false, `Symlink: ${cursor}`);
  }
  return stat(path);
}

function newestFirst(a, b) {
  const av = a.match(releasePattern).slice(1).map(Number);
  const bv = b.match(releasePattern).slice(1).map(Number);
  for (let i = 0; i < av.length; i++) if (av[i] !== bv[i]) return bv[i] - av[i];
  return 0;
}

const releaseVersion = name => name.replace(/^(?:PasteRS|CopyRail)-/, '');
const releaseApp = name => name.startsWith('CopyRail-') ? 'CopyRail.app' : 'PasteRS.app';
function selectCurrent(names, current) {
  if (!current) return [...names].sort(newestFirst)[0];
  const matches = names.filter(name => releaseVersion(name) === current);
  assert.equal(matches.length, 1, 'Current version must have one unambiguous valid delivery manifest');
  return matches[0];
}

export function retainedVersions(names, current, running = []) {
  assert.ok(names.every(name => releasePattern.test(name)), 'Unsupported release name');
  const sorted = [...names].sort(newestFirst);
  const chosen = selectCurrent(sorted, current);
  if (current) assert.ok(names.includes(chosen), 'Current version must have a valid delivery manifest');
  const keep = new Set(chosen ? [chosen, ...sorted.filter(name => name !== chosen).slice(0, 2)] : []);
  for (const name of running) keep.add(name);
  return keep;
}

export function readProcesses() {
  return execFileSync('/bin/ps', ['-axo', 'pid=,comm='], { encoding: 'utf8' })
    .trim().split('\n').filter(Boolean).map(line => {
      const [, pid, executable] = line.match(/^\s*(\d+)\s+(.+)$/);
      return { pid: Number(pid), executable };
    });
}

function inUse(path, processes) {
  return processes.some(({ executable }) => executable === path || executable.startsWith(`${path}/`));
}

export function assertIdle(processes) {
  assert.equal(processes.some(({ executable }) =>
    /^(cargo(?:-.+)?|rustc|rust-lld|trunk|wasm-bindgen|wasm-opt|history_benchmark(?:-.+)?)$/.test(basename(executable))), false,
  'A Rust/WASM build is active; finish it before cleanup');
}

export async function planArtifacts(root, { current, processes = [], sweep = false } = {}) {
  const remove = [], skipped = [];
  const add = async (rel, reason, type = 'directory', guardRel = rel) => {
    const path = join(root, rel);
    const guardPath = join(root, guardRel);
    const entry = await safePath(root, path);
    if (!entry) return;
    assert.ok(type === 'directory' ? entry.isDirectory() : entry.isFile(), `Unexpected artifact type: ${path}`);
    if (inUse(guardPath, processes)) { skipped.push({ path, reason: 'running process' }); return; }
    remove.push({ path, guardPath, reason, dev: entry.dev, ino: entry.ino,
      mtimeMs: entry.mtimeMs, size: entry.size });
  };
  const output = join(root, 'output');
  const versions = [];
  if (await safePath(root, output)) {
    for (const entry of await readdir(output, { withFileTypes: true })) {
      if (!releasePattern.test(entry.name)) continue;
      const manifestPath = join(output, entry.name, 'manifest.json');
      if (!await safePath(root, manifestPath)) { skipped.push({ path: join(output, entry.name), reason: 'no manifest' }); continue; }
      const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
      assert.equal(manifest.deliveryVersion, releaseVersion(entry.name));
      assert.equal(manifest.app, releaseApp(entry.name));
      assert.equal(manifest.buildInfo.appIdentifier, 'io.pasters.desktop');
      versions.push(entry.name);
    }
  }
  const running = versions.filter(name => inUse(join(output, name), processes));
  const keep = retainedVersions(versions, current, running);
  for (const name of versions.filter(name => !keep.has(name))) {
    await add(`output/${name}`, 'older than 3 retained versions (current included)');
    await add(`output/${name}.zip`, 'same expired release', 'file');
    await add(`output/${name}.zip.sha256`, 'same expired release', 'file');
  }
  if (sweep) {
    const selected = selectCurrent([...keep], current);
    assert.ok(selected, 'Sweep requires a retained delivery with a verified binary');
    const delivery = join(output, selected);
    const manifest = JSON.parse(await readFile(join(delivery, 'manifest.json'), 'utf8'));
    const binary = join(delivery, releaseApp(selected), 'Contents/MacOS/pasters-desktop');
    const binaryStat = await safePath(root, binary);
    assert.equal(await hashFile(root, binary), manifest.binarySha256, 'Retained delivery checksum mismatch');
    assert.equal(binaryStat.size, manifest.binaryBytes, 'Retained delivery size mismatch');
    const cutoff = binaryStat.mtimeMs;
    for (const profile of ['debug', 'release']) {
      // Root-level Cargo outputs were missed by the cache-only pass. Sweep
      // only artifacts older than the retained delivery; protect any newer work.
      for (const name of ['libpasters_desktop_lib.a', 'libpasters_desktop_lib.rlib',
        'libpasters_desktop_lib.dylib', 'libpasters_desktop_lib.d', 'pasters-desktop', 'pasters-desktop.d']) {
        const rel = `target/${profile}/${name}`;
        const entry = await safePath(root, join(root, rel));
        if (!entry) continue;
        if (entry.mtimeMs > cutoff) { skipped.push({ path: join(root, rel), reason: 'newer than retained delivery' }); continue; }
        await add(rel, 'superseded loose Cargo output', 'file', `target/${profile}`);
      }
      for (const brand of ['PasteRS', 'CopyRail']) {
        const rel = `target/${profile}/bundle/macos/${brand}.app`;
        const bundledApp = join(root, rel);
        if (await safePath(root, bundledApp)) {
          const state = await treeState(root, bundledApp);
          let archived = false;
          for (const name of keep) {
            const app = join(output, name, releaseApp(name));
            if (await safePath(root, app) && (await treeState(root, app)).digest === state.digest) { archived = true; break; }
          }
          if (archived) await add(rel, 'byte-identical copy of retained release', 'directory', `target/${profile}`);
          else skipped.push({ path: bundledApp, reason: 'bundle differs from retained releases' });
        }
      }
      for (const name of ['dmg', 'share/create-dmg']) {
        const rel = `target/${profile}/bundle/${name}`;
        const path = join(root, rel);
        if (!await safePath(root, path)) continue;
        if ((await treeState(root, path)).newest > cutoff) { skipped.push({ path, reason: 'newer packaging work' }); continue; }
        await add(rel, 'obsolete DMG packaging intermediates', 'directory', `target/${profile}`);
      }
    }
  }
  // Known rebuildable cache subdirectories only. Preserve target root, final
  // binaries, normal bundle, UI reports/screenshots and source fixtures.
  for (const path of cachePaths) await add(path, 'rebuildable compilation cache');
  const bundles = join(root, 'target/debug/bundle/macos');
  if (await safePath(root, bundles)) {
    for (const name of await readdir(bundles)) {
      if (!/^(?:PasteRS|CopyRail) Local QA [a-f0-9]{8}\.app$/.test(name)) continue;
      const plist = join(bundles, name, 'Contents/Info.plist');
      if (!await safePath(root, plist)) continue;
      const xml = await readFile(plist, 'utf8');
      assert.match(xml, /<key>CFBundleIdentifier<\/key>\s*<string>io\.pasters\.localqa\.[a-f0-9]{32}<\/string>/);
      await add(relative(root, join(bundles, name)), 'completed duplicate local QA bundle');
    }
  }
  const benchmarks = join(root, 'target/history-performance');
  if (await safePath(root, benchmarks)) {
    for (const name of await readdir(benchmarks)) {
      if (!/^(synthetic(?:-upgrade)?|probe-compact)-[A-Za-z0-9]{6}$/.test(name)) continue;
      const probe = name.startsWith('probe-compact-');
      if (!probe) {
        const fixture = join(benchmarks, name, 'synthetic-fixture.json');
        if (!await safePath(root, fixture)) continue;
        const info = JSON.parse(await readFile(fixture, 'utf8'));
        assert.equal(info.kind, 'pasters-history-benchmark-v1');
      }
      // Retain metadata and measured reports; only discard regenerable DB bytes.
      // probe_compact() creates a disposable copy without its own metadata file.
      const db = probe ? 'probe.sqlite3' : 'history.sqlite3';
      for (const file of [db, `${db}-wal`, `${db}-shm`]) {
        await add(relative(root, join(benchmarks, name, file)), 'synthetic benchmark database', 'file');
      }
    }
  }
  return { keep: [...keep].sort(newestFirst), skipped, remove };
}

export async function maintain(root, { apply = false, current, sweep = false, processReader = readProcesses, onDelete = () => {} } = {}) {
  const processes = processReader();
  if (apply) assertIdle(processes);
  const plan = await planArtifacts(root, { current, processes, sweep });
  if (!apply) return plan;
  // Preflight all paths before the first deletion. Refresh process and inode
  // checks for each entry, so late-started applications are never killed/deleted.
  for (const item of plan.remove) {
    const entry = await safePath(root, item.path);
    assert.ok(entry && entry.dev === item.dev && entry.ino === item.ino && entry.mtimeMs === item.mtimeMs && entry.size === item.size, `Artifact changed: ${item.path}`);
  }
  for (const item of plan.remove) {
    const live = processReader();
    assertIdle(live);
    assert.equal(inUse(item.guardPath, live), false, `Now running: ${item.path}`);
    const entry = await safePath(root, item.path);
    assert.ok(entry && entry.dev === item.dev && entry.ino === item.ino && entry.mtimeMs === item.mtimeMs && entry.size === item.size, `Artifact changed: ${item.path}`);
    await rm(item.path, { recursive: entry.isDirectory(), force: false });
    onDelete(item);
  }
  return plan;
}

async function main() {
  const args = process.argv.slice(2);
  if (args.join(' ') === '--help') {
    console.log('Usage: node scripts/maintain-artifacts.mjs [--apply] [--sweep] [--current 0.1.0-local-beta.N]\nDefault: preview. Keeps 3 versions total: current/latest + 2 history versions. --sweep also removes superseded loose binaries and archived bundle copies, after checking the retained delivery. Never starts/stops apps, installs or accesses user history.');
    return;
  }
  let apply = false, sweep = false, current;
  while (args.length) {
    const arg = args.shift();
    if (arg === '--apply' && !apply) apply = true;
    else if (arg === '--sweep' && !sweep) sweep = true;
    else if (arg === '--current' && !current) {
      current = args.shift();
      assert.ok(current && releasePattern.test(`PasteRS-${current}`), 'Invalid --current version');
    } else throw Error(`Unsupported argument: ${arg}`);
  }
  const root = await realpath(resolve(dirname(fileURLToPath(import.meta.url)), '..'));
  // Prevent overlapping maintenance, without any scheduler or daemon.
  const lock = join(root, 'target/.artifact-maintenance.lock');
  await safePath(root, lock);
  if (apply) await mkdir(lock);
  try {
    const plan = await maintain(root, { apply, current, sweep,
      onDelete: ({ path }) => console.log(`Deleted: ${path}`) });
    console.log(JSON.stringify({ mode: apply ? 'applied' : 'preview', ...plan }, null, 2));
  } finally {
    if (apply) await rmdir(lock);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
