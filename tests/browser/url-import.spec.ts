import { openAssets } from './helpers';
import { test, expect } from '@playwright/test';

test('web access imports without a search service, preserves provenance on reload, and resets permission', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'URL import' } })
  ).json();
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'tool-model',
        model_health: 'not_probed',
        search_available: false,
        url_import_available: true,
        execution_available: true,
        asset_library_available: true,
        backends: [{ id: 'compatible', model: 'tool-model' }],
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  const submissions: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    const input = route.request().postDataJSON();
    submissions.push(input);
    const id = crypto.randomUUID();
    const asset = {
      id: crypto.randomUUID(),
      workspace_id: workspace.id,
      name: 'age.csv',
      media_type: 'text/csv',
      bytes: 7,
      origin: 'import',
      execution_id: null,
      created_at_ms: 1,
      source: {
        url: 'https://example.com/age.csv',
        final_url: 'https://cdn.example.com/age.csv',
        sha256: 'a'.repeat(64),
        run_id: id,
        call_id: 'import_1',
      },
    };
    const run = {
      id,
      workspace_id: workspace.id,
      provider: 'compatible',
      model: 'tool-model',
      status: 'succeeded',
      error: null,
      output: 'File saved.',
      mode: 'chat',
      workspace_tools: input.workspace_tools,
      base_revision_id: null,
      execution_result: null,
      execution_revision_id: null,
      node_scope: 'compute',
      input_assets: [],
      search_enabled: false,
      import_enabled: true,
      imports: [asset],
      searches: [],
      model_rounds: 2,
      queue_ms: 0,
      duration_ms: 8,
      stats: {
        reported_model: null,
        finish_reason: 'stop',
        input_tokens: null,
        output_tokens: null,
        total_tokens: null,
        first_token_ms: null,
      },
    };
    detail.assets = [asset];
    detail.runs = [run];
    detail.messages = [
      { role: 'user', content: input.prompt, run_id: id },
      { role: 'assistant', content: run.output, run_id: id },
    ];
    return route.fulfill({ status: 202, json: run });
  });
  await page.goto('/');
  const web = page.getByRole('checkbox', { name: /^Web access for this request/ });
  await expect(web).not.toBeChecked();
  await web.check();
  await page.getByRole('checkbox', { name: /^Analyze files and create outputs/ }).check();
  await page.getByLabel('Message', { exact: true }).fill('Import age.csv and analyze it.');
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.getByLabel('Tool activity')).toContainText('1 imports');
  expect(submissions[0]).toMatchObject({
    search: false,
    allow_url_import: true,
    node_scope: 'compute',
    input_asset_ids: [],
  });
  await expect(web).not.toBeChecked();
  await page.reload();
  await expect(page.getByLabel('Tool activity')).toContainText('1 imports');
  await openAssets(page);
  const library = page.getByLabel('Workspace assets', { exact: true });
  await library.getByText('Import source', { exact: true }).click();
  await expect(
    library.getByRole('link', { name: 'https://example.com/age.csv', exact: true }),
  ).toHaveAttribute('href', 'https://example.com/age.csv');
  await expect(library).toContainText('SHA-256: ' + 'a'.repeat(64));
  await expect(page.getByLabel('App revisions')).toContainText('Empty project');
  // A malformed refreshed provenance never replaces the last accepted state.
  detail.assets[0].source.final_url = 'http://127.0.0.1/private';
  await page.getByRole('button', { name: 'Close Files & assets', exact: true }).click();
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await openAssets(page);
  await expect(page.locator('.run-error[role=alert]')).toContainText('incompatible shape');
  await expect(
    library.getByRole('link', { name: 'https://cdn.example.com/age.csv', exact: true }),
  ).toBeVisible();
});

test('a downgraded import permission receipt preserves the pending request', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Import receipt' } })
  ).json();
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'tool-model',
        model_health: 'not_probed',
        url_import_available: true,
        backends: [{ id: 'compatible', model: 'tool-model' }],
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    expect(route.request().postDataJSON().allow_url_import).toBe(true);
    return route.fulfill({
      status: 202,
      json: {
        id: crypto.randomUUID(),
        workspace_id: workspace.id,
        provider: 'compatible',
        model: 'tool-model',
        status: 'queued',
        error: null,
        output: '',
        mode: 'chat',
        base_revision_id: null,
        execution_result: null,
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
        import_enabled: false,
      },
    });
  });
  await page.goto('/');
  const web = page.getByRole('checkbox', { name: /^Web access for this request/ });
  await web.check();
  await page.getByLabel('Message', { exact: true }).fill('Import this URL.');
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
  await expect(web).toBeChecked();
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Import this URL.');
});
