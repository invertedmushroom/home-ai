import { openTools, closeTools, openWorkspaces } from './helpers';
import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import type { Asset, Run, ToolExecution, WorkspaceDetail } from '../../apps/web/src/api';

async function setupStatusAndWorker(page: Page) {
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'tool-model',
        model_health: 'not_probed',
        search_available: true,
        execution_available: true,
        backends: [{ id: 'compatible', model: 'tool-model' }],
      },
    }),
  );
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Ready' } }),
  );
}

async function savedCsvProject(request: APIRequestContext, title: string) {
  const workspace = await (await request.post('/api/workspaces', { data: { title } })).json();
  const content = '\uFEFFamount\r\n4\r\n5\r\n6\r\n';
  const proposal = await (
    await request.post(`/api/workspaces/${workspace.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: null },
        changes: [{ op: 'write', path: 'data/sales.csv', content }],
      },
    })
  ).json();
  const revision = await (await request.post(`/api/proposals/${proposal.id}/apply`)).json();
  return { workspace, content, revision };
}

test('workspace attachments use an exact saved project copy and existing assets, survive a failed refresh/send, and keep their contents when the project changes', async ({
  page,
  request,
}) => {
  const { workspace, content, revision } = await savedCsvProject(request, 'Saved analysis inputs');
  const existing = await (
    await request.post(`/api/workspaces/${workspace.id}/assets`, {
      data: { name: 'settings.json', content: '{"currency":"EUR"}' },
    })
  ).json();
  await setupStatusAndWorker(page);
  await page.goto('/');
  await page.getByRole('checkbox', { name: /^Analyze files and create outputs/ }).check();
  await openTools(page);
  const picker = page.getByRole('group', { name: 'Attach from workspace', exact: true });
  const choose = picker.getByLabel('Workspace project file');
  const attach = picker.getByRole('button', { name: 'Attach project file' });
  await picker.getByRole('checkbox', { name: /settings\.json/ }).check();
  await choose.selectOption('data/sales.csv');

  // A saved attachment receipt remains usable when its subsequent refresh fails.
  let failRefresh = false;
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => {
    if (!failRefresh) return route.fallback();
    failRefresh = false;
    return route.fulfill({ status: 503, json: { error: 'Attachment refresh failed' } });
  });
  let copies = 0;
  await page.route(`**/api/workspaces/${workspace.id}/assets`, async (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    copies++;
    expect(route.request().postDataJSON()).toEqual({ name: 'data/sales.csv', content });
    const response = await route.fetch();
    failRefresh = true;
    return route.fulfill({ response });
  });
  await attach.click();
  await expect(page.locator('.error[role=alert]')).toContainText('Attachment refresh failed');
  await expect(picker.getByRole('checkbox', { name: /data\/sales\.csv/ })).toBeChecked();
  await expect(choose).toHaveValue('');
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const copy = detail.assets.find((asset: Asset) => asset.name === 'data/sales.csv');
  expect(copy).toBeTruthy();
  const stored = await (await request.get(`/api/assets/${copy.id}`)).json();
  expect(stored.content).toBe(content);
  expect(detail.revisions).toHaveLength(1);
  expect(detail.messages).toHaveLength(0);
  expect(await (await request.get(`/api/workspaces/${workspace.id}/executions`)).json()).toEqual(
    [],
  );

  // Changing the head invalidates an unconfirmed project choice without altering
  // an already attached copy or silently selecting the new revision's file.
  await choose.selectOption('data/sales.csv');
  const next = await (
    await request.post(`/api/workspaces/${workspace.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: revision.id },
        changes: [{ op: 'write', path: 'data/sales.csv', content: 'amount\n999\n' }],
      },
    })
  ).json();
  await request.post(`/api/proposals/${next.id}/apply`);
  await closeTools(page);
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await openTools(page);
  await expect(picker).toContainText('Revision 2');
  await expect(choose).toHaveValue('');
  await expect(attach).toBeDisabled();
  await expect(picker.getByRole('checkbox', { name: /data\/sales\.csv/ })).toBeChecked();
  const submissions: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    submissions.push(route.request().postDataJSON());
    return route.fulfill({ status: 503, json: { error: 'Model busy' } });
  });
  await closeTools(page);
  await page.getByLabel('Message', { exact: true }).fill('Total the attached saved CSV.');
  await closeTools(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Model busy');
  expect(submissions[0]).toMatchObject({
    node_scope: 'compute',
    input_asset_ids: [existing.id, copy.id],
  });
  await openTools(page);
  await expect(picker.getByRole('checkbox', { name: /settings\.json/ })).toBeChecked();
  await expect(picker.getByRole('checkbox', { name: /data\/sales\.csv/ })).toBeChecked();
  expect(copies).toBe(1);
  await page.reload();
  await page.getByRole('checkbox', { name: /^Analyze files and create outputs/ }).check();
  await openTools(page);
  await expect(picker.getByRole('checkbox', { name: /data\/sales\.csv/ })).not.toBeChecked();
});

