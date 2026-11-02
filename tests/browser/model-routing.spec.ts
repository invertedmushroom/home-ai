import { closeSettings, openModels, openTools, selectModel, selectTask } from './helpers';
import { test, expect } from '@playwright/test';

const catalog = {
  profiles: [
    {
      id: 'chat',
      label: 'Chat profile',
      backend: 'lmstudio',
      model: 'chat-model',
      tool_calls: true,
    },
    {
      id: 'code',
      label: 'Code profile',
      backend: 'lmstudio',
      model: 'code-model',
      tool_calls: true,
    },
  ],
  preferences: { conversation: ['chat'], coding: ['code'] },
};
const status = {
  provider: 'lmstudio',
  model: 'default-model',
  model_health: 'not_probed',
  backends: [{ id: 'lmstudio', model: 'default-model' }],
  model_catalog: catalog,
};

test('Auto task selection, explicit profile override and provenance survive reload', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Model profiles' } })
  ).json();
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const submissions: Record<string, unknown>[] = [];
  await page.route('**/api/status', (route) => route.fulfill({ json: status }));
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    const input = route.request().postDataJSON();
    submissions.push(input);
    const selection = input.model_selection;
    const profile = catalog.profiles.find(
      (p) =>
        p.id ===
        (selection.mode === 'profile'
          ? selection.profile_id
          : catalog.preferences[selection.task as 'conversation' | 'coding'][0]),
    )!;
    const run = {
      id: crypto.randomUUID(),
      workspace_id: workspace.id,
      status: 'succeeded',
      error: null,
      provider: 'lmstudio',
      model: profile.model,
      mode: 'chat',
      workspace_tools: input.workspace_tools,
      output: `Reply ${submissions.length}`,
      base_revision_id: null,
      queue_ms: 0,
      duration_ms: 5,
      stats: {
        reported_model: profile.model,
        finish_reason: 'stop',
        input_tokens: null,
        output_tokens: null,
        total_tokens: null,
        first_token_ms: null,
      },
      model_route: {
        profile_id: profile.id,
        label: profile.label,
        backend: profile.backend,
        model: profile.model,
        automatic: selection.mode === 'auto',
        task: selection.mode === 'auto' ? selection.task : null,
        reason: 'Configured task preference.',
      },
    };
    detail.runs.push(run);
    detail.messages.push(
      { role: 'user', content: input.prompt, run_id: run.id },
      { role: 'assistant', content: run.output, run_id: run.id },
    );
    return route.fulfill({ status: 202, json: run });
  });
  await page.goto('/');
  await selectModel(page, 'auto');
  await selectTask(page, 'coding');
  await page.getByLabel('Message', { exact: true }).fill('Use the coding preference');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.getByText('Reply 1', { exact: true })).toBeVisible();
  expect(submissions[0].backend).toBeUndefined();
  expect(submissions[0].model_selection).toEqual({ mode: 'auto', task: 'coding' });
  await expect(page.locator('.run-stats')).toContainText('Auto (coding) · Code profile');
  await selectModel(page, 'profile:chat');
  await expect(page.getByLabel('Task preference')).toHaveCount(0);
  await page.getByLabel('Message', { exact: true }).fill('Explicit profile');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.getByText('Reply 2', { exact: true })).toBeVisible();
  expect(submissions[1].model_selection).toEqual({ mode: 'profile', profile_id: 'chat' });
  await page.reload();
  await expect(page.locator('.run-stats').filter({ hasText: 'Auto (coding)' })).toContainText(
    'Code profile',
  );
  await expect(
    page.locator('.message-label').getByText('lmstudio · chat-model', { exact: true }),
  ).toBeVisible();
});

test('unknown profile references cannot publish a model catalog', async ({ page }) => {
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        ...status,
        model_catalog: { ...catalog, preferences: { conversation: ['missing'], coding: [] } },
      },
    }),
  );
  await page.goto('/');
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'incompatible shape',
  );
  await expect(page.getByLabel('Model backend')).toBeDisabled();
});

test('a downgraded Auto receipt preserves the unsent request', async ({ page, request }) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Bad Auto receipt' } })
  ).json();
  await page.route('**/api/status', (route) => route.fulfill({ json: status }));
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) =>
    route.fulfill({
      status: 202,
      json: {
        id: crypto.randomUUID(),
        workspace_id: workspace.id,
        status: 'queued',
        error: null,
        provider: 'lmstudio',
        model: 'default-model',
        mode: 'chat',
        workspace_tools: true,
        output: '',
        base_revision_id: null,
        queue_ms: 0,
        duration_ms: null,
        stats: {
          reported_model: null,
          finish_reason: null,
          input_tokens: null,
          output_tokens: null,
          total_tokens: null,
          first_token_ms: null,
        },
      },
    }),
  );
  await page.goto('/');
  await selectModel(page, 'auto');
  await page.getByLabel('Message', { exact: true }).fill('Keep this request');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'incompatible shape',
  );
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Keep this request');
});

