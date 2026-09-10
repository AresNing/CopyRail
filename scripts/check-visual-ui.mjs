import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { withCompiledUiTest } from './compiled-ui-test.mjs';

async function assetFingerprint() {
  const root = new URL('../apps/desktop/dist/', import.meta.url);
  const files = (await readdir(root)).filter(name => /\.(wasm|css|js|html)$/.test(name)).sort();
  return Object.fromEntries(await Promise.all(files.map(async name => [name,
    createHash('sha256').update(await readFile(new URL(name, root))).digest('hex'),
  ])));
}
const assetsBefore = await assetFingerprint();

// Evaluate only opaque/reduced-transparency surfaces. This is not a contrast
// claim about arbitrary desktop wallpapers behind native system vibrancy.
function contrastRatios(selectors) {
  const parse = color => {
    const values = color.match(/[\d.]+/g)?.map(Number);
    if (!values || values.length < 3) throw new Error(`Unsupported computed color: ${color}`);
    return [...values.slice(0, 3), values[3] ?? 1];
  };
  const over = (fg, bg) => fg.slice(0, 3).map((v, i) => v * fg[3] + bg[i] * (1 - fg[3]));
  const background = el => {
    if (!el) return [255, 255, 255];
    const style = getComputedStyle(el);
    if (style.backgroundImage !== 'none') throw new Error('Cannot score a gradient as a flat surface');
    const color = parse(style.backgroundColor);
    return color[3] === 1 ? color.slice(0, 3) : over(color, background(el.parentElement));
  };
  const luminance = rgb => rgb.map(v => { const s = v / 255; return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; }).reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
  return selectors.flatMap(selector => {
    const elements = [...document.querySelectorAll(selector)];
    if (!elements.length) throw new Error(`Missing contrast target: ${selector}`);
    return elements.filter(el => !el.disabled && getComputedStyle(el).display !== 'none').map(el => {
      const bg = background(el);
      const fg = over(parse(getComputedStyle(el).color), bg);
      const l = [luminance(bg), luminance(fg)].sort((a, b) => b - a);
      return { selector, text: el.textContent.slice(0, 24), ratio: (l[0] + 0.05) / (l[1] + 0.05) };
    });
  });
}

