import { closeSettings, openModels, openTools } from './helpers';
import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import { createHash } from 'node:crypto';
import type { ImageJob, Run, WorkspaceDetail } from '../../apps/web/src/api';

const profile = {
  id: 'klein',
  label: 'Klein test',
  model: 'flux2_klein_4b',
  fingerprint: 'a'.repeat(64),
};
const png = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=',
  'base64',
);

function runReceipt(workspace: string): Run {
  const id = crypto.randomUUID();
  const jobId = crypto.randomUUID();
  const image = {
    id: crypto.randomUUID(),
    workspace_id: workspace,
    image_job_id: jobId,
    media_type: 'image/png' as const,
    bytes: png.length,
    width: 1,
    height: 1,
    sha256: createHash('sha256').update(png).digest('hex'),
    created_at_ms: Date.now(),
  };
  return {
    id,
    workspace_id: workspace,
    status: 'succeeded',
    error: null,
    provider: 'compatible',
    model: 'fixture-model',
    mode: 'chat',
    workspace_tools: true,
    base_revision_id: null,
    execution_result: null,
    execution_revision_id: null,
    node_scope: 'compute',
    input_assets: [],
    tool_execution: null,
    tool_executions: [],
    output: 'Your image is ready.',
    queue_ms: 0,
    duration_ms: 100,
    stats: {
      reported_model: 'fixture-model',
      finish_reason: 'stop',
      input_tokens: 10,
      output_tokens: 10,
      total_tokens: 20,
      first_token_ms: 5,
    },
    search_enabled: false,
    searches: [],
    model_rounds: 2,
    image_profile: profile,
    image_input: null,
    image_job: {
      id: jobId,
      workspace_id: workspace,
      run_id: id,
      call_id: 'image_call',
      prompt: 'A green tree.',
      seed: 123,
      status: 'succeeded',
      error: null,
      created_at_ms: Date.now(),
      finished_at_ms: Date.now(),
      profile,
      output_image: image,
      input_image: null,
      remote_job_id: 'remote_one',
    },
  };
}

async function setup(page: Page, request: APIRequestContext) {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Image generation' } })
  ).json();
  const detail: WorkspaceDetail = await (
    await request.get(`/api/workspaces/${workspace.id}`)
  ).json();
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'fixture-model',
        model_health: 'not_probed',
        search_available: true,
        execution_available: true,
        image_profiles: [profile],
        image_input_available: true,
        backends: [{ id: 'compatible', model: 'fixture-model' }],
      },
    }),
  );
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Ready' } }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  return { workspace, detail };
}

function publish(detail: WorkspaceDetail, run: Run) {
  detail.runs = [run];
  detail.messages = [
    { role: 'user', content: 'Generate an image.', run_id: run.id },
    ...(run.status === 'succeeded'
      ? [{ role: 'assistant' as const, content: run.output, run_id: run.id }]
      : []),
  ];
  detail.image_assets = run.image_job?.output_image ? [run.image_job.output_image] : [];
}

const loraCatalog = [
  {
    id: 'selfie',
    label: 'Selfie',
    description: 'Close-up selfie perspective',
    file: 'selfie.safetensors',
    trigger_words: ['selfie'],
    strength: 0.7,
  },
  {
    id: 'neon',
    label: 'Neon',
    description: 'Neon light style',
    file: 'neon.safetensors',
    trigger_words: ['neon'],
    strength: 0.4,
  },
];

