import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { basename } from 'node:path';
import { parseArgs } from 'node:util';
import { chromium } from '@playwright/test';

const { values } = parseArgs({
  options: {
    url: { type: 'string' },
    backend: { type: 'string' },
    image: { type: 'string' },
    output: { type: 'string' },
  },
});

async function main() {
  if (!values.url || !values.backend || !values.image || !values.output)
    throw new Error(
      'Usage: node scripts\\asset-smoke.mjs --url <Deep URL> --backend <id> --image <PNG/JPEG> --output <new ZIP file>',
    );
  const url = new URL(values.url);
  if (
    url.protocol !== 'http:' ||
    !['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  )
    throw new Error('Use an explicit loopback HTTP Deep URL.');
  const bytes = await readFile(values.image);
  assert(bytes.length > 0 && bytes.length <= 8_388_608);
  const digest = createHash('sha256').update(bytes).digest('hex');
  const browser = await chromium.launch({
    executablePath: process.env.DEEP_BROWSER_PATH || undefined,
  });
  try {
    const page = await browser.newPage();
    page.setDefaultTimeout(30_000);
    await page.goto(url.href);
    const [created] = await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname === '/api/workspaces',
      ),
      page.getByRole('button', { name: 'New workspace' }).click(),
    ]);
    assert.equal(created.status(), 201);
    const workspace = await created.json();
    const library = page.getByLabel('Workspace assets');
    await library.getByLabel('Upload image', { exact: true }).setInputFiles(values.image);
    await library.getByText('Image saved to the workspace library.').waitFor();
    const getDetail = async () => {
      const response = await page.request.get(new URL(`/api/workspaces/${workspace.id}`, url).href);
      assert.equal(response.status(), 200);
      return response.json();
    };
    let detail = await getDetail();
    assert.equal(detail.image_assets.length, 1);
    assert.equal(detail.runs.length, 0);
    const asset = detail.image_assets[0];
    assert.equal(asset.name, basename(values.image));
    assert.equal(asset.sha256, digest);
    const projectPath = `assets/hero.${asset.media_type === 'image/png' ? 'png' : 'jpg'}`;
    const row = page.getByLabel(`Image asset ${asset.id}`, { exact: true });
    await row.getByLabel(`Project path for ${asset.id}`).fill(projectPath);
    await row.getByRole('button', { name: 'Use in project', exact: true }).click();
    await page.getByText('Review proposed changes', { exact: true }).waitFor();
    async function applySelected() {
      const [response] = await Promise.all([
        page.waitForResponse(
          (response) =>
            response.request().method() === 'POST' &&
            /\/api\/proposals\/[^/]+\/apply$/.test(new URL(response.url()).pathname),
        ),
        page.getByRole('button', { name: 'Apply changes', exact: true }).click(),
      ]);
      assert.equal(response.status(), 200, await response.text());
      return response.json();
    }
    await applySelected();
    detail = await getDetail();
    const binding = [{ path: projectPath, asset_id: asset.id }];
    assert.deepEqual(detail.revisions.at(-1).asset_bindings, binding);
    assert.equal(detail.revisions.at(-1).files.length, 0);
    await page.getByLabel('Model backend').selectOption(values.backend);
    await page.getByLabel('Response mode').selectOption('files');

    async function generate(prompt) {
      await page.getByLabel('Message', { exact: true }).fill(prompt);
      const [response] = await Promise.all([
        page.waitForResponse(
          (response) =>
            response.request().method() === 'POST' &&
            new URL(response.url()).pathname === `/api/workspaces/${workspace.id}/runs`,
        ),
        page.getByRole('button', { name: /^Send/ }).click(),
      ]);
      assert.equal(response.status(), 202, await response.text());
      const accepted = await response.json();
      const deadline = Date.now() + 210_000;
      while (true) {
        const run = await (
          await page.request.get(new URL(`/api/runs/${accepted.id}`, url).href)
        ).json();
        if (!['queued', 'running'].includes(run.status)) {
          assert.equal(run.status, 'succeeded', JSON.stringify(run));
          break;
        }
        assert(Date.now() < deadline, 'Source generation did not settle.');
        await new Promise((resolve) => setTimeout(resolve, 200));
      }
      const current = await getDetail();
      const proposal = current.proposals.find((proposal) => proposal.run_id === accepted.id);
      assert(proposal);
      assert.equal(proposal.asset_changes.length, 0);
      await page.getByLabel('Proposed changes').getByRole('button').last().click();
      await applySelected();
      return accepted.id;
    }

    const initialRun = await generate(
      `Create a small polished browser app featuring the existing approved image at "${projectPath}". ` +
        'Use an img element with id="hero". Include a heading, a short caption and a button. ' +
        'Use relative classic scripts/styles and no external resources. Refer to the image by its existing project path only. ' +
        'Do not write the image file, copy its bytes, embed base64, or change asset bindings.',
    );
    await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
    const frame = page.frameLocator('iframe[title="Generated app preview"]');
    await frame.locator('#hero').waitFor();
    const first = await frame.locator('#hero').evaluate(async (img) => {
      await img.decode();
      return { width: img.naturalWidth, height: img.naturalHeight, src: img.getAttribute('src') };
    });
    assert.equal(first.width, asset.width);
    assert.equal(first.height, asset.height);
    assert(first.src.startsWith('blob:null/'));

    const editRun = await generate(
      `Change the main heading to exactly "Asset reuse verified". Preserve the existing img with id="hero" ` +
        `and image path "${projectPath}". Do not change any image asset or binding.`,
    );
    detail = await getDetail();
    assert.deepEqual(detail.revisions.at(-1).asset_bindings, binding);
    assert.equal(detail.image_assets.length, 1);
    assert.equal(detail.image_assets[0].sha256, digest);
    await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
    await frame.getByRole('heading', { name: 'Asset reuse verified', exact: true }).waitFor();
    const editedImage = await frame.locator('#hero').evaluate(async (img) => {
      await img.decode();
      return { width: img.naturalWidth, height: img.naturalHeight };
    });
    assert.deepEqual(editedImage, { width: asset.width, height: asset.height });

    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.getByRole('button', { name: 'Download app bundle (.zip)', exact: true }).click(),
    ]);
    const file = await download.path();
    assert(file, await download.failure());
    const zip = await readFile(file);
    assert.equal(zip.readUInt32LE(0), 0x04034b50);
    assert(zip.includes(Buffer.from(projectPath)));
    assert(zip.includes(bytes));
    assert(zip.includes(Buffer.from('Asset reuse verified')));
    await writeFile(values.output, zip, { flag: 'wx' });
    await page.reload();
    await page.getByLabel(`Image asset ${asset.id}`, { exact: true }).waitFor();
    detail = await getDetail();
    assert.deepEqual(detail.revisions.at(-1).asset_bindings, binding);
    console.log(
      JSON.stringify({
        workspace_id: workspace.id,
        asset_id: asset.id,
        source_runs: [initialRun, editRun],
        asset_sha256: digest,
        project_path: projectPath,
        revisions: detail.revisions.length,
        zip_bytes: zip.length,
        zip_sha256: createHash('sha256').update(zip).digest('hex'),
        reload_recovered: true,
      }),
    );
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
