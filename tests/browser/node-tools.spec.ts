import { closeRuns, openExecution, openRuns, openWorkspaces, selectModel } from './helpers';
import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import type { Run, WorkspaceDetail, Execution, ExecutionAttachment } from '../../apps/web/src/api';

async function fixture(page: Page, request: APIRequestContext) {
  const workspace = await (
    await request.post('/api/workspaces', {
      data: { title: `Script workflows ${crypto.randomUUID().slice(0, 8)}` },
    })
  ).json();
  const proposal = await (
    await request.post(`/api/workspaces/${workspace.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: null },
        changes: [{ op: 'write', path: 'main.cjs', content: 'console.log(42);' }],
      },
    })
  ).json();
  await request.post(`/api/proposals/${proposal.id}/apply`);
  const detail: WorkspaceDetail = await (
    await request.get(`/api/workspaces/${workspace.id}`)
  ).json();
  const jobs: Execution[] = [];
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'compatible',
        model: 'tool-model',
        model_health: 'not_probed',
        search_available: false,
        execution_available: true,
        backends: [
          { id: 'compatible', model: 'tool-model' },
          { id: 'groq', model: 'other-model' },
        ],
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Ready' } }),
  );
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) =>
    route.fulfill({ json: jobs }),
  );
  return { workspace, detail, jobs, revision: detail.revisions.at(-1)! };
}

function receipt(workspace: string, revision: string | null): Run {
  const id = crypto.randomUUID();
  const result: ExecutionAttachment = {
    id: crypto.randomUUID(),
    workspace_id: workspace,
    revision_id: revision ?? crypto.randomUUID(),
    entry: 'main.cjs',
    action: 'run',
    status: 'succeeded',
    stdout: '42',
    stderr: '',
    exit_code: 0,
    signal: null,
    truncated: false,
    error: null,
    logs_shortened: false,
  };
  return {
    id,
    workspace_id: workspace,
    status: 'succeeded',
    error: null,
    provider: 'compatible',
    model: 'tool-model',
    mode: 'chat',
    base_revision_id: null,
    execution_result: null,
    execution_revision_id: revision,
    output: '',
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
    search_enabled: false,
    searches: [],
    model_rounds: revision ? 2 : 0,
    tool_execution: revision
      ? { id: result.id, call_id: 'native_call', entry: result.entry, action: 'run', result }
      : null,
  };
}

function job(run: Run): Execution {
  const result = run.tool_execution!.result!;
  return {
    ...result,
    image_id: `sha256:${'a'.repeat(64)}`,
    outputs: [],
    created_at_ms: Date.now(),
    finished_at_ms: Date.now(),
  };
}

function publishExplanation(detail: WorkspaceDetail, result: Execution, backend: string): Run {
  const run = receipt(detail.workspace.id, null);
  run.provider = backend;
  run.output = 'The saved script printed 42.';
  run.execution_result = { ...result, logs_shortened: false };
  detail.runs.push(run);
  detail.messages.push({ role: 'assistant', run_id: run.id, content: run.output });
  return run;
}

test('Run and explain executes the selected saved script once, freezes the backend and preserves the unsent composer', async ({
  page,
  request,
}) => {
  const { workspace, detail, jobs, revision } = await fixture(page, request);
  const executions: Record<string, unknown>[] = [];
  const explanations: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) => {
    if (route.request().method() !== 'POST') return route.fulfill({ json: jobs });
    const input = route.request().postDataJSON();
    executions.push(input);
    jobs.push({
      ...job(receipt(workspace.id, revision.id)),
      id: input.id,
      status: 'running',
      exit_code: null,
      finished_at_ms: null,
    });
    return route.fulfill({ status: 202, json: jobs[0] });
  });
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    const input = route.request().postDataJSON();
    explanations.push(input);
    expect(input.execution_result_id).toBe(jobs[0].id);
    return route.fulfill({ status: 202, json: publishExplanation(detail, jobs[0], input.backend) });
  });
  await page.goto('/');
  await page.getByRole('checkbox', { name: 'Analyze files and create outputs' }).check();
  await closeRuns(page);
  await page.getByLabel('Message', { exact: true }).fill('Keep this unsent request.');
  await openExecution(page);
  await page.getByRole('button', { name: 'Run and explain', exact: true }).click();
  await expect(page.locator('.execution-status')).toHaveText('running');
  expect(explanations).toHaveLength(0);
  await closeRuns(page);
  await selectModel(page, 'groq');
  Object.assign(jobs[0], { status: 'succeeded', exit_code: 0, finished_at_ms: Date.now() });
  await expect(page.getByLabel('Conversation')).toContainText('The saved script printed 42.');
  expect(executions).toHaveLength(1);
  expect(executions[0]).toMatchObject({
    revision_id: revision.id,
    entry: 'main.cjs',
    action: 'run',
  });
  expect(explanations).toHaveLength(1);
  expect(explanations[0]).toMatchObject({
    backend: 'compatible',
    mode: 'chat',
    execution_result_id: jobs[0].id,
  });
  expect(explanations[0]).not.toHaveProperty('execution_revision_id');
  expect(explanations[0]).not.toHaveProperty('node_scope');
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue(
    'Keep this unsent request.',
  );
  await expect(
    page.getByRole('checkbox', { name: 'Analyze files and create outputs' }),
  ).toBeChecked();
  await expect(page.getByLabel('Model backend')).toHaveValue('groq');
  await openRuns(page);
  await page.getByRole('button', { name: 'Explain result', exact: true }).click();
  await expect.poll(() => explanations.length).toBe(2);
  expect(explanations[1].backend).toBe('groq');
  expect(executions).toHaveLength(1);
});

test('lost execution receipts reconcile without replay and a failed explanation can be retried without rerunning', async ({
  page,
  request,
}) => {
  const { workspace, detail, jobs, revision } = await fixture(page, request);
  let executions = 0;
  let explanations = 0;
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) => {
    if (route.request().method() !== 'POST') return route.fulfill({ json: jobs });
    executions++;
    const input = route.request().postDataJSON();
    jobs.push({ ...job(receipt(workspace.id, revision.id)), id: input.id });
    return route.abort('failed');
  });
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    explanations++;
    return explanations === 1
      ? route.fulfill({ status: 503, json: { error: 'Explanation unavailable' } })
      : route.fulfill({ status: 202, json: publishExplanation(detail, jobs[0], 'compatible') });
  });
  await page.goto('/');
  await openExecution(page);
  await page.getByRole('button', { name: 'Run and explain', exact: true }).click();
  await expect(
    page.getByRole('region', { name: 'Node execution', exact: true }).getByRole('alert'),
  ).toContainText('Explanation unavailable');
  expect(executions).toBe(1);
  expect(explanations).toBe(1);
  await openRuns(page);
  await page.getByRole('button', { name: 'Explain result', exact: true }).click();
  await expect(page.getByLabel('Conversation')).toContainText('The saved script printed 42.');
  expect(executions).toBe(1);
  expect(explanations).toBe(2);
});

test('stopping Run and explain cancels its automatic explanation', async ({ page, request }) => {
  const { workspace, jobs, revision } = await fixture(page, request);
  let explanations = 0;
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) => {
    if (route.request().method() !== 'POST') return route.fulfill({ json: jobs });
    const input = route.request().postDataJSON();
    jobs.push({
      ...job(receipt(workspace.id, revision.id)),
      id: input.id,
      status: 'running',
      exit_code: null,
      finished_at_ms: null,
    });
    return route.fulfill({ status: 202, json: jobs[0] });
  });
  await page.route('**/api/executions/*', (route) => {
    Object.assign(jobs[0], { status: 'cancelled', finished_at_ms: Date.now(), error: 'Cancelled' });
    return route.fulfill({ json: jobs[0] });
  });
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    explanations++;
    return route.abort();
  });
  await page.goto('/');
  await openExecution(page);
  await page.getByRole('button', { name: 'Run and explain', exact: true }).click();
  await page.getByRole('button', { name: 'Stop execution', exact: true }).click();
  await expect(page.locator('.execution-status')).toHaveText('cancelled');
  await openRuns(page);
  await expect(page.getByRole('button', { name: 'Explain result', exact: true })).toBeEnabled();
  expect(explanations).toBe(0);
});

test('leaving a workspace drops its pending explanation and reopening never replays it', async ({
  page,
  request,
}) => {
  const { workspace, jobs, revision } = await fixture(page, request);
  let explanations = 0;
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) => {
    if (route.request().method() !== 'POST') return route.fulfill({ json: jobs });
    const input = route.request().postDataJSON();
    jobs.push({
      ...job(receipt(workspace.id, revision.id)),
      id: input.id,
      status: 'running',
      exit_code: null,
      finished_at_ms: null,
    });
    return route.fulfill({ status: 202, json: jobs[0] });
  });
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    explanations++;
    return route.abort();
  });
  await page.goto('/');
  await openExecution(page);
  await page.getByRole('button', { name: 'Run and explain', exact: true }).click();
  await expect(page.locator('.execution-status')).toHaveText('running');
  await page.getByRole('button', { name: 'New workspace' }).click();
  Object.assign(jobs[0], { status: 'succeeded', exit_code: 0, finished_at_ms: Date.now() });
  await openWorkspaces(page);
  await page.getByRole('button', { name: workspace.title }).click();
  await expect(page.locator('.execution-status')).toHaveText('succeeded');
  await openRuns(page);
  await expect(page.getByRole('button', { name: 'Explain result', exact: true })).toBeEnabled();
  expect(explanations).toBe(0);
});

test('a native result with a different revision cannot replace validated conversation evidence', async ({
  page,
  request,
}) => {
  const { workspace, detail, revision, jobs } = await fixture(page, request);
  const run = receipt(workspace.id, revision.id);
  detail.runs.push(run);
  detail.messages.push({
    role: 'assistant',
    run_id: run.id,
    content: 'Keep the valid native answer',
  });
  jobs.push(job(run));
  await page.goto('/');
  await expect(page.getByLabel('Tool activity')).toContainText('run main.cjs · succeeded');
  run.tool_execution!.result!.revision_id = crypto.randomUUID();
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(page.locator('.run-error[role=alert]')).toContainText('incompatible shape');
  await expect(page.getByLabel('Conversation')).toContainText('Keep the valid native answer');
  await expect(page.getByLabel('Tool activity')).toContainText('run main.cjs · succeeded');
});

test('a native job admitted after its intent appears is observed while the model run remains active', async ({
  page,
  request,
}) => {
  const { workspace, detail, revision, jobs } = await fixture(page, request);
  const run = receipt(workspace.id, revision.id);
  const finished = run.tool_execution!.result!;
  run.status = 'running';
  run.model_rounds = 1;
  run.tool_execution!.result = null;
  let submitted = false;
  let observed!: () => void;
  const emptyAfterIntent = new Promise<void>((resolve) => {
    observed = resolve;
  });
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) => {
    if (submitted && jobs.length === 0) {
      observed();
      return route.fulfill({ json: [] });
    }
    return route.fulfill({ json: jobs });
  });
  submitted = true;
  detail.runs.push(run);
  detail.messages.push({ role: 'user', run_id: run.id, content: 'Run saved script' });
  await page.route(`**/api/runs/${run.id}`, (route) => route.fulfill({ json: run }));
  await page.goto('/');
  await emptyAfterIntent;
  jobs.push({
    ...finished,
    status: 'running',
    exit_code: null,
    image_id: `sha256:${'a'.repeat(64)}`,
    outputs: [],
    created_at_ms: Date.now(),
    finished_at_ms: null,
  });
  await openRuns(page);
  await expect(page.getByLabel('Execution results')).toBeVisible();
  await expect(page.locator('.execution-status')).toHaveText('running');
  await expect(page.getByRole('button', { name: 'Stop execution', exact: true })).toBeVisible();
  Object.assign(jobs[0], { status: 'succeeded', exit_code: 0, finished_at_ms: Date.now() });
  run.tool_execution!.result = finished;
  run.status = 'succeeded';
  detail.messages.push({ role: 'assistant', run_id: run.id, content: 'Delayed job completed' });
  await expect(page.locator('.execution-status')).toHaveText('succeeded');
  await expect(page.getByLabel('Conversation')).toContainText('Delayed job completed');
});