test('WanGP inventory refresh is explicit and malformed responses preserve the previous snapshot', async ({
  page,
  request,
}) => {
  await setup(page, request);
  const preset = { ...profile, loras: [], lora_strengths: '', lora_catalog: loraCatalog };
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'fixture-model',
        model_health: 'not_probed',
        backends: [{ id: 'compatible', model: 'fixture-model' }],
        image_profiles: [preset],
      },
    }),
  );
  let reads = 0;
  await page.route('**/api/image-profiles/klein/inventory', (route) => {
    reads++;
    return route.fulfill({
      json: {
        profile_id: 'klein',
        model: 'flux2_klein_4b',
        available: true,
        capabilities: ['text_to_image', 'lora'],
        models: [{ id: 'flux2_klein_4b', label: 'Klein inventory' }],
        loras: reads === 1 ? ['selfie.safetensors'] : ['../invalid.safetensors'],
      },
    });
  });
  await page.goto('/');
  await openTools(page);
  await openModels(page);
  await page.getByText('Image preset details', { exact: true }).click();
  await expect(page.getByText('Close-up selfie perspective', { exact: false })).toBeVisible();
  await expect(page.getByText('Trigger words: neon', { exact: false })).toBeVisible();
  await openTools(page);
  await openModels(page);
  await page.getByText('WanGP models and LoRAs', { exact: true }).click();
  expect(reads).toBe(0);
  await page.getByRole('button', { name: 'Refresh WanGP' }).click();
  await expect(page.getByText('Checkpoint for flux2_klein_4b:', { exact: false })).toContainText(
    'Available locally',
  );
  await expect(page.getByText('Neon: configured adapter not found locally.')).toBeVisible();
  await page.getByRole('button', { name: 'Refresh WanGP' }).click();
  await expect(page.getByRole('alert')).toContainText('Showing the last successful snapshot');
  await expect(page.getByText('Checkpoint for flux2_klein_4b:', { exact: false })).toContainText(
    'Available locally',
  );
  expect(reads).toBe(2);
});

