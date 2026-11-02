import { openAssets, selectRevision, openExport, selectCurrent } from './helpers';
import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import type { ImageAsset, Revision, WorkspaceDetail } from '../../apps/web/src/api';

declare global {
  interface Window {
    __assetFixtureScriptRan?: boolean;
    __assetFixtureSvgRan?: boolean;
    __assetFixtureFetch?: Promise<string>;
  }
}

async function image(page: Page, color = '#138a35') {
  const data = await page.evaluate((color) => {
    const canvas = document.createElement('canvas');
    canvas.width = 8;
    canvas.height = 6;
    const context = canvas.getContext('2d')!;
    context.fillStyle = color;
    context.fillRect(0, 0, 8, 6);
    return canvas.toDataURL('image/png').split(',')[1];
  }, color);
  return Buffer.from(data, 'base64');
}

async function workspace(request: APIRequestContext) {
  return (await request.post('/api/workspaces', { data: { title: 'Assets app' } })).json();
}

async function detail(request: APIRequestContext, id: string): Promise<WorkspaceDetail> {
  return (await request.get(`/api/workspaces/${id}`)).json();
}

async function source(request: APIRequestContext, id: string, base: string | null) {
  const proposal = await (
    await request.post(`/api/workspaces/${id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: base },
        changes: [
          {
            op: 'write',
            path: 'index.html',
            content:
              '<!doctype html><html><head><link rel="stylesheet" href="styles/main.css"></head><body><h1>Assets app</h1><img id="hero" src="assets/hero.png"><img id="responsive" srcset="assets/hero.png 1x, assets/hero.png 2x"><div id="background"></div><div id="escaped"></div><div id="bare"></div><div id="comment"></div><img id="vector" src="icons/check.svg"><script src="app.js"></script></body></html>',
          },
          {
            op: 'write',
            path: 'styles/main.css',
            content: String.raw`#background { width:8px; height:6px; background-image:url('../assets/hero.png'); }
#escaped { background-image:url('../assets/he\72 o.png'); }
#bare { background-image:url(../assets/he\72 o.png); }
#comment { background-image:url('../assets/hero.png' /**/); }
#background::after { content:"url('../assets/hero.png')"; }
/* url('../assets/not-bound.png') is only a comment. */`,
          },
          {
            op: 'write',
            path: 'icons/check.svg',
            content:
              '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><script>window.__assetFixtureSvgRan=true;</script><rect width="8" height="6" fill="green"/></svg>',
          },
          {
            op: 'write',
            path: 'app.js',
            content:
              'window.__assetFixtureScriptRan=true; window.__assetFixtureFetch=fetch("http://127.0.0.1:14318/api/status").then(()=>"unexpected").catch(()=>"blocked");',
          },
        ],
      },
    })
  ).json();
  const result = await request.post(`/api/proposals/${proposal.id}/apply`);
  expect(result.ok()).toBeTruthy();
  return result.json() as Promise<Revision>;
}

async function upload(page: Page, request: APIRequestContext, id: string, png: Buffer) {
  await openAssets(page);
  const library = page.getByLabel('Workspace assets');
  await library.getByLabel('Upload image', { exact: true }).setInputFiles({
    name: 'hero.png',
    mimeType: 'image/png',
    buffer: png,
  });
  await expect(library.getByText('Image saved to the workspace library.')).toBeVisible();
  await expect(library.getByText('hero.png', { exact: true })).toBeVisible();
  return (await detail(request, id)).image_assets!.at(-1)!;
}

async function bind(page: Page, asset: ImageAsset) {
  const row = page.getByLabel(`Image asset ${asset.id}`, { exact: true });
  await row.getByLabel(`Project path for ${asset.id}`).fill('assets/hero.png');
  await row.getByRole('button', { name: 'Use in project', exact: true }).click();
  await expect(page.getByText('Review proposed changes', { exact: true })).toBeVisible();
}

test('closing the asset tool during an upload preserves the operation and saved receipt', async ({
  page,
  request,
}) => {
  const bytes = await image(page);
  const w = await workspace(request);
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let started!: () => void;
  const uploading = new Promise<void>((resolve) => {
    started = resolve;
  });
  let submissions = 0;
  await page.route(`**/api/workspaces/${w.id}/image-assets?*`, async (route) => {
    submissions++;
    const response = await route.fetch();
    started();
    await held;
    await route.fulfill({ response });
  });
  await page.goto('/');
  await openAssets(page);
  const library = page.getByLabel('Workspace assets');
  await library
    .getByLabel('Upload image', { exact: true })
    .setInputFiles({ name: 'retained.png', mimeType: 'image/png', buffer: bytes });
  await uploading;
  await page.getByRole('button', { name: 'Close Files & assets', exact: true }).click();
  release();
  await openAssets(page);
  await expect(library.getByText('Image saved to the workspace library.')).toBeVisible();
  await expect(library.getByText('retained.png', { exact: true })).toBeVisible();
  expect(submissions).toBe(1);
  expect((await detail(request, w.id)).image_assets).toHaveLength(1);
});

test('uploaded images become reviewed project bindings, render safely in captured previews and export real portable files', async ({
  page,
  request,
}) => {
  const png = await image(page);
  const w = await workspace(request);
  await page.goto('/');
  const asset = await upload(page, request, w.id, png);
  await bind(page, asset);
  expect((await detail(request, w.id)).revisions).toHaveLength(0);
  await expect(page.getByLabel('Asset binding diff')).toContainText(asset.id);
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  const bindingRevision = (await detail(request, w.id)).revisions.at(-1)!;
  expect(bindingRevision.files).toHaveLength(0);
  expect(bindingRevision.asset_bindings).toEqual([{ path: 'assets/hero.png', asset_id: asset.id }]);
  const appRevision = await source(request, w.id, bindingRevision.id);
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await selectCurrent(page);
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  const frame = page.frameLocator('iframe[title="Generated app preview"]');
  await expect(frame.locator('#hero')).toBeVisible();
  await expect
    .poll(() => frame.locator('#hero').evaluate((img: HTMLImageElement) => img.naturalWidth))
    .toBe(8);
  const firstUrl = await frame.locator('#hero').getAttribute('src');
  expect(firstUrl).toMatch(/^blob:null\//);
  await expect
    .poll(() => frame.locator('#responsive').evaluate((img: HTMLImageElement) => img.naturalWidth))
    .toBeGreaterThan(0);
  await expect
    .poll(() => frame.locator('#vector').evaluate((img: HTMLImageElement) => img.naturalWidth))
    .toBe(8);
  expect(
    await frame.locator('#background').evaluate((node) => getComputedStyle(node).backgroundImage),
  ).toContain('blob:null/');
  expect(
    await frame.locator('#escaped').evaluate((node) => getComputedStyle(node).backgroundImage),
  ).toContain('blob:null/');
  expect(
    await frame.locator('#bare').evaluate((node) => getComputedStyle(node).backgroundImage),
  ).toContain('blob:null/');
  expect(
    await frame.locator('#comment').evaluate((node) => getComputedStyle(node).backgroundImage),
  ).toContain('blob:null/');
  expect(
    await frame
      .locator('#background')
      .evaluate((node) => getComputedStyle(node, '::after').content),
  ).toContain('../assets/hero.png');
  const isolation = await frame.locator('body').evaluate(async () => ({
    origin: window.origin,
    script: window.__assetFixtureScriptRan,
    svgScript: window.__assetFixtureSvgRan ?? false,
    fetch: await window.__assetFixtureFetch,
  }));
  expect(isolation).toEqual({ origin: 'null', script: true, svgScript: false, fetch: 'blocked' });
  await expect(page.locator('iframe[title="Generated app preview"]')).toHaveAttribute(
    'sandbox',
    'allow-scripts',
  );

  const secondBytes = await image(page, '#d82424');
  const secondId = crypto.randomUUID();
  const uploaded = await request.post(
    `/api/workspaces/${w.id}/image-assets?id=${secondId}&name=second.png`,
    {
      data: secondBytes,
      headers: { 'Content-Type': 'image/png' },
    },
  );
  expect(uploaded.ok()).toBeTruthy();
  const replacement = await (
    await request.post(`/api/workspaces/${w.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: appRevision.id },
        changes: [],
        asset_changes: [{ op: 'bind', path: 'assets/hero.png', asset_id: secondId }],
      },
    })
  ).json();
  expect((await request.post(`/api/proposals/${replacement.id}/apply`)).ok()).toBeTruthy();
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  expect(await frame.locator('#hero').getAttribute('src')).toBe(firstUrl);
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  await expect.poll(() => frame.locator('#hero').getAttribute('src')).not.toBe(firstUrl);

  await selectRevision(page, appRevision.id);
  await openExport(page);
  const [download] = await Promise.all([
    page.waitForEvent('download'),
    page.getByRole('button', { name: 'Download app bundle (.zip)', exact: true }).click(),
  ]);
  const zip = await readFile((await download.path())!);
  expect(zip.readUInt32LE(0)).toBe(0x04034b50);
  expect(zip.includes(Buffer.from('assets/hero.png'))).toBe(true);
  expect(zip.includes(png)).toBe(true);
  expect(zip.includes(secondBytes)).toBe(false);
  expect((await detail(request, w.id)).revisions[0].asset_bindings![0].asset_id).toBe(asset.id);
});

