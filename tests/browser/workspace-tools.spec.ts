import { test, expect } from '@playwright/test';

test('workspace changes save without Apply, capabilities compose, and a downgraded receipt preserves the request', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Native files' } })
  ).json();
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const profile = {
    id: 'local-image',
    label: 'Local image',
    model: 'fixture',
    fingerprint: 'a'.repeat(64),
  };
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'fixture',
        model_health: 'not_probed',
        search_available: true,
        execution_available: true,
        image_profiles: [profile],
        backends: [{ id: 'compatible', model: 'fixture' }],
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  const submissions: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    const input = route.request().postDataJSON();
    submissions.push(input);
    const run = {
      id: crypto.randomUUID(),
      workspace_id: workspace.id,
      status: 'succeeded',
      error: null,
      provider: 'compatible',
      model: 'fixture',
      mode: 'chat',
      workspace_tools: submissions.length > 1,
      base_revision_id: null,
      execution_result: null,
      execution_revision_id: null,
      node_scope: 'compute',
      input_assets: [],
      tool_execution: null,
      tool_executions: [],
      search_enabled: input.search,
      import_enabled: false,
      imports: [],
      searches: [],
      image_profile: profile,
      image_input: null,
      image_job: null,
      model_rounds: 2,
      output: 'Saved report.md directly.',
      queue_ms: 0,
      duration_ms: 10,
      stats: {
        reported_model: null,
        finish_reason: 'stop',
        input_tokens: null,
        output_tokens: null,
        total_tokens: null,
        first_token_ms: null,
      },
    };
    if (submissions.length === 1) return route.fulfill({ status: 202, json: run });
    const executionId = crypto.randomUUID();
    const output = {
      id: crypto.randomUUID(),
      workspace_id: workspace.id,
      name: 'report.md',
      media_type: 'text/markdown',
      bytes: 15,
      origin: 'execution',
      execution_id: executionId,
      created_at_ms: 1,
      source: null,
    };
    const execution = {
      id: executionId,
      call_id: 'compute',
      entry: '__compute.cjs',
      action: 'run',
      code: "require('fs').writeFileSync('/output/report.md', '# Saved report\\n');",
      input_assets: [],
      result: {
        id: executionId,
        workspace_id: workspace.id,
        revision_id: null,
        entry: '__compute.cjs',
        action: 'run',
        status: 'succeeded',
        stdout: '',
        stderr: '',
        exit_code: 0,
        signal: null,
        truncated: false,
        error: null,
        logs_shortened: false,
        output_assets: [output],
      },
    };
    const completed = { ...run, tool_execution: execution, tool_executions: [execution] };
    const revision = {
      id: crypto.randomUUID(),
      run_id: run.id,
      execution_id: null,
      edit_id: null,
      asset_ids: null,
      number: 1,
      html: '',
      files: [{ path: 'report.md', content: '# Saved report\n' }],
      asset_bindings: [],
    };
    const proposal = {
      id: crypto.randomUUID(),
      workspace_id: workspace.id,
      run_id: run.id,
      execution_id: null,
      edit_id: null,
      asset_ids: null,
      base_revision_id: null,
      status: 'applied',
      applied_revision_id: revision.id,
      rejection_reason: null,
      changes: [{ op: 'write', path: 'report.md', content: '# Saved report\n' }],
      asset_changes: [],
    };
    detail.runs = [completed];
    detail.assets = [output];
    detail.revisions = [revision];
    detail.proposals = [proposal];
    detail.messages = [
      { role: 'user', run_id: run.id, content: input.prompt },
      { role: 'assistant', run_id: run.id, content: run.output },
    ];
    return route.fulfill({ status: 202, json: completed });
  });
  await page.goto('/');
  const files = page.getByRole('checkbox', { name: /^Change project files/ });
  const compute = page.getByRole('checkbox', { name: /^Analyze files/ });
  const image = page.getByRole('checkbox', { name: /^Image generation/ });
  const web = page.getByRole('checkbox', { name: /^Web access/ });
  await expect(files).toBeChecked();
  await expect(compute).toBeChecked();
  await expect(image).toBeChecked();
  await web.check();
  await expect(files).toBeChecked();
  await expect(compute).toBeChecked();
  await expect(image).toBeChecked();
  await page.getByLabel('Message', { exact: true }).fill('Save a report.');
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Save a report.');
  await expect(web).toBeChecked();
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.getByText('Saved report.md directly.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Review selected files in project' })).toHaveCount(
    0,
  );
  await expect(page.getByText('Outputs saved automatically with this reply.')).toBeVisible();
  await expect(page.getByLabel('App revisions')).toContainText('report.md');
  await expect(page.getByLabel('App revisions')).toContainText('Current files');
  expect(submissions[1]).toMatchObject({
    mode: 'chat',
    workspace_tools: true,
    node_scope: 'compute',
    image_profile_id: profile.id,
    search: true,
  });
  await expect(compute).toBeChecked();
  await expect(image).toBeChecked();
  await expect(web).not.toBeChecked();
  await page.reload();
  await expect(page.getByText('Saved report.md directly.', { exact: true })).toBeVisible();
  await expect(page.getByLabel('App revisions')).toContainText('report.md');
});