test('saved image results show the actual adapter combination after reload and reject inconsistent selection', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const run = runReceipt(workspace.id);
  run.image_profile = { ...run.image_profile!, loras: [], lora_catalog: loraCatalog };
  run.image_job!.profile = run.image_profile;
  run.image_job!.generation = {
    loras: ['neon.safetensors', 'selfie.safetensors'],
    lora_strengths: '0.4 0.7',
    selected_lora_ids: ['neon', 'selfie'],
  };
  publish(detail, run);
  await page.route('**/api/image-assets/*/content*', (route) =>
    route.fulfill({ contentType: 'image/png', body: png }),
  );
  await page.goto('/');
  await closeSettings(page);
  const card = page.getByLabel('Image generation result');
  await card.getByRole('button', { name: 'Image job details', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Image window', exact: true })).toContainText(
    'Applied LoRAs: neon.safetensors, selfie.safetensors',
  );
  await expect(page.getByRole('region', { name: 'Image window', exact: true })).toContainText(
    'Applied strengths: 0.4 0.7',
  );
  const imageWindow = page.getByRole('region', { name: 'Image window', exact: true });
  const savedBounds = (await imageWindow.boundingBox())!;
  const imageElement = await imageWindow.locator('.inspected-image').elementHandle();
  await expect(imageWindow.getByRole('button', { name: 'Expand Image' })).toHaveCount(0);
  await expect(imageWindow.getByRole('button', { name: 'Restore Image window' })).toHaveCount(0);
  await card.getByRole('button', { name: 'Expand image', exact: true }).click();
  const stage = (await page.locator('.workspace-stage').boundingBox())!;
  const expandedBounds = (await imageWindow.boundingBox())!;
  expect(expandedBounds.width).toBeGreaterThan(stage.width - 4);
  expect(expandedBounds.height).toBeGreaterThan(stage.height - 4);
  await expect(imageWindow.getByRole('img')).toBeVisible();
  await page.screenshot({ path: 'logs/chat-image-expanded.png' });
  await imageWindow.getByRole('button', { name: 'Close Image', exact: true }).click();
  await card.getByRole('button', { name: 'Image job details', exact: true }).click();
  expect((await imageWindow.boundingBox())!.width).toBeCloseTo(savedBounds.width, 0);
  expect(await imageElement!.evaluate((node) => node.isConnected)).toBe(true);
  await page.screenshot({ path: 'logs/chat-image-details.png' });
  await imageWindow.getByRole('button', { name: 'Close Image', exact: true }).click();
  await card.getByRole('button', { name: 'Expand generated image', exact: true }).click();
  await expect(imageWindow).toHaveClass(/is-expanded/);
  await imageWindow.getByRole('button', { name: 'Close Image', exact: true }).click();
  const summary = page
    .getByLabel('Tool activity')
    .getByText(/^Run details/)
    .first();
  await summary.click();
  await page.getByRole('button', { name: 'Open run details', exact: true }).click();
  const runWindow = page.getByRole('region', { name: 'Run details window', exact: true });
  await expect(runWindow).toContainText(run.id);
  await runWindow.getByText('All saved run data', { exact: true }).click();
  await expect(runWindow).toContainText('"selected_lora_ids"');
  await page.screenshot({ path: 'logs/chat-run-details.png' });
  await runWindow.getByRole('button', { name: 'Close Run details', exact: true }).click();
  await closeSettings(page);
  await page.reload();
  await card.getByRole('button', { name: 'Image job details', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Image window', exact: true })).toContainText(
    'Selected adapter IDs: neon, selfie',
  );
  run.image_job!.generation.selected_lora_ids = ['not-configured'];
  await closeSettings(page);
  await page.reload();
  await expect(page.getByRole('alert')).toContainText('incompatible shape');
  await expect(card).toHaveCount(0);
});

test('changing image presets disposes a pending WanGP discovery read', async ({
  page,
  request,
}) => {
  await setup(page, request);
  const second = {
    ...profile,
    id: 'second-klein',
    label: 'Second Klein',
    fingerprint: 'b'.repeat(64),
  };
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'fixture-model',
        model_health: 'not_probed',
        backends: [{ id: 'compatible', model: 'fixture-model' }],
        image_profiles: [profile, second],
      },
    }),
  );
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let started = false;
  await page.route('**/api/image-profiles/klein/inventory', async (route) => {
    started = true;
    await held;
    await route
      .fulfill({
        json: {
          profile_id: 'klein',
          model: profile.model,
          available: false,
          capabilities: [],
          models: [{ id: profile.model, label: 'Obsolete model snapshot' }],
          loras: [],
        },
      })
      .catch(() => {});
  });
  await page.route('**/api/image-profiles/second-klein/inventory', (route) =>
    route.fulfill({
      json: {
        profile_id: 'second-klein',
        model: profile.model,
        available: true,
        capabilities: ['text_to_image'],
        models: [{ id: profile.model, label: 'Second model snapshot' }],
        loras: [],
      },
    }),
  );
  await page.goto('/');
  await openTools(page);
  await openModels(page);
  await page.getByText('WanGP models and LoRAs', { exact: true }).click();
  await page.getByRole('button', { name: 'Refresh WanGP' }).click();
  await expect.poll(() => started).toBe(true);
  await openTools(page);
  await openModels(page);
  await page.getByLabel('Image generation profile').selectOption('second-klein');
  await openTools(page);
  await openModels(page);
  await page.getByText('WanGP models and LoRAs', { exact: true }).click();
  await expect(page.getByRole('button', { name: 'Refresh WanGP' })).toBeEnabled();
  await page.getByRole('button', { name: 'Refresh WanGP' }).click();
  await expect(page.getByText('Checkpoint for flux2_klein_4b:', { exact: false })).toContainText(
    'Available locally',
  );
  release();
  await expect(page.getByText('Checkpoint for flux2_klein_4b:', { exact: false })).toContainText(
    'Available locally',
  );
  await expect(page.getByText('Obsolete model snapshot', { exact: false })).toHaveCount(0);
});