test('a lost image upload response retries the same immutable identity instead of duplicating assets', async ({
  page,
  request,
}) => {
  const bytes = await image(page);
  const w = await workspace(request);
  const ids: string[] = [];
  await page.route(`**/api/workspaces/${w.id}/image-assets?*`, async (route) => {
    ids.push(new URL(route.request().url()).searchParams.get('id')!);
    if (ids.length === 1) {
      await route.fetch();
      return route.fulfill({ status: 503, json: { error: 'Upload receipt lost' } });
    }
    return route.fallback();
  });
  await page.goto('/');
  await openAssets(page);
  const library = page.getByLabel('Workspace assets');
  await library.getByLabel('Upload image', { exact: true }).setInputFiles({
    name: 'hero.png',
    mimeType: 'image/png',
    buffer: bytes,
  });
  await expect(library.getByRole('alert')).toContainText('Upload receipt lost');
  await library.getByRole('button', { name: 'Retry image upload' }).click();
  await expect(library.getByText('hero.png', { exact: true })).toBeVisible();
  expect(ids).toHaveLength(2);
  expect(ids[0]).toBe(ids[1]);
  expect((await detail(request, w.id)).image_assets).toHaveLength(1);
});

test('a lost asset proposal response preserves exact binding, submission ID and captured base', async ({
  page,
  request,
}) => {
  const bytes = await image(page);
  const w = await workspace(request);
  await page.goto('/');
  const asset = await upload(page, request, w.id, bytes);
  const inputs: unknown[] = [];
  await page.route(`**/api/workspaces/${w.id}/proposals`, async (route) => {
    inputs.push(route.request().postDataJSON());
    if (inputs.length === 1) {
      await route.fetch();
      return route.fulfill({ status: 503, json: { error: 'Binding receipt lost' } });
    }
    return route.fallback();
  });
  const row = page.getByLabel(`Image asset ${asset.id}`, { exact: true });
  await row.getByLabel(`Project path for ${asset.id}`).fill('assets/hero.png');
  await row.getByRole('button', { name: 'Use in project', exact: true }).click();
  await openAssets(page);
  const library = page.getByLabel('Workspace assets');
  await expect(library.getByRole('alert')).toContainText('Binding receipt lost');
  await library.getByRole('button', { name: 'Retry asset proposal' }).click();
  await expect(page.getByText('Review proposed changes', { exact: true })).toBeVisible();
  expect(inputs).toHaveLength(2);
  expect(inputs[1]).toEqual(inputs[0]);
  expect((await detail(request, w.id)).proposals).toHaveLength(1);
  expect((await detail(request, w.id)).revisions).toHaveLength(0);
});