await withCompiledUiTest(async ({ page, evaluate, waitFor, fixture, screenshot, browser, artifacts }) => {
  const screenshots = [];
  const contrasts = [];
  const media = (theme, reduced = false, more = false) => page('Emulation.setEmulatedMedia', { features: [
    { name: 'prefers-color-scheme', value: theme },
    { name: 'prefers-reduced-transparency', value: reduced ? 'reduce' : 'no-preference' },
    { name: 'prefers-contrast', value: more ? 'more' : 'no-preference' },
    { name: 'prefers-reduced-motion', value: reduced ? 'reduce' : 'no-preference' },
  ] });
  const load = async (compact = false, scale = 1) => {
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: compact ? 148 : 248, deviceScaleFactor: scale, mobile: false });
    await page('Page.navigate', { url: `${fixture}/?fixture=visual${compact ? '&compact=1' : ''}` });
    await waitFor(`document.querySelectorAll('.clip-card').length === 8 && document.querySelector('.card-media')?.naturalWidth === 320 && document.querySelector('.native-test-badge')`);
    assert.equal(await evaluate(`!!document.querySelector('.paste-shell.compact')`), compact);
    assert.equal(await evaluate(`!!document.querySelector('.error-banner')`), false);
  };
  const key = async (key, code, keyCode, modifiers = 0) => {
    await page('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode: keyCode, modifiers });
    await page('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: keyCode, modifiers });
  };
  const checkContrast = async (theme, selectors) => {
    const rows = await evaluate(`(${contrastRatios.toString()})(${JSON.stringify(selectors)})`);
    contrasts.push(...rows.map(row => ({ theme, ...row })));
    for (const row of rows) assert.ok(row.ratio >= 4.5, `${theme}: ${row.selector} ${row.text}: ${row.ratio.toFixed(2)} < 4.5`);
  };
  for (const theme of ['light', 'dark']) {
    await media(theme);
    await load();
    assert.equal(await evaluate(`document.querySelector('.clip-card:nth-child(7) .card-preview').textContent.includes('<img') && !document.querySelector('.clip-card:nth-child(7) .card-content img') && !document.querySelector('img[src="never-load"]')`), true, 'literal HTML stays escaped; a separate source application icon is permitted');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.kind-text .card-kind')).backgroundColor !== getComputedStyle(document.querySelector('.kind-image .card-kind')).backgroundColor`), true);
    assert.equal(await evaluate(`document.querySelector('.kind-image .card-summary').textContent === '320 × 200'`), true);
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('.card-swatch')).map(el => ({text: el.textContent.trim(), color: getComputedStyle(el).backgroundColor}))`), [
      { text: '#58AD97', color: 'rgb(88, 173, 151)' },
      { text: '#17243B', color: 'rgb(23, 36, 59)' },
    ], 'prefixed and bare color codes use the same safe canonical swatch');
    assert.equal(await evaluate(`Array.from(document.querySelectorAll('.clip-card')).every(card => {
      const r = card.getBoundingClientRect(), content = card.querySelector('.card-content').getBoundingClientRect(), header = card.querySelector('.card-header').getBoundingClientRect(), footer = card.querySelector('footer').getBoundingClientRect();
      return r.width === 196 && r.bottom <= innerHeight && header.bottom <= content.top + 1 && content.bottom <= footer.top + 1 && card.scrollWidth <= card.clientWidth + 1;
    })`), true, 'card geometry and bounds');
    screenshots.push(await screenshot(`visual-cards-${theme}` + '.png'));

    await evaluate(`document.querySelector('.clip-card').click()`);
    await key(' ', 'Space', 32);
    await waitFor(`document.querySelector('.preview-overlay .preview-text')`);
    await waitFor(`window.previewFrameFixture.open === true`);
    // A headless page has no native frame. Match the acknowledged reader
    // viewport; actual positioning and restoration require macOS acceptance.
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 608, deviceScaleFactor: 1, mobile: false });
    assert.equal(await evaluate(`document.querySelector('.preview-text').textContent.includes('这不是用户剪贴板')`), true);
    screenshots.push(await screenshot(`visual-preview-${theme}.png`));
    await media(theme, true);
    await checkContrast(theme, ['.preview-overlay > header strong', '.preview-overlay > header span', '.preview-text', '.preview-overlay > footer']);
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.preview-overlay')).backdropFilter`), 'none');
    await key('Escape', 'Escape', 27);
    await waitFor(`!document.querySelector('.preview-overlay') && !window.previewFrameFixture.open && document.activeElement?.id === 'history-results'`);
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 248, deviceScaleFactor: 1, mobile: false });
    await checkContrast(theme, ['.card-kind', '.card-time', '.card-summary', '.card-preview', '.card-swatch', '.content-action', '.organize-button', '.stack-button', '.stack-button strong', '.pinboard.active', '.card-actions button', '.card-actions kbd']);
    await key('ArrowRight', 'ArrowRight', 39, 8);
    await waitFor(`document.querySelector('.selection-count')?.textContent === '已选 2 项'`);
    await checkContrast(theme, ['.selection-count']);
    await media(theme);
    screenshots.push(await screenshot(`visual-multiselect-${theme}.png`));
    await evaluate(`document.querySelector('.clip-card').click(); document.querySelector('.clip-card .stack-toggle').click()`);
    await waitFor(`document.querySelector('.stack-button.active') && document.querySelector('.stack-toggle.active')`);
    await media(theme, true);
    await checkContrast(theme, ['.stack-button.active', '.stack-button.active strong', '.stack-toggle.active']);
    await evaluate(`document.querySelector('.clip-card .stack-toggle').click()`);

    await evaluate(`document.querySelector('[title="编辑选中内容"]').click()`);
    await waitFor(`document.querySelector('.content-editor textarea')`);
    await checkContrast(theme, ['.content-editor > header strong', '.content-editor > header span', '.content-editor textarea', '.content-editor > footer button']);
    await evaluate(`document.querySelector('.save-content').scrollIntoView({ block: 'nearest' })`);
    assert.equal(await evaluate(`document.querySelector('.save-content').getBoundingClientRect().bottom <= innerHeight`), true);
    screenshots.push(await screenshot(`visual-editor-${theme}.png`));
    await key('Escape', 'Escape', 27);
    await waitFor(`!document.querySelector('.content-editor')`);
    await evaluate(`document.querySelector('.settings-button').click()`);
    await waitFor(`document.querySelector('.settings-popover .settings-content')`);
    await page('Emulation.setDeviceMetricsOverride', { width: 1440, height: 608, deviceScaleFactor: 1, mobile: false });
    await checkContrast(theme, ['.settings-popover header', '.permission-actions button', '.backup-actions button']);
    assert.equal(await evaluate(`document.querySelector('.save-settings')===null && document.querySelector('.settings-popover > footer')===null`), true);
    assert.equal(await evaluate(`document.querySelector('.settings-popover').scrollWidth <= document.querySelector('.settings-popover').clientWidth + 1`), true);
    screenshots.push(await screenshot(`visual-settings-${theme}.png`));

    await media(theme, true, true);
    await load();
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.clip-card')).transitionDuration`), '0s');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.clip-card')).borderTopWidth`), '2px');
    screenshots.push(await screenshot(`visual-high-contrast-${theme}.png`));
    for (const scale of [1, 2]) {
      await media(theme);
      await load(true, scale);
      assert.equal(await evaluate(`Array.from(document.querySelectorAll('.clip-card')).every(card => {
        const r = card.getBoundingClientRect(), c = card.querySelector('.card-content').getBoundingClientRect();
        return r.width === 168 && r.bottom <= innerHeight && c.height > 30 && getComputedStyle(card.querySelector('footer')).display === 'none';
      })`), true, 'compact content remains visible');
      screenshots.push(await screenshot(`visual-compact-${theme}-${scale}x.png`));
    }
    await load(false, 2);
    screenshots.push(await screenshot(`visual-cards-${theme}-2x.png`));
  }
  const report = {
    result: 'passed', browser, nativeEndToEnd: false, originalPixelDiff: null,
    checks: ['real compiled WASM', '8 synthetic mixed-content cards', 'light/dark expanded and compact at 1x/2x', 'text escaping', 'prefixed and bare hex use canonical RGB swatches', 'image dimensions', 'preview keyboard open/close', 'multi-selection and active Stack', 'editor/settings controls reachable', 'opaque-surface text contrast >= 4.5', 'reduced transparency and motion', 'high contrast'],
    assetSha256: assetsBefore,
    minimumOpaqueTextContrast: Math.min(...contrasts.map(row => row.ratio)), contrasts, screenshots,
  };
  assert.deepEqual(await assetFingerprint(), assetsBefore, 'compiled assets must not change during verification');
  await writeFile(join(artifacts, 'visual-report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify({ ...report, contrasts: `${contrasts.length} measured text elements` }));
});