test('project attachment failure retains the chosen file and workspace changes discard a late attachment result', async ({
  page,
  request,
}) => {
  const { workspace } = await savedCsvProject(request, 'Attachment owner');
  const other = await (
    await request.post('/api/workspaces', { data: { title: 'Other attachment workspace' } })
  ).json();
  await setupStatusAndWorker(page);
  let release!: () => void;
  let started!: () => void;
  let settled!: () => void;
  const pending = new Promise<void>((resolve) => {
    release = resolve;
  });
  const requested = new Promise<void>((resolve) => {
    started = resolve;
  });
  const completed = new Promise<void>((resolve) => {
    settled = resolve;
  });
  let attempts = 0;
  await page.route(`**/api/workspaces/${workspace.id}/assets`, async (route) => {
    attempts++;
    if (attempts === 1)
      return route.fulfill({ status: 503, json: { error: 'Attachment unavailable' } });
    started();
    await pending;
    try {
      await route.fulfill({ status: 503, json: { error: 'Old workspace error' } });
    } finally {
      settled();
    }
  });
  await page.goto('/');
  await openWorkspaces(page);
  await page.getByRole('button', { name: workspace.title, exact: false }).click();
  await page.getByRole('checkbox', { name: /^Analyze files and create outputs/ }).check();
  await openTools(page);
  const choose = page.getByLabel('Workspace project file');
  await choose.selectOption('data/sales.csv');
  await page.getByRole('button', { name: 'Attach project file' }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Attachment unavailable');
  await expect(choose).toHaveValue('data/sales.csv');
  await page.getByRole('button', { name: 'Dismiss error' }).click();
  await page.getByRole('button', { name: 'Attach project file' }).click();
  await requested;
  await openWorkspaces(page);
  await page.getByRole('button', { name: other.title, exact: false }).click();
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    other.title,
  );
  release();
  await completed;
  await page.getByRole('checkbox', { name: /^Analyze files and create outputs/ }).check();
  await openTools(page);
  await expect(page.getByLabel('Attach text files')).toBeEnabled();
  await expect(page.getByRole('checkbox', { name: /data\/sales\.csv/ })).toHaveCount(0);
  await expect(page.locator('.error[role=alert]')).toHaveCount(0);
  expect((await (await request.get(`/api/workspaces/${other.id}`)).json()).assets).toEqual([]);
});

for (const uploadInput of [false, true]) {
  test(`implicit first workspace creation preserves tool permission after ${uploadInput ? 'uploading a computation file' : 'a failed first submission'}`, async ({
    page,
  }) => {
    await setupStatusAndWorker(page);
    await page.route('**/api/workspaces', (route) =>
      route.request().method() === 'GET' ? route.fulfill({ json: [] }) : route.fallback(),
    );
    const submissions: Record<string, unknown>[] = [];
    await page.route('**/api/workspaces/*/runs', (route) => {
      submissions.push(route.request().postDataJSON());
      return route.fulfill({ status: 409, json: { error: 'Worker busy' } });
    });

    await page.goto('/');
    await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(false);
    const computeToggle = page.getByRole('checkbox', {
      name: /^Analyze files and create outputs/,
    });
    const searchToggle = page.getByRole('checkbox', {
      name: /^Web access for this request/,
    });
    await searchToggle.check();
    await computeToggle.check();
    if (uploadInput) {
      await openTools(page);
      await page.getByLabel('Attach text files').setInputFiles({
        name: 'items.csv',
        mimeType: 'text/csv',
        buffer: Buffer.from('item,qty\nApples,14\nPears,26\n'),
      });
      await expect(page.getByRole('checkbox', { name: /items\.csv/ })).toBeChecked();
      await expect(computeToggle).toBeChecked();
      await expect(searchToggle).toBeChecked();
    }

    await closeTools(page);
    await page.getByLabel('Message', { exact: true }).fill('Compute the total quantity.');
    await closeTools(page);
    await page.getByRole('button', { name: /^Send/ }).click();
    await expect(page.locator('.error[role=alert]')).toContainText('Worker busy');
    expect(submissions).toHaveLength(1);
    expect(submissions[0]).toMatchObject({
      mode: 'chat',
      node_scope: 'compute',
      search: true,
    });
    expect(submissions[0].input_asset_ids).toHaveLength(uploadInput ? 1 : 0);
    await expect(computeToggle).toBeChecked();
    await expect(searchToggle).toBeChecked();
    if (uploadInput) {
      await openTools(page);
      await expect(page.getByRole('checkbox', { name: /items\.csv/ })).toBeChecked();
    }

    await page.getByRole('button', { name: 'New workspace' }).click();
    await expect(computeToggle).toBeChecked();
    await expect(searchToggle).not.toBeChecked();
  });
}