test('changing workspaces aborts an in-flight image preview capture without publishing a stale frame', async ({
  page,
  request,
}) => {
  const bytes = await image(page);
  const w = await workspace(request);
  await page.goto('/');
  const asset = await upload(page, request, w.id, bytes);
  await bind(page, asset);
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await source(request, w.id, (await detail(request, w.id)).revisions.at(-1)!.id);
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await selectCurrent(page);
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let reached!: () => void;
  const waiting = new Promise<void>((resolve) => {
    reached = resolve;
  });
  let finished!: () => void;
  const complete = new Promise<void>((resolve) => {
    finished = resolve;
  });
  await page.route(`**/api/image-assets/${asset.id}/content`, async (route) => {
    if (route.request().resourceType() !== 'fetch') return route.fallback();
    reached();
    await gate;
    await route.fulfill({ body: bytes, contentType: 'image/png' });
    finished();
  });
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  await waiting;
  await page.getByRole('button', { name: 'New workspace' }).click();
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toHaveText(
    /^Workspace /,
  );
  release();
  await complete;
  await expect(page.locator('iframe[title="Generated app preview"]')).toHaveCount(0);
  await openAssets(page);
  await expect(page.getByLabel('Workspace assets')).toContainText('Workspace library · 0');
});

test('cloning a pending binding proposal into a text editor draft preserves its exact asset reference', async ({
  page,
  request,
}) => {
  const bytes = await image(page);
  const w = await workspace(request);
  await page.goto('/');
  const asset = await upload(page, request, w.id, bytes);
  await bind(page, asset);
  const original = (await detail(request, w.id)).proposals[0];
  await page.getByRole('button', { name: 'Expand Editor', exact: true }).click();
  const close = page.getByRole('button', { name: 'Close Files & assets', exact: true });
  if (await close.isVisible()) await close.click();
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await page.getByLabel('New file path').fill('index.html');
  await page.getByRole('button', { name: 'Add file', exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'File editor', exact: true });
  await editor.focus();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.insertText('<html><img id="hero" src="assets/hero.png"></html>');
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expect(page.getByText('Review proposed changes', { exact: true })).toBeVisible();
  const before = await detail(request, w.id);
  const cloned = before.proposals.find((proposal) => proposal.id !== original.id)!;
  expect(cloned.asset_changes).toEqual(original.asset_changes);
  expect(cloned.changes).toHaveLength(1);
  expect(before.proposals.find((proposal) => proposal.id === original.id)!.changes).toHaveLength(0);
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  const frame = page.frameLocator('iframe[title="Generated app preview"]');
  await expect
    .poll(() => frame.locator('#hero').evaluate((img: HTMLImageElement) => img.naturalWidth))
    .toBe(8);
  expect((await detail(request, w.id)).revisions[0].asset_bindings).toEqual([
    { path: 'assets/hero.png', asset_id: asset.id },
  ]);
});

