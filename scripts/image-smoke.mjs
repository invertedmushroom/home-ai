import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { parseArgs } from 'node:util';
import { basename, extname } from 'node:path';
import { chromium } from '@playwright/test';

export async function runImageSmoke(values, observe = () => {}) {
  if (!values.url || !values.backend || !values.profile) {
    throw new Error(
      'Usage: node scripts\\image-smoke.mjs --url <Deep URL> --backend <id> --profile <id> [--reference <PNG/JPEG file>] [--output <file>] [--cancel]',
    );
  }
  const url = new URL(values.url);
  if (
    url.protocol !== 'http:' ||
    !['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new Error('Use an explicit loopback HTTP Deep URL.');
  }
  const browser = await chromium.launch({
    executablePath: process.env.DEEP_BROWSER_PATH || undefined,
  });
  try {
    const page = await browser.newPage();
    page.setDefaultTimeout(30_000);
    await page.goto(url.href);
    await page.getByRole('button', { name: 'New workspace' }).click();
    let reference;
    if (values.reference) {
      const bytes = await readFile(values.reference);
      const title = `Reference image smoke ${crypto.randomUUID()}`;
      const workspaceResponse = await page.request.post(new URL('/api/workspaces', url).href, {
        data: { title },
      });
      assert.equal(workspaceResponse.status(), 201);
      const workspace = await workspaceResponse.json();
      const name = basename(values.reference);
      const upload = await page.request.post(
        new URL(
          `/api/workspaces/${workspace.id}/image-assets?id=${crypto.randomUUID()}&name=${encodeURIComponent(name)}`,
          url,
        ).href,
        {
          data: bytes,
          headers: {
            'Content-Type': extname(name).toLowerCase() === '.png' ? 'image/png' : 'image/jpeg',
          },
        },
      );
      assert.equal(upload.status(), 201, await upload.text());
      reference = await upload.json();
      await page.reload();
      await page.getByRole('button', { name: title }).click();
    }
    await page.getByLabel('Model backend').selectOption(values.backend);
    await page.getByRole('checkbox', { name: /^Image generation/ }).check();
    await page.getByLabel('Image generation profile').selectOption(values.profile);
    assert.equal(
      await page.getByRole('checkbox', { name: /^Change project files/ }).isChecked(),
      false,
    );
    if (reference)
      await page.getByLabel('Reference image', { exact: true }).selectOption(reference.id);
    await page
      .getByLabel('Message', { exact: true })
      .fill(
        reference
          ? 'Use the image tool to edit the selected reference image: turn the scene into a snowy winter landscape while preserving its composition. No text.'
          : 'Use the image tool to create an illustration of a small orange robot watering a seedling on a wooden windowsill, soft morning light, no text.',
      );
    const [response] = await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          /\/api\/workspaces\/[^/]+\/runs$/.test(response.url()),
      ),
      page.getByRole('button', { name: /^Send/ }).click(),
    ]);
    assert.equal(response.status(), 202, await response.text());
    const accepted = await response.json();
    observe(accepted);
    assert.equal(accepted.mode, 'chat');
    assert.equal(accepted.provider, values.backend);
    assert.equal(accepted.image_profile.id, values.profile);
    assert.equal(accepted.image_input?.asset.id, reference?.id);
    const deadline = Date.now() + 390_000;
    let run = accepted;
    let stopped = false;
    while (true) {
      const observed = await page.request.get(new URL(`/api/runs/${accepted.id}`, url).href);
      assert.equal(observed.status(), 200);
      run = await observed.json();
      observe(run);
      if (
        values.cancel &&
        !stopped &&
        run.image_job?.status === 'running' &&
        run.image_job.remote_job_id
      ) {
        await page.locator('.composer').getByRole('button', { name: 'Stop request' }).click();
        stopped = true;
      }
      const parentDone = !['queued', 'running'].includes(run.status);
      const imageDone =
        run.image_job &&
        !['planned', 'submitting', 'running', 'cancelling'].includes(run.image_job.status);
      if (parentDone && (imageDone || !run.image_job)) break;
      assert(Date.now() < deadline, 'Image smoke deadline exceeded; inspect its saved receipt.');
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    if (values.cancel) {
      assert(stopped, 'Image finished before the cancellation path could be exercised.');
      assert.equal(run.status, 'cancelled', JSON.stringify(run));
      assert.equal(run.image_job.status, 'cancelled', JSON.stringify(run));
      assert.equal(run.image_job.output_image, null);
      return {
        run_id: run.id,
        image_job_id: run.image_job.id,
        parent: run.status,
        image_job: run.image_job.status,
        late_image_published: false,
      };
    }
    assert.equal(run.status, 'succeeded', JSON.stringify(run));
    assert.equal(run.image_job?.status, 'succeeded', JSON.stringify(run));
    assert.equal(run.model_rounds, 2);
    const image = run.image_job.output_image;
    assert.equal(image.width, 512);
    assert.equal(image.height, 512);
    const card = page.getByLabel('Image generation result');
    await card.getByRole('img').waitFor();
    const dimensions = await card.getByRole('img').evaluate(async (img) => {
      await img.decode();
      return { width: img.naturalWidth, height: img.naturalHeight };
    });
    assert.deepEqual(dimensions, { width: 512, height: 512 });
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      card.getByRole('link', { name: 'Download image' }).click(),
    ]);
    const file = await download.path();
    assert(file, await download.failure());
    const content = await readFile(file);
    assert.equal(content.length, image.bytes);
    assert.equal(createHash('sha256').update(content).digest('hex'), image.sha256);
    if (values.output) await writeFile(values.output, content, { flag: 'wx' });
    await page.reload();
    await page.getByLabel('Image generation result').getByRole('img').waitFor();
    const detail = await (
      await page.request.get(new URL(`/api/workspaces/${run.workspace_id}`, url).href)
    ).json();
    assert.equal(detail.revisions.length, 0);
    assert.equal(detail.assets.length, 0);
    assert.equal(detail.image_assets.length, reference ? 2 : 1);
    if (reference) {
      const saved = detail.runs.find((item) => item.id === run.id);
      assert.deepEqual(saved.image_input.asset, reference);
      assert.deepEqual(saved.image_job.input_image, saved.image_input);
      await page
        .getByLabel('Image generation result')
        .getByRole('link', { name: reference.name })
        .waitFor();
    }
    const reply = detail.messages.find(
      (message) => message.role === 'assistant' && message.run_id === run.id,
    );
    assert(reply?.content.trim(), 'The saved final assistant reply is empty.');
    return {
      run_id: run.id,
      image_job_id: run.image_job.id,
      backend: run.provider,
      model: run.model,
      profile: run.image_profile.id,
      media_type: image.media_type,
      bytes: image.bytes,
      ...dimensions,
      sha256: image.sha256,
      reload_recovered: true,
      input_asset_id: reference?.id,
      project_revisions: detail.revisions.length,
      assistant_reply_saved: true,
    };
  } finally {
    await browser.close();
  }
}

if (import.meta.main) {
  const { values } = parseArgs({
    options: {
      url: { type: 'string' },
      backend: { type: 'string' },
      profile: { type: 'string' },
      output: { type: 'string' },
      reference: { type: 'string' },
      cancel: { type: 'boolean', default: false },
    },
  });
  runImageSmoke(values)
    .then((result) => console.log(JSON.stringify(result)))
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