test('reference image selection is explicit, survives a failed send, and reloads with output provenance', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const reference = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    image_job_id: null,
    name: 'Garden reference.png',
    origin: 'upload' as const,
    media_type: 'image/png' as const,
    bytes: png.length,
    width: 1,
    height: 1,
    sha256: createHash('sha256').update(png).digest('hex'),
    created_at_ms: Date.now(),
  };
  detail.image_assets = [reference];
  const run = runReceipt(workspace.id);
  run.image_input = { role: 'reference', asset: reference };
  run.image_job!.input_image = run.image_input;
  const submissions: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    submissions.push(route.request().postDataJSON());
    if (submissions.length === 1)
      return route.fulfill({ status: 409, json: { error: 'Image job busy' } });
    publish(detail, run);
    detail.image_assets = [reference, run.image_job!.output_image!];
    return route.fulfill({ status: 202, json: run });
  });
  await page.route('**/api/image-assets/*/content*', (route) =>
    route.fulfill({ contentType: 'image/png', body: png }),
  );
  await page.goto('/');
  await closeSettings(page);
  await page.getByRole('checkbox', { name: /^Image generation/ }).check();
  await openTools(page);
  await expect(page.getByLabel('Reference image', { exact: true })).toHaveValue('');
  await openTools(page);
  await page.getByLabel('Reference image', { exact: true }).selectOption(reference.id);
  await closeSettings(page);
  await page.getByLabel('Message', { exact: true }).fill('Make this garden snowy.');
  await closeSettings(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Image job busy');
  await openTools(page);
  await expect(page.getByLabel('Reference image', { exact: true })).toHaveValue(reference.id);
  await closeSettings(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toBeVisible();
  await closeSettings(page);
  expect(submissions[1].image_input).toEqual({ role: 'reference', asset_id: reference.id });
  await closeSettings(page);
  const card = page.getByLabel('Image generation result');
  await card.getByRole('button', { name: 'Image job details', exact: true }).click();
  await expect(
    page
      .getByRole('region', { name: 'Image window', exact: true })
      .getByRole('link', { name: reference.name }),
  ).toHaveAttribute('href', `/api/image-assets/${reference.id}/content`);
  await closeSettings(page);
  await page.reload();
  await card.getByRole('button', { name: 'Image job details', exact: true }).click();
  await expect(
    page
      .getByRole('region', { name: 'Image window', exact: true })
      .getByRole('link', { name: reference.name }),
  ).toBeVisible();
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toBeVisible();
});

test('configured image and computation tools compose with workspace and web access and recover saved results after reload', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const run = runReceipt(workspace.id);
  run.search_enabled = true;
  const submissions: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    submissions.push(route.request().postDataJSON());
    if (submissions.length === 1) {
      return route.fulfill({ status: 409, json: { error: 'Image job busy' } });
    }
    publish(detail, run);
    return route.fulfill({ status: 202, json: run });
  });
  await page.route('**/api/image-assets/*/content*', (route) => {
    return route.fulfill({
      contentType: 'image/png',
      headers: route.request().url().includes('download=true')
        ? { 'content-disposition': 'attachment; filename="image.png"' }
        : {},
      body: png,
    });
  });
  await page.goto('/');
  await closeSettings(page);
  const imageToggle = page.getByRole('checkbox', { name: /^Image generation/ });
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toBeVisible();
  await closeSettings(page);
  await page.getByRole('checkbox', { name: /^Web access/ }).check();
  await imageToggle.check();
  await closeSettings(page);
  await expect(page.getByRole('checkbox', { name: /^Change project files/ })).toBeChecked();
  await closeSettings(page);
  await expect(page.getByRole('checkbox', { name: /^Web access/ })).toBeChecked();
  await closeSettings(page);
  await page.getByLabel('Message', { exact: true }).fill('Generate an image.');
  await closeSettings(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Image job busy');
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toHaveValue('klein');
  await closeSettings(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toBeVisible();
  await closeSettings(page);
  expect(submissions[1]).toMatchObject({
    mode: 'chat',
    image_profile_id: 'klein',
    search: true,
    workspace_tools: true,
  });
  expect(submissions[1].node_scope).toBe('compute');
  await closeSettings(page);
  const card = page.getByLabel('Image generation result');
  await expect(card.getByRole('img')).toBeVisible();
  await expect(card).not.toContainText('succeeded');
  await card.getByRole('button', { name: 'Image job details', exact: true }).click();
  const imageWindow = page.getByRole('region', { name: 'Image window', exact: true });
  await expect(imageWindow).toContainText('succeeded');
  await expect(imageWindow.getByRole('link', { name: 'Download image' })).toHaveAttribute(
    'href',
    `/api/image-assets/${run.image_job!.output_image!.id}/content?download=true`,
  );
  await expect(imageWindow.getByRole('link', { name: 'Download image' })).toHaveAttribute(
    'download',
    `image-${run.image_job!.output_image!.id}.png`,
  );
  await expect(page.getByLabel('App revisions')).toContainText('0 FILES');
  await closeSettings(page);
  await page.reload();
  await expect(page.getByLabel('Image generation result').getByRole('img')).toBeVisible();
  expect(submissions).toHaveLength(2);
});