test('empty-project CSV-to-SVG temporary computation recovers from failed first script, isolates SVG as an image, downloads outputs, and reviews selected assets into project', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'CSV computation' } })
  ).json();

  // Pre-create output assets in the real SQLite store so GET /api/assets/{id} and POST /api/workspaces/{id}/asset-proposals use real backend endpoints
  const svgSource =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 40"><script>window.__svgScriptExecuted = true;</script><rect width="100" height="24" fill="#25483a"/><text x="8" y="16" fill="#ffffff">Total: 50</text></svg>';
  const svgAsset: Asset = await (
    await request.post(`/api/workspaces/${workspace.id}/assets`, {
      data: { name: 'chart.svg', content: svgSource },
    })
  ).json();
  const summaryAsset: Asset = await (
    await request.post(`/api/workspaces/${workspace.id}/assets`, {
      data: { name: 'summary.json', content: '{"total":50}\n' },
    })
  ).json();

  await setupStatusAndWorker(page);

  let currentDetail: WorkspaceDetail = await (
    await request.get(`/api/workspaces/${workspace.id}`)
  ).json();
  // Hide pre-created output assets until the computation run completes
  let runCompleted = false;

  await page.route(`**/api/workspaces/${workspace.id}`, async (route) => {
    const fresh: WorkspaceDetail = await (
      await request.get(`/api/workspaces/${workspace.id}`)
    ).json();
    currentDetail = {
      ...fresh,
      assets: runCompleted
        ? fresh.assets
        : fresh.assets.filter((a) => a.id !== svgAsset.id && a.id !== summaryAsset.id),
      runs: currentDetail.runs,
      messages: currentDetail.messages,
    };
    return route.fulfill({ json: currentDetail });
  });

  const submissions: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, async (route) => {
    const input = route.request().postDataJSON();
    submissions.push(input);
    const fresh: WorkspaceDetail = await (
      await request.get(`/api/workspaces/${workspace.id}`)
    ).json();
    const uploadedCsv = fresh.assets.find((a) => a.name === 'sales.csv')!;
    const call1Id = crypto.randomUUID();
    const call2Id = crypto.randomUUID();
    const execAssetSvg: Asset = {
      ...svgAsset,
      origin: 'execution',
      execution_id: call2Id,
    };
    const execAssetSummary: Asset = {
      ...summaryAsset,
      origin: 'execution',
      execution_id: call2Id,
    };
    const inputRef = {
      id: uploadedCsv.id,
      name: uploadedCsv.name,
      media_type: uploadedCsv.media_type,
      bytes: uploadedCsv.bytes,
      path: 'inputs/1-sales.csv',
    };
    const toolExecutions: ToolExecution[] = [
      {
        id: call1Id,
        call_id: 'call_compute_1',
        entry: '__compute.cjs',
        action: 'run',
        code: 'console.log(nonExistentVariable);',
        input_assets: [inputRef],
        result: {
          id: call1Id,
          workspace_id: workspace.id,
          revision_id: null,
          entry: '__compute.cjs',
          action: 'run',
          status: 'failed',
          stdout: '',
          stderr: 'ReferenceError: nonExistentVariable is not defined',
          exit_code: 1,
          signal: null,
          truncated: false,
          error: 'Script exited with status 1.',
          logs_shortened: false,
          output_assets: [],
        },
      },
      {
        id: call2Id,
        call_id: 'call_compute_2',
        entry: '__compute.cjs',
        action: 'run',
        code: 'const fs = require("fs");\nfs.writeFileSync("output/chart.svg", "<svg/>");\nconsole.log("total=50");',
        input_assets: [inputRef],
        result: {
          id: call2Id,
          workspace_id: workspace.id,
          revision_id: null,
          entry: '__compute.cjs',
          action: 'run',
          status: 'succeeded',
          stdout: 'total=50\n',
          stderr: '',
          exit_code: 0,
          signal: null,
          truncated: false,
          error: null,
          logs_shortened: false,
          output_assets: [execAssetSvg, execAssetSummary],
        },
      },
    ];
    const run: Run = {
      id: crypto.randomUUID(),
      workspace_id: workspace.id,
      status: 'succeeded',
      error: null,
      provider: 'compatible',
      model: 'tool-model',
      mode: 'chat',
      base_revision_id: null,
      execution_result: null,
      execution_revision_id: null,
      node_scope: 'compute',
      input_assets: [inputRef],
      tool_execution: toolExecutions[1],
      tool_executions: toolExecutions,
      output: 'The CSV total is 50 and chart.svg is ready.',
      queue_ms: 0,
      duration_ms: 25,
      stats: {
        reported_model: 'tool-model',
        finish_reason: 'stop',
        input_tokens: 45,
        output_tokens: 18,
        total_tokens: 63,
        first_token_ms: 4,
      },
      search_enabled: false,
      searches: [],
      model_rounds: 3,
    };
    runCompleted = true;
    currentDetail.runs = [run];
    currentDetail.messages = [
      { role: 'user', content: input.prompt, run_id: run.id },
      { role: 'assistant', content: run.output, run_id: run.id },
    ];
    return route.fulfill({ status: 202, json: run });
  });

  await page.goto('/');
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(false);

  const computeToggle = page.getByRole('checkbox', {
    name: /^Analyze files and create outputs/,
  });
  await expect(computeToggle).toBeChecked();
  await computeToggle.check();

  await openTools(page);
  await expect(
    page.getByText(
      /The selected model receives filenames and execution logs; logs can include file data/,
    ),
  ).toBeVisible();

  await openTools(page);
  await page.getByLabel('Attach text files').setInputFiles({
    name: 'sales.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from('month,amount\nJan,10\nFeb,25\nMar,15\n', 'utf-8'),
  });

  const csvCheckbox = page.getByRole('checkbox', { name: /sales\.csv/ });
  await expect(csvCheckbox).toBeVisible();
  await expect(csvCheckbox).toBeChecked();

  await closeTools(page);
  await page
    .getByLabel('Message', { exact: true })
    .fill('Compute the CSV total and generate an SVG chart.');
  await closeTools(page);
  await page.getByRole('button', { name: /^Send/ }).click();

  await expect(computeToggle).toBeChecked();
  expect(submissions).toHaveLength(1);
  expect(submissions[0]).toMatchObject({
    mode: 'chat',
    backend: 'compatible',
    node_scope: 'compute',
  });
  expect((submissions[0].input_asset_ids as string[]).length).toBe(1);

  // Project remains empty after temporary computation
  await expect(page.getByLabel('App revisions')).toContainText('0 FILES');
  await expect(page.getByLabel('App revisions')).toContainText('Empty project');

  // The collapsed run summary reports actual recorded model turns and computations.
  await expect(page.getByLabel('Tool activity')).toContainText(
    'Run details · 3 model turns · 2 computations',
  );

  // Conversation result card displays status, which call ran, stdout, and SVG image preview
  const card = page.getByLabel('Temporary computation result');
  await expect(card).toBeVisible();
  await expect(card).toContainText('Analysis and outputs · succeeded');
  await expect(card).toContainText('Call 2 of 2 ran after call 1 (failed)');
  await expect(card.getByLabel('Computation stdout')).toHaveText('total=50\n');

  // Verify chronological ordering within the turn:
  // User prompt -> Assistant response -> Run details -> Temporary computation result
  const userMessageBox = await page
    .locator('.message.user')
    .filter({ hasText: 'Compute the CSV total and generate an SVG chart.' })
    .boundingBox();
  const toolActivityBox = await page.getByLabel('Tool activity').boundingBox();
  const computeCardBox = await card.boundingBox();
  const assistantReplyBox = await page
    .locator('.message.assistant > pre')
    .filter({ hasText: 'The CSV total is 50 and chart.svg is ready.' })
    .boundingBox();
  expect(userMessageBox).not.toBeNull();
  expect(toolActivityBox).not.toBeNull();
  expect(computeCardBox).not.toBeNull();
  expect(assistantReplyBox).not.toBeNull();
  expect(userMessageBox!.y).toBeLessThan(assistantReplyBox!.y);
  expect(assistantReplyBox!.y).toBeLessThan(toolActivityBox!.y);
  expect(toolActivityBox!.y).toBeLessThan(computeCardBox!.y);

  const svgImg = card.getByRole('img', { name: 'Preview of chart.svg' });
  await expect(svgImg).toBeVisible();
  await expect(svgImg).toHaveAttribute('src', /^data:image\/svg\+xml;utf8,/);
  const scriptExecuted = await page.evaluate(
    () => (window as unknown as { __svgScriptExecuted?: boolean }).__svgScriptExecuted,
  );
  expect(scriptExecuted).toBeUndefined();

  // Disclosure shows both calls and their submitted code
  await card.getByText('Submitted code and execution details').click();
  await expect(card.getByLabel('Submitted code 1')).toContainText(
    'console.log(nonExistentVariable);',
  );
  await expect(card.getByLabel('Submitted code 2')).toContainText(
    'fs.writeFileSync("output/chart.svg"',
  );

  // Download SVG output asset
  const downloadPromise = page.waitForEvent('download');
  await card.getByRole('button', { name: 'Download chart.svg' }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe('chart.svg');

  // Select only chart.svg to review in project
  await card.getByRole('checkbox', { name: /summary\.json/ }).uncheck();
  await card.getByRole('button', { name: 'Review selected files in project' }).click();

  const revisionsPanel = page.getByLabel('App revisions');
  await expect(revisionsPanel).toContainText('Review proposed changes');
  await expect(revisionsPanel).toContainText('Source: workspace assets · 1 file');

  await revisionsPanel.getByRole('button', { name: 'Apply changes' }).click();
  await expect(revisionsPanel).toContainText('Revision 1');
  await expect(revisionsPanel).toContainText('1 FILES');
});