test('named runtimes use manual profiles and inventory refresh preserves snapshots and rejects stale reads', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Runtime inventory' } })
  ).json();
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const connections = [
    { id: 'desk-text', label: 'Desk PC', kind: 'lmstudio', discovery: true },
    { id: 'linux-text', label: 'Linux PC', kind: 'lmstudio', discovery: true },
  ];
  const profiles = [
    {
      id: 'saluki',
      label: 'Saluki 27B',
      backend: 'desk-text',
      model: 'saluki-model',
      tool_calls: true,
    },
    {
      id: 'qwen',
      label: 'Qwen distill',
      backend: 'linux-text',
      model: 'qwen-model',
      tool_calls: true,
    },
  ];
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'desk-text',
        model: 'saluki-model',
        model_health: 'not_probed',
        connections,
        model_discovery_available: true,
        backends: profiles.map((p) => ({
          id: p.backend,
          model: p.model,
          kind: 'lmstudio',
          profile_required: true,
        })),
        model_catalog: {
          profiles,
          default_profile: 'saluki',
          preferences: { conversation: [], coding: [] },
        },
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  let reads = 0;
  let release: () => void = () => {};
  const wait = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route('**/api/connections/desk-text/models', async (route) => {
    reads++;
    await wait;
    await route
      .fulfill({
        json: {
          connection_id: 'desk-text',
          status: 'available',
          message: null,
          models: [
            {
              id: 'late',
              label: 'Stale desk model',
              state: 'loaded',
              tool_calls: true,
              vision: false,
              context_length: 4096,
              max_context_length: 4096,
            },
          ],
        },
      })
      .catch(() => {});
  });
  let linuxReads = 0;
  await page.route('**/api/connections/linux-text/models', (route) => {
    reads++;
    linuxReads++;
    return route.fulfill({
      json: {
        connection_id: linuxReads === 1 ? 'linux-text' : 'wrong-runtime',
        status: 'available',
        message: null,
        models: [
          {
            id: 'qwen-model',
            label: 'Observed Qwen',
            state: 'unloaded',
            tool_calls: true,
            vision: true,
            context_length: null,
            max_context_length: 262144,
          },
        ],
      },
    });
  });
  await page.goto('/');
  await expect(page.getByLabel('Model backend')).toHaveValue('profile:saluki');
  await expect(page.getByLabel('Task preference')).toHaveCount(0);
  expect(reads).toBe(0);
  await openTools(page);
  await openModels(page);
  await page.getByText('Runtime models', { exact: true }).click();
  await page.getByRole('button', { name: 'Refresh models', exact: true }).click();
  await expect.poll(() => reads).toBe(1);
  await page.getByLabel('Runtime connection').selectOption('linux-text');
  release();
  await page.getByRole('button', { name: 'Refresh models', exact: true }).click();
  await expect(page.getByText('Observed Qwen', { exact: true })).toBeVisible();
  await expect(page.getByText('Stale desk model', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Refresh models', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('last successful snapshot');
  await expect(page.getByText('Observed Qwen', { exact: true })).toBeVisible();
  let submission: Record<string, unknown> | undefined;
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    submission = route.request().postDataJSON();
    return route.fulfill({ status: 400, json: { error: 'Fixture preserves this request.' } });
  });
  await closeSettings(page);
  await selectModel(page, 'profile:qwen');
  await page.getByLabel('Message', { exact: true }).fill('Manual Qwen request');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect
    .poll(() => submission?.model_selection)
    .toEqual({ mode: 'profile', profile_id: 'qwen' });
  expect(submission?.backend).toBeUndefined();
});

test('image presets show compatible LoRA choices on the same base model', async ({ page }) => {
  const presets = [
    {
      id: 'base',
      label: 'FLUX Default',
      model: 'flux2_klein_4b',
      fingerprint: 'a'.repeat(64),
      loras: [],
      lora_strengths: null,
    },
    {
      id: 'portrait',
      label: 'FLUX Portrait',
      model: 'flux2_klein_4b',
      fingerprint: 'b'.repeat(64),
      loras: ['portrait.safetensors'],
      lora_strengths: '0.8',
    },
  ];
  await page.route('**/api/status', (route) =>
    route.fulfill({ json: { ...status, image_profiles: presets } }),
  );
  await page.goto('/');
  await expect(page.locator('.composer select')).toHaveCount(0);
  const newButton = page.getByRole('button', { name: 'New workspace', exact: true });
  await expect(newButton.locator('xpath=following-sibling::button[1]')).toHaveAttribute(
    'aria-label',
    'Models',
  );
  await openTools(page);
  await openModels(page);
  await page.getByLabel('Image generation profile').selectOption('portrait');
  await openModels(page);
  await page.getByText('Image preset details', { exact: true }).click();
  await expect(page.getByText('Base model: flux2_klein_4b', { exact: true })).toBeVisible();
  await expect(page.getByText('LoRAs: portrait.safetensors', { exact: true })).toBeVisible();
  await expect(page.getByText('Strengths: 0.8', { exact: true })).toBeVisible();
  await closeSettings(page);
  await page.getByLabel('Image generation for this request', { exact: true }).uncheck();
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toHaveValue('portrait');
  await page.getByRole('button', { name: 'Close Models', exact: true }).click();
  await page.getByLabel('Image generation for this request', { exact: true }).check();
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toHaveValue('portrait');
  await openModels(page);
  await page.getByLabel('Image generation profile').selectOption('base');
  await expect(page.getByText('LoRAs: None', { exact: true })).toBeVisible();
});