test('an older API leaves the library readable and explains why new asset actions are unavailable', async ({
  page,
  request,
}) => {
  await workspace(request);
  await page.route('**/api/status', async (route) => {
    const response = await route.fetch();
    const status = await response.json();
    delete status.asset_library_available;
    await route.fulfill({ json: status });
  });
  await page.goto('/');
  await openAssets(page);
  const library = page.getByLabel('Workspace assets');
  await expect(library).toContainText('Restart the Rust server');
  await expect(library.getByLabel('Upload image', { exact: true })).toBeDisabled();
  await page.route('**/api/workspaces/*/proposals', (route) => {
    expect(Object.hasOwn(route.request().postDataJSON(), 'asset_changes')).toBe(false);
    return route.fallback();
  });
  await page.getByRole('button', { name: 'Expand Editor', exact: true }).click();
  const close = page.getByRole('button', { name: 'Close Files & assets', exact: true });
  if (await close.isVisible()) await close.click();
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await page.getByLabel('New file path').fill('index.html');
  await page.getByRole('button', { name: 'Add file', exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'File editor', exact: true });
  await editor.focus();
  await page.keyboard.insertText('<h1>Legacy-compatible text edit</h1>');
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expect(page.getByText('Review proposed changes', { exact: true })).toBeVisible();
});