test('failed submissions and malformed receipts preserve input selection while workspace switches and scope toggles reset cleanly', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Compute resilience' } })
  ).json();
  const proposal = await (
    await request.post(`/api/workspaces/${workspace.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: null },
        changes: [{ op: 'write', path: 'main.cjs', content: 'console.log(1);' }],
      },
    })
  ).json();
  await request.post(`/api/proposals/${proposal.id}/apply`);

  await setupStatusAndWorker(page);

  let attempts = 0;
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    attempts++;
    const body = route.request().postDataJSON();
    expect(body.node_scope).toBe('compute');
    expect(body.input_asset_ids).toHaveLength(1);
    if (attempts === 1) {
      return route.fulfill({ status: 503, json: { error: 'Worker busy' } });
    }
    // Second attempt returns a downgraded receipt with node_scope: 'off'
    return route.fulfill({
      status: 202,
      json: {
        id: crypto.randomUUID(),
        workspace_id: workspace.id,
        status: 'succeeded',
        error: null,
        provider: 'compatible',
        model: 'tool-model',
        mode: 'chat',
        base_revision_id: null,
        execution_result: null,
        execution_revision_id: null,
        node_scope: 'off',
        input_assets: [],
        tool_execution: null,
        tool_executions: [],
        output: 'Downgraded',
        queue_ms: 0,
        duration_ms: 5,
        stats: {
          reported_model: 'tool-model',
          finish_reason: 'stop',
          input_tokens: 10,
          output_tokens: 2,
          total_tokens: 12,
          first_token_ms: 1,
        },
        search_enabled: false,
        searches: [],
        model_rounds: 0,
      },
    });
  });

  await page.goto('/');
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(false);

  await expect(page.getByRole('checkbox', { name: /^Allow one saved-script action/ })).toHaveCount(
    0,
  );
  const computeToggle = page.getByRole('checkbox', {
    name: /^Analyze files and create outputs/,
  });

  await computeToggle.check();
  await expect(computeToggle).toBeChecked();
  await expect(page.getByRole('checkbox', { name: /^Change project files/ })).not.toBeChecked();

  await openTools(page);
  await page.getByLabel('Attach text files').setInputFiles([
    { name: 'first.csv', mimeType: 'text/csv', buffer: Buffer.from('a,b\n1,2\n') },
    { name: 'second.csv', mimeType: 'text/csv', buffer: Buffer.from('x,y\n3,4\n') },
  ]);

  const firstBox = page.getByRole('checkbox', { name: /first\.csv/ });
  const secondBox = page.getByRole('checkbox', { name: /second\.csv/ });
  await expect(firstBox).toBeChecked();
  await expect(secondBox).toBeChecked();
  await secondBox.uncheck();

  await closeTools(page);
  await page.getByLabel('Message', { exact: true }).fill('Analyze first.csv');
  await closeTools(page);
  await page.getByRole('button', { name: /^Send/ }).click();

  await expect(page.locator('.error[role=alert]')).toContainText('Worker busy');
  await expect(computeToggle).toBeChecked();
  await openTools(page);
  await expect(firstBox).toBeChecked();
  await expect(secondBox).not.toBeChecked();

  await closeTools(page);
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
  await expect(computeToggle).toBeChecked();
  await openTools(page);
  await expect(firstBox).toBeChecked();
  await expect(secondBox).not.toBeChecked();

  // A new workspace keeps local computation available and clears selected inputs.
  await page.getByRole('button', { name: 'New workspace' }).click();
  await expect(computeToggle).toBeChecked();
});

test('reviewing selected computation assets captures the current project base and requires a fresh review if head advances', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Asset base conflict' } })
  ).json();
  const asset: Asset = await (
    await request.post(`/api/workspaces/${workspace.id}/assets`, {
      data: { name: 'report.svg', content: '<svg xmlns="http://www.w3.org/2000/svg"/>' },
    })
  ).json();

  await setupStatusAndWorker(page);

  const callId = crypto.randomUUID();
  const execAsset: Asset = { ...asset, origin: 'execution', execution_id: callId };
  const toolExec: ToolExecution = {
    id: callId,
    call_id: 'call_1',
    entry: '__compute.cjs',
    action: 'run',
    code: 'console.log("ok");',
    input_assets: [],
    result: {
      id: callId,
      workspace_id: workspace.id,
      revision_id: null,
      entry: '__compute.cjs',
      action: 'run',
      status: 'succeeded',
      stdout: 'ok\n',
      stderr: '',
      exit_code: 0,
      signal: null,
      truncated: false,
      error: null,
      logs_shortened: false,
      output_assets: [execAsset],
    },
  };
  const run: Run = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    status: 'succeeded',
    error: null,
    provider: 'compatible',
    model: 'tool-model',
    mode: 'chat',
    base_revision_id: null,
    execution_result: null,
    execution_revision_id: null,
    node_scope: 'compute',
    input_assets: [],
    tool_execution: toolExec,
    tool_executions: [toolExec],
    output: 'Generated report.svg',
    queue_ms: 0,
    duration_ms: 10,
    stats: {
      reported_model: 'tool-model',
      finish_reason: 'stop',
      input_tokens: 12,
      output_tokens: 4,
      total_tokens: 16,
      first_token_ms: 2,
    },
    search_enabled: false,
    searches: [],
    model_rounds: 2,
  };

  await page.route(`**/api/workspaces/${workspace.id}`, async (route) => {
    const fresh: WorkspaceDetail = await (
      await request.get(`/api/workspaces/${workspace.id}`)
    ).json();
    return route.fulfill({
      json: {
        ...fresh,
        runs: [run],
        messages: [
          { role: 'user', content: 'Generate report.svg', run_id: run.id },
          { role: 'assistant', content: 'Generated report.svg', run_id: run.id },
        ],
      },
    });
  });

  await page.goto('/');
  const card = page.getByLabel('Temporary computation result');
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: 'Review selected files in project' }).click();

  const revisionsPanel = page.getByLabel('App revisions');
  await expect(revisionsPanel).toContainText('Review proposed changes');
  await expect(revisionsPanel.getByRole('button', { name: 'Apply changes' })).toBeEnabled();

  // Advance project head concurrently
  const concurrentProposal = await (
    await request.post(`/api/workspaces/${workspace.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: null },
        changes: [{ op: 'write', path: 'README.md', content: '# Head v1\n' }],
      },
    })
  ).json();
  await request.post(`/api/proposals/${concurrentProposal.id}/apply`);

  await revisionsPanel.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(revisionsPanel).toContainText(
    'The base revision is stale. Generate or run new changes against the latest revision.',
  );
  await expect(revisionsPanel.getByRole('button', { name: 'Apply changes' })).toBeDisabled();

  // A fresh review from the computation card captures the new head (Revision 1) and applies cleanly
  await card.getByRole('button', { name: 'Review selected files in project' }).click();
  await expect(revisionsPanel).toContainText('Base: Revision 1 · 1 changed file');
  await expect(revisionsPanel.getByRole('button', { name: 'Apply changes' })).toBeEnabled();
  await revisionsPanel.getByRole('button', { name: 'Apply changes' }).click();
  await expect(revisionsPanel).toContainText('Revision 2');
  await expect(revisionsPanel).toContainText('2 FILES');
});
