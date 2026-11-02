import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

async function fixture(page: Page, request: APIRequestContext, phase: string | null = 'draining') {
  const workspace = await (
    await request.post('/api/workspaces', {
      data: { title: `Runtime lifecycle ${crypto.randomUUID()}` },
    })
  ).json();
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const run = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    status: 'cancelled',
    error: null,
    provider: 'lmstudio',
    model: 'fixture',
    mode: 'chat',
    output: '',
    base_revision_id: null,
    queue_ms: 0,
    duration_ms: 10,
    stats: {
      reported_model: null,
      finish_reason: null,
      input_tokens: null,
      output_tokens: null,
      total_tokens: null,
      first_token_ms: null,
    },
    local_model: phase ? { phase, round: 1, operation_id: crypto.randomUUID() } : undefined,
  };
  detail.runs = [run];
  detail.messages = [{ role: 'user', content: 'Stopped lifecycle request', run_id: run.id }];
  let reads = 0;
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'lmstudio',
        model: 'fixture',
        model_health: 'not_probed',
        backends: [{ id: 'lmstudio', model: 'fixture' }],
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => {
    reads++;
    return route.fulfill({ json: detail });
  });
  return { workspace, detail, run, reads: () => reads };
}

test('terminal runs keep polling cleanup, reveal settlement and then stop polling', async ({
  page,
  request,
}) => {
  const state = await fixture(page, request);
  await page.goto('/');
  const activity = page.getByRole('status', { name: 'Model runtime status' });
  await expect(activity).toContainText('cleanup pending');
  await expect(page.getByText('cancelled', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete turn', exact: true })).toHaveCount(0);
  const before = state.reads();
  await expect.poll(state.reads).toBeGreaterThan(before);
  state.run.local_model!.phase = 'settled';
  await expect(activity).toContainText('this request is settled');
  await expect(activity).toContainText('weights may remain loaded');
  await expect(page.getByRole('button', { name: 'Delete turn', exact: true })).toBeVisible();
  const settled = state.reads();
  await page.waitForTimeout(1100);
  expect(state.reads()).toBe(settled);
});

test('blocked ownership differs from a live cleanup and does not poll forever or disable another request', async ({
  page,
  request,
}) => {
  const state = await fixture(page, request, 'blocked');
  await page.goto('/');
  const activity = page.getByRole('status', { name: 'Model runtime status' });
  await expect(activity).toContainText('completion unconfirmed');
  await expect(activity).toContainText('operator verifies');
  await expect(page.getByRole('button', { name: 'Delete turn', exact: true })).toHaveCount(0);
  await page.getByLabel('Message', { exact: true }).fill('Another explicitly selected request');
  await expect(page.getByRole('button', { name: /^Send/ })).toBeEnabled();
  const reads = state.reads();
  await page.waitForTimeout(1100);
  expect(state.reads()).toBe(reads);
});

test('older servers without activity do not claim that upstream cleanup is complete', async ({
  page,
  request,
}) => {
  const state = await fixture(page, request, null);
  await page.goto('/');
  await expect(page.getByText('Stopped lifecycle request', { exact: true })).toBeVisible();
  await expect(page.getByRole('status', { name: 'Model runtime status' })).toHaveCount(0);
  const reads = state.reads();
  await page.waitForTimeout(700);
  expect(state.reads()).toBe(reads);
});

test('invalid runtime identity preserves the last valid cleanup state and reports the error', async ({
  page,
  request,
}) => {
  const state = await fixture(page, request);
  await page.goto('/');
  const activity = page.getByRole('status', { name: 'Model runtime status' });
  await expect(activity).toContainText('cleanup pending');
  state.run.local_model!.phase = 'waiting';
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'incompatible shape',
  );
  await expect(activity).toContainText('cleanup pending');
  await expect(activity).not.toContainText('waiting for its turn');
});

test('workspace switches dispose cleanup polling', async ({ page, request }) => {
  const state = await fixture(page, request);
  const other = await (
    await request.post('/api/workspaces', {
      data: { title: 'Other runtime workspace' },
    })
  ).json();
  // The chooser initially selects the newest workspace.
  await page.route('**/api/workspaces', async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({ json: [state.workspace, other] });
  });
  await page.goto('/');
  await expect(page.getByRole('status', { name: 'Model runtime status' })).toContainText(
    'cleanup pending',
  );
  await page.getByRole('button', { name: 'Choose workspace', exact: true }).click();
  await page.getByRole('button', { name: 'Other runtime workspace', exact: false }).click();
  await expect(page.getByText('Stopped lifecycle request', { exact: true })).toHaveCount(0);
  const reads = state.reads();
  await page.waitForTimeout(1100);
  expect(state.reads()).toBe(reads);
});
