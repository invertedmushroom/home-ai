import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

async function fixture(page: Page, request: APIRequestContext) {
  const workspace: { id: string; title: string } = await (
    await request.post('/api/workspaces', { data: { title: 'Search permission' } })
  ).json();
  const detail = {
    workspace,
    messages: [] as { role: 'user' | 'assistant'; content: string; run_id: string }[],
    runs: [] as ReturnType<typeof run>[],
    revisions: [],
    proposals: [],
  };
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'tool-model',
        model_health: 'not_probed',
        search_available: true,
        backends: [{ id: 'compatible', model: 'tool-model' }],
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  return { workspace, detail };
}

function run(workspace: string, enabled: boolean) {
  return {
    id: crypto.randomUUID(),
    workspace_id: workspace,
    status: 'succeeded',
    error: null,
    provider: 'compatible',
    model: 'tool-model',
    output: '',
    mode: 'chat',
    base_revision_id: null,
    execution_result: null,
    queue_ms: 0,
    duration_ms: 12,
    stats: {
      reported_model: 'tool-model',
      finish_reason: 'stop',
      input_tokens: 20,
      output_tokens: 8,
      total_tokens: 28,
      first_token_ms: 2,
    },
    search_enabled: enabled,
    model_rounds: enabled ? 2 : 0,
    searches: enabled
      ? [
          {
            id: crypto.randomUUID(),
            call_id: 'call_1',
            query: 'Rust docs',
            engine: 'brave',
            status: 'succeeded',
            elapsed_ms: 7,
            error: null,
            sources: [
              {
                id: 'S1',
                title: '<img src=x> Rust docs',
                url: 'https://doc.rust-lang.org/book/',
                snippet: 'Untrusted snippet',
              },
            ],
          },
        ]
      : [],
  };
}

test('search is explicit for each request and only saved source receipts create links after reload', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await fixture(page, request);
  const submissions: { search: boolean; backend: string; mode: string; prompt: string }[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    const input = route.request().postDataJSON();
    submissions.push(input);
    const receipt = run(workspace.id, input.search);
    detail.runs.push(receipt);
    detail.messages.push(
      { role: 'user', content: input.prompt, run_id: receipt.id },
      {
        role: 'assistant',
        content: input.search
          ? 'Read the docs [S1]. Unsupported [S99] https://invented.test/'
          : 'Plain answer',
        run_id: receipt.id,
      },
    );
    return route.fulfill({ status: 202, json: receipt });
  });
  await page.goto('/');
  const permission = page.getByRole('checkbox', { name: /^Web access for this request/ });
  await expect(permission).not.toBeChecked();
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(false);
  await page.getByLabel('Message', { exact: true }).fill('No search');
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.getByText('Plain answer', { exact: true })).toBeVisible();
  expect(submissions[0].search).toBe(false);
  await expect(page.getByLabel('Tool activity').locator('details')).not.toHaveAttribute('open', '');
  await permission.check();
  await page.getByLabel('Message', { exact: true }).fill('Search for docs');
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.getByLabel('Search sources').getByRole('link')).toHaveCount(1);
  await expect(permission).not.toBeChecked();
  expect(submissions[1]).toMatchObject({ backend: 'compatible', search: true, mode: 'chat' });
  expect(submissions[1]).not.toHaveProperty('search_config');
  const source = page.getByLabel('Search sources').getByRole('link');
  await expect(source).toHaveAttribute('href', 'https://doc.rust-lang.org/book/');
  await expect(source).toHaveAttribute('rel', 'noopener noreferrer');
  await expect(page.getByLabel('Tool activity').locator('img')).toHaveCount(0);
  await expect(page.getByRole('link', { name: /S99/ })).toHaveCount(0);
  await page.reload();
  await expect(page.getByLabel('Search sources').getByRole('link')).toHaveCount(1);
  await expect(permission).not.toBeChecked();
});

test('failed submission preserves permission and an incompatible permission receipt cannot silently downgrade it', async ({
  page,
  request,
}) => {
  const { workspace } = await fixture(page, request);
  let attempts = 0;
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    attempts++;
    expect(route.request().postDataJSON().search).toBe(true);
    return attempts === 1
      ? route.fulfill({ status: 503, json: { error: 'Temporary admission failure' } })
      : route.fulfill({ status: 202, json: run(workspace.id, false) });
  });
  await page.goto('/');
  const permission = page.getByRole('checkbox', { name: /^Web access for this request/ });
  await permission.check();
  await page.getByLabel('Message', { exact: true }).fill('Keep this request');
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Temporary admission failure');
  await expect(permission).toBeChecked();
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
  await expect(permission).toBeChecked();
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Keep this request');
  expect(attempts).toBe(2);
  await page.getByRole('button', { name: 'New workspace' }).click();
  await expect(permission).not.toBeChecked();
});

test('server Unicode query rules remain readable in the browser across reload', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await fixture(page, request);
  const receipt = run(workspace.id, true);
  detail.runs.push(receipt);
  detail.messages.push({ role: 'assistant', run_id: receipt.id, content: 'Unicode query answer' });
  for (const query of ['\uFEFF', 'x\uFEFF!literal', 'Rust\u0085docs']) {
    receipt.searches[0].query = query;
    await page.goto('/');
    await expect(page.getByLabel('Conversation')).toContainText('Unicode query answer');
    await expect(page.getByLabel('Search sources').getByRole('link')).toHaveCount(1);
  }
  receipt.searches[0].query = 'x\u0085!engine';
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(page.locator('.run-error[role=alert]')).toContainText('incompatible shape');
  await expect(page.getByLabel('Conversation')).toContainText('Unicode query answer');
});

test('malformed search sources preserve the last valid conversation and never become active links', async ({
  page,
  request,
}) => {
  const { workspace, detail } = await fixture(page, request);
  const receipt = run(workspace.id, true);
  detail.runs.push(receipt);
  detail.messages.push({ role: 'assistant', run_id: receipt.id, content: 'Keep the valid answer' });
  await page.goto('/');
  const source = page.getByLabel('Search sources').getByRole('link');
  await expect(source).toHaveAttribute('href', 'https://doc.rust-lang.org/book/');
  receipt.searches[0].sources[0].url = 'javascript:alert(1)';
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(page.locator('.run-error[role=alert]')).toContainText('incompatible shape');
  await expect(source).toHaveAttribute('href', 'https://doc.rust-lang.org/book/');
  await expect(page.getByLabel('Conversation')).toContainText('Keep the valid answer');
});