test('foreign image evidence is rejected without clearing the selected profile or showing a result', async ({
  page,
  request,
}) => {
  const { workspace } = await setup(page, request);
  const run = runReceipt(workspace.id);
  run.image_job!.output_image!.workspace_id = crypto.randomUUID();
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) =>
    route.fulfill({ status: 202, json: run }),
  );
  await page.goto('/');
  await closeSettings(page);
  await page.getByRole('checkbox', { name: /^Image generation/ }).check();
  await closeSettings(page);
  await page.getByLabel('Message', { exact: true }).fill('Generate an image.');
  await closeSettings(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toHaveValue('klein');
  await expect(page.getByLabel('Image generation result')).toHaveCount(0);
});

test('a receipt cannot replace the selected reference with another image', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const reference = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    image_job_id: null,
    name: 'Selected.png',
    origin: 'upload' as const,
    media_type: 'image/png' as const,
    bytes: png.length,
    width: 1,
    height: 1,
    sha256: createHash('sha256').update(png).digest('hex'),
    created_at_ms: Date.now(),
  };
  detail.image_assets = [reference];
  const run = runReceipt(workspace.id);
  run.image_input = { role: 'reference', asset: { ...reference, id: crypto.randomUUID() } };
  run.image_job!.input_image = run.image_input;
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) =>
    route.fulfill({ status: 202, json: run }),
  );
  await page.route('**/api/image-assets/*/content*', (route) =>
    route.fulfill({ contentType: 'image/png', body: png }),
  );
  await page.goto('/');
  await closeSettings(page);
  await page.getByRole('checkbox', { name: /^Image generation/ }).check();
  await openTools(page);
  await page.getByLabel('Reference image', { exact: true }).selectOption(reference.id);
  await closeSettings(page);
  await page.getByLabel('Message', { exact: true }).fill('Edit this image.');
  await closeSettings(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
  await openTools(page);
  await expect(page.getByLabel('Reference image', { exact: true })).toHaveValue(reference.id);
  await expect(page.getByLabel('Image generation result')).toHaveCount(0);
  await closeSettings(page);
  await page.getByRole('checkbox', { name: /^Web access/ }).check();
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toBeVisible();
  await closeSettings(page);
  await page.getByRole('checkbox', { name: /^Image generation/ }).check();
  await closeSettings(page);
  await expect(page.getByRole('checkbox', { name: /^Web access/ })).toBeChecked();
  await openTools(page);
  await expect(page.getByLabel('Reference image', { exact: true })).toHaveValue(reference.id);
});

test('reconciliation restarts owned polling after the parent ends and never displays late image output', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const run = runReceipt(workspace.id);
  run.status = 'failed';
  run.error = 'Image completion is unresolved.';
  const job = run.image_job!;
  job.status = 'unresolved';
  job.error = 'Remote outcome unknown.';
  job.output_image = null;
  job.finished_at_ms = null;
  publish(detail, run);
  let reconciled = false;
  await page.route(`**/api/image-jobs/${job.id}/reconcile`, (route) => {
    reconciled = true;
    job.status = 'cancelling';
    return route.fulfill({ json: job });
  });
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => {
    if (reconciled) {
      job.status = 'cancelled';
      job.finished_at_ms = Date.now();
    }
    return route.fulfill({ json: detail });
  });
  await page.goto('/');
  await closeSettings(page);
  const card = page.getByLabel('Image generation result');
  await expect(card).toContainText('unresolved');
  await card.getByRole('button', { name: 'Reconcile image job' }).click();
  await expect(card).toContainText('Image generation · cancelled');
  await expect(card.getByRole('img')).toHaveCount(0);
});

test('operator resolution requires explicit confirmation and retains the unknown outcome', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const run = runReceipt(workspace.id);
  run.status = 'failed';
  run.error = 'Submission receipt unknown.';
  const job: ImageJob = run.image_job!;
  job.status = 'unresolved';
  job.error = 'Submission receipt unknown.';
  job.remote_job_id = null;
  job.output_image = null;
  job.finished_at_ms = null;
  publish(detail, run);
  let confirmations = 0;
  await page.route(`**/api/image-jobs/${job.id}/confirm-stopped`, (route) => {
    expect(route.request().postDataJSON()).toEqual({ confirm_remote_stopped: true });
    confirmations++;
    job.status = 'interrupted';
    job.error = 'Operator confirmed the runtime stopped. Previous outcome remains unknown.';
    job.finished_at_ms = Date.now();
    return route.fulfill({ json: job });
  });
  await page.goto('/');
  await closeSettings(page);
  const card = page.getByLabel('Image generation result');
  await expect(card.getByRole('button', { name: 'Reconcile image job' })).toHaveCount(0);
  page.once('dialog', (dialog) => dialog.dismiss());
  await card.getByRole('button', { name: 'Resolve after stopping WanGP' }).click();
  expect(confirmations).toBe(0);
  page.once('dialog', (dialog) => dialog.accept());
  await card.getByRole('button', { name: 'Resolve after stopping WanGP' }).click();
  await expect(card).toContainText('Image generation · interrupted');
  await expect(card).toContainText('Previous outcome remains unknown');
  await expect(card.getByRole('img')).toHaveCount(0);
});

test('live image phases show reported steps, stale contact and saved progress after reload', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const run = runReceipt(workspace.id);
  run.status = 'running';
  run.output = '';
  const job = run.image_job!;
  job.status = 'running';
  job.finished_at_ms = null;
  job.output_image = null;
  job.created_at_ms = Date.now() - 18_000;
  job.last_contact_at_ms = Date.now();
  job.progress = { phase: 'inference', current_step: 2, total_steps: 4 };
  publish(detail, run);
  await page.goto('/');
  await closeSettings(page);
  const card = page.getByLabel('Image generation result');
  const bar = card.getByRole('progressbar');
  await expect(card).toContainText('Denoising · 2 of 4 steps');
  await expect(bar).toHaveAttribute('value', '2');
  await expect(bar).toHaveAttribute('max', '4');
  await expect(card).toContainText('Elapsed:');
  await expect(card).toContainText('WanGP last contacted');
  await expect(card.getByRole('img')).toHaveCount(0);
  job.last_contact_at_ms = Date.now() - 20_000;
  await expect(card).toContainText('Updates delayed');
  await closeSettings(page);
  await page.reload();
  await expect(card).toContainText('Denoising · 2 of 4 steps');
  await expect(card).toContainText('Updates delayed');
  job.progress = { phase: 'loading_model', current_step: null, total_steps: null };
  job.last_contact_at_ms = Date.now();
  await expect(card).toContainText('Loading model');
  await expect(bar).not.toHaveAttribute('value');
  await expect(card).not.toContainText('Updates delayed');
  job.progress = { phase: 'inference', current_step: 4, total_steps: 4 };
  await expect(card).toContainText('4 of 4 steps');
  await expect(card).toContainText('Image generation · running');
  await expect(card.getByRole('img')).toHaveCount(0);
  job.status = 'unresolved';
  job.error = 'Remote completion is unknown.';
  run.status = 'failed';
  run.error = 'Remote completion is unknown.';
  await expect(card).toContainText('Last reported phase: Denoising');
  await expect(bar).toHaveCount(0);
  await expect(card).toContainText('Remote completion is unknown.');
});

test('hosted OpenAI jobs show hosted progress wording without WanGP labels', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'fixture-model',
        model_health: 'not_probed',
        image_profiles: [{ ...profile, id: 'flare', label: 'OpenAI Flare', provider: 'openai' }],
        image_input_available: true,
        backends: [{ id: 'compatible', model: 'fixture-model' }],
      },
    }),
  );
  const run = runReceipt(workspace.id);
  run.status = 'running';
  run.output = '';
  const job = run.image_job!;
  job.profile = { ...profile, id: 'flare', label: 'OpenAI Flare', provider: 'openai' };
  run.image_profile = job.profile;
  job.status = 'submitting';
  job.finished_at_ms = null;
  job.output_image = null;
  job.remote_job_id = null;
  publish(detail, run);
  await page.goto('/');
  await closeSettings(page);
  const card = page.getByLabel('Image generation result');
  await expect(card).toContainText('Submitting to OpenAI');
  job.status = 'running';
  job.remote_job_id = 'openai-image-api';
  await expect(card).toContainText('Awaiting the OpenAI response');
  await expect(card).toContainText(
    'OpenAI sends no progress updates; the image arrives when the request completes.',
  );
  await expect(card).not.toContainText('WanGP');
  await expect(card).not.toContainText('seed');
});

test('invalid image progress preserves the last valid workspace observation', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const run = runReceipt(workspace.id);
  run.status = 'running';
  const job = run.image_job!;
  job.status = 'running';
  job.finished_at_ms = null;
  job.output_image = null;
  job.progress = { phase: 'inference', current_step: 1, total_steps: 4 };
  job.last_contact_at_ms = Date.now();
  publish(detail, run);
  await page.goto('/');
  await closeSettings(page);
  const card = page.getByLabel('Image generation result');
  await expect(card).toContainText('1 of 4 steps');
  for (const bad of [
    { phase: 'inference', current_step: 5, total_steps: 4 },
    { phase: 'inference', current_step: -1, total_steps: 4 },
    { phase: 'inference', current_step: 1, total_steps: 0 },
  ] as const) {
    job.progress = bad;
    await page.waitForResponse(async (response) => {
      if (!response.url().endsWith(`/api/workspaces/${workspace.id}`)) return false;
      const value = await response.json();
      const progress = value.runs[0]?.image_job?.progress;
      return (
        progress?.current_step === bad.current_step && progress?.total_steps === bad.total_steps
      );
    });
    await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
    await expect(card).toContainText('1 of 4 steps');
    job.progress = { phase: 'decoding', current_step: null, total_steps: null };
    await expect(card).toContainText('Decoding image');
    job.progress = { phase: 'inference', current_step: 1, total_steps: 4 };
    await expect(card).toContainText('1 of 4 steps');
  }
  job.remote_job_id = null;
  await page.waitForResponse(async (response) => {
    if (!response.url().endsWith(`/api/workspaces/${workspace.id}`)) return false;
    return (await response.json()).runs[0]?.image_job?.remote_job_id === null;
  });
  await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
  await expect(card).toContainText('1 of 4 steps');
});

test('implicit first workspace creation preserves a failed image request and workspace changes retain the configured profile', async ({
  page,
  request,
}) => {
  await setup(page, request);
  await page.route('**/api/workspaces', (route) =>
    route.request().method() === 'GET' ? route.fulfill({ json: [] }) : route.fallback(),
  );
  await page.route('**/api/workspaces/*/runs', (route) => {
    expect(route.request().postDataJSON().image_profile_id).toBe('klein');
    return route.fulfill({ status: 409, json: { error: 'Image job busy' } });
  });
  await page.goto('/');
  await closeSettings(page);
  await page.getByRole('checkbox', { name: /^Image generation/ }).check();
  await closeSettings(page);
  await page.getByLabel('Message', { exact: true }).fill('Create an image.');
  await closeSettings(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Image job busy');
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toHaveValue('klein');
  await page.getByRole('button', { name: 'New workspace' }).click();
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toBeVisible();
});

test('switching workspaces disposes an in-flight image reconciliation action', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await setup(page, request);
  const run = runReceipt(workspace.id);
  run.status = 'failed';
  run.error = 'Remote outcome unknown.';
  const job = run.image_job!;
  job.status = 'unresolved';
  job.error = 'Remote outcome unknown.';
  job.output_image = null;
  job.finished_at_ms = null;
  publish(detail, run);
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let finished!: () => void;
  const finishedResponse = new Promise<void>((resolve) => {
    finished = resolve;
  });
  await page.route(`**/api/image-jobs/${job.id}/reconcile`, async (route) => {
    await gate;
    await route.fulfill({ json: { ...job, status: 'cancelling' } });
    finished();
  });
  await page.goto('/');
  const pendingRequest = page.waitForRequest((request) =>
    request.url().endsWith(`/image-jobs/${job.id}/reconcile`),
  );
  await page.getByRole('button', { name: 'Reconcile image job' }).click();
  await pendingRequest;
  await page.getByRole('button', { name: 'New workspace' }).click();
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toHaveText(
    /^Workspace /,
  );
  release();
  await finishedResponse;
  await expect(page.getByLabel('Image generation result')).toHaveCount(0);
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toBeVisible();
});
