import { openExecution, openRuns, closeRuns } from './helpers';
import { test, expect } from '@playwright/test';
import type { APIRequestContext, Page } from '@playwright/test';
import type { Execution, ExecutionAttachment, Run, WorkspaceDetail } from '../../apps/web/src/api';

async function fixture(page: Page, request: APIRequestContext) {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Result repair' } })
  ).json();
  const proposal = await (
    await request.post(`/api/workspaces/${workspace.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: null },
        changes: [{ op: 'write', path: 'main.cjs', content: "throw new Error('broken');" }],
      },
    })
  ).json();
  await request.post(`/api/proposals/${proposal.id}/apply`);
  const detail: WorkspaceDetail = await (
    await request.get(`/api/workspaces/${workspace.id}`)
  ).json();
  const job: Execution = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    revision_id: detail.revisions[0].id,
    entry: 'main.cjs',
    action: 'run',
    image_id: `sha256:${'b'.repeat(64)}`,
    status: 'failed',
    stdout: '<img src="http://invalid/">literal output',
    stderr: 'ReferenceError: missing is not defined',
    exit_code: 1,
    signal: null,
    truncated: false,
    error: 'Node exited with code 1.',
    created_at_ms: Date.now(),
    finished_at_ms: Date.now(),
    outputs: [],
  };
  const result: ExecutionAttachment = {
    id: job.id,
    workspace_id: job.workspace_id,
    revision_id: job.revision_id,
    entry: job.entry,
    action: job.action,
    status: 'failed',
    stdout: job.stdout,
    stderr: job.stderr,
    exit_code: job.exit_code,
    signal: job.signal,
    truncated: job.truncated,
    error: job.error,
    logs_shortened: false,
  };
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: false, ready: false, message: 'Worker disabled' } }),
  );
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) =>
    route.fulfill({ json: [job] }),
  );
  return { workspace, detail, job, result };
}

function receipt(detail: WorkspaceDetail, result: ExecutionAttachment | null): Run {
  return {
    id: crypto.randomUUID(),
    workspace_id: detail.workspace.id,
    provider: 'demo',
    model: 'demo-model',
    status: 'succeeded',
    error: null,
    output: '',
    mode: 'files',
    base_revision_id: detail.revisions.at(-1)!.id,
    stats: {
      reported_model: null,
      finish_reason: null,
      input_tokens: null,
      output_tokens: null,
      total_tokens: null,
      first_token_ms: null,
    },
    queue_ms: 0,
    duration_ms: 1,
    execution_result: result,
  };
}

test('execution cards draft an explicit result request, retain user text/backend and show the frozen attachment after reload', async ({
  page,
  request,
}) => {
  const { workspace, detail, job, result } = await fixture(page, request);
  const submissions: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    const input = route.request().postDataJSON();
    submissions.push(input);
    const run = receipt(detail, result);
    run.provider = input.backend;
    detail.runs.push(run);
    detail.messages.push({ role: 'user', run_id: run.id, content: input.prompt });
    return route.fulfill({ status: 202, json: run });
  });
  await page.goto('/');
  await openExecution(page);
  const activity = page.getByRole('region', { name: 'Execution results', exact: true });
  await expect(
    page.getByLabel('Conversation').getByRole('region', { name: 'Execution results', exact: true }),
  ).toHaveCount(0);
  await expect(activity).toContainText('main.cjs');
  await expect(activity.locator('.execution-status')).toHaveText('failed');
  await activity.getByText('Execution logs', { exact: true }).click();
  await expect(activity).toContainText('ReferenceError: missing');
  await expect(activity.locator('img')).toHaveCount(0);
  await closeRuns(page);
  await page.getByLabel('Message', { exact: true }).fill('Keep the existing counter behavior.');
  const backend = await page.getByLabel('Model backend').inputValue();
  await openRuns(page);
  await activity.getByRole('button', { name: 'Ask AI to fix', exact: true }).click();
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue(
    /Keep the existing counter behavior\.[\s\S]*attached execution result/,
  );
  await expect(page.getByRole('checkbox', { name: /^Change project files/ })).toBeChecked();
  await expect(page.getByLabel('Model backend')).toHaveValue(backend);
  await expect(page.getByLabel('Execution result attachment')).toContainText(job.id.slice(0, 8));
  expect(submissions).toHaveLength(0);
  await openRuns(page);
  await activity.getByRole('button', { name: 'Ask AI to fix', exact: true }).click();
  expect(
    (await page.getByLabel('Message', { exact: true }).inputValue()).match(/Use the attached/g),
  ).toHaveLength(1);
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.getByLabel('Execution result attachment')).toHaveCount(0);
  await expect(page.getByLabel('Included execution result')).toContainText(job.id.slice(0, 8));
  expect(submissions).toHaveLength(1);
  expect(submissions[0].execution_result_id).toBe(job.id);
  expect(submissions[0].base_revision).toEqual({ id: detail.revisions[0].id });
  expect(submissions[0]).not.toHaveProperty('stdout');
  await page.reload();
  await expect(page.getByLabel('Included execution result')).toContainText('main.cjs · failed');
});

test('removing a result or switching workspace prevents implicit log sharing', async ({
  page,
  request,
}) => {
  const { workspace } = await fixture(page, request);
  const submissions: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    submissions.push(route.request().postDataJSON());
    return route.fulfill({ status: 503, json: { error: 'Keep this draft' } });
  });
  await page.goto('/');
  await openExecution(page);
  await openRuns(page);
  await page
    .getByRole('region', { name: 'Execution results', exact: true })
    .getByRole('button', { name: 'Ask AI to fix' })
    .click();
  await page.getByRole('button', { name: 'Remove result', exact: true }).click();
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Keep this draft');
  expect(submissions[0]).not.toHaveProperty('execution_result_id');
  await openRuns(page);
  await page
    .getByRole('region', { name: 'Execution results', exact: true })
    .getByRole('button', { name: 'Ask AI to fix' })
    .click();
  await page.getByRole('button', { name: 'New workspace' }).click();
  await expect(page.getByLabel('Execution result attachment')).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Execution results', exact: true })).toHaveCount(0);
});

test('failed submission retains its attachment and a mismatched receipt is rejected', async ({
  page,
  request,
}) => {
  const { workspace, detail, result } = await fixture(page, request);
  let attempts = 0;
  const requests: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    attempts++;
    requests.push(route.request().postDataJSON());
    return attempts === 1
      ? route.fulfill({ status: 409, json: { error: 'Result is not ready' } })
      : route.fulfill({
          status: 202,
          json: receipt(detail, { ...result, id: crypto.randomUUID() }),
        });
  });
  await page.goto('/');
  await openExecution(page);
  await openRuns(page);
  await page
    .getByRole('region', { name: 'Execution results', exact: true })
    .getByRole('button', { name: 'Ask AI to fix' })
    .click();
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Result is not ready');
  await expect(page.getByLabel('Execution result attachment')).toBeVisible();
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('incompatible shape');
  await expect(page.getByLabel('Execution result attachment')).toBeVisible();
  expect(requests[0].execution_result_id).toBe(requests[1].execution_result_id);
});

test('an older result names its input revision while a fix targets the current head', async ({
  page,
  request,
}) => {
  const { workspace, detail, result } = await fixture(page, request);
  const response = await request.post(`/api/workspaces/${workspace.id}/proposals`, {
    data: {
      id: crypto.randomUUID(),
      base_revision: { id: detail.revisions[0].id },
      changes: [{ op: 'write', path: 'note.txt', content: 'new head' }],
    },
  });
  const proposal = await response.json();
  const revision = await (await request.post(`/api/proposals/${proposal.id}/apply`)).json();
  const sent: Record<string, unknown>[] = [];
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    sent.push(route.request().postDataJSON());
    return route.fulfill({ status: 503, json: { error: 'Fixture keeps draft' } });
  });
  await page.goto('/');
  await openExecution(page);
  const activity = page.getByRole('region', { name: 'Execution results', exact: true });
  await expect(activity).toContainText('revision 1');
  await expect(activity).toContainText('older revision');
  await openRuns(page);
  await activity.getByRole('button', { name: 'Ask AI to fix' }).click();
  await expect(page.getByLabel('Execution result attachment')).toContainText('older revision');
  await page.getByRole('button', { name: /^Send/ }).click();
  await expect(page.locator('.error[role=alert]')).toContainText('Fixture keeps draft');
  expect(sent[0].base_revision).toEqual({ id: revision.id });
  expect(sent[0].execution_result_id).toBe(result.id);
});

test('active results cannot draft a fix', async ({ page, request }) => {
  const { workspace, job } = await fixture(page, request);
  job.status = 'running';
  job.finished_at_ms = null;
  job.exit_code = null;
  await page.goto('/');
  await openExecution(page);
  const activity = page.getByRole('region', { name: 'Execution results', exact: true });
  await expect(activity.getByRole('button', { name: 'Ask AI to fix' })).toBeDisabled();
  await page.getByRole('button', { name: 'New workspace' }).click();
  await expect(page.getByLabel('Execution result attachment')).toHaveCount(0);
  await expect(activity).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Choose workspace', exact: true }),
  ).not.toContainText('Result repair');
  expect(workspace.id).not.toBe('');
});

test('a late execution read cannot publish result cards into another workspace', async ({
  page,
  request,
}) => {
  const { workspace, job } = await fixture(page, request);
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let reading: () => void = () => {};
  const started = new Promise<void>((resolve) => {
    reading = resolve;
  });
  await page.route(`**/api/workspaces/${workspace.id}/executions`, async (route) => {
    reading();
    await gate;
    await route.fulfill({ json: [job] }).catch(() => {});
  });
  await page.goto('/');
  await openExecution(page);
  await started;
  await page.getByRole('button', { name: 'New workspace' }).click();
  release();
  await expect(
    page.getByRole('button', { name: 'Choose workspace', exact: true }),
  ).not.toContainText('Result repair');
  await expect(page.getByRole('region', { name: 'Execution results', exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Execution result attachment')).toHaveCount(0);
});

test('invalid attached observations preserve the last valid conversation', async ({
  page,
  request,
}) => {
  const { workspace, detail, result } = await fixture(page, request);
  const run = receipt(detail, result);
  detail.runs.push(run);
  detail.messages.push({ role: 'user', content: 'Preserve this message', run_id: run.id });
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  await page.goto('/');
  await openExecution(page);
  await expect(page.getByLabel('Included execution result')).toBeVisible();
  result.workspace_id = crypto.randomUUID();
  await closeRuns(page);
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(page.locator('.run-error[role=alert]')).toContainText('incompatible shape');
  await expect(page.getByLabel('Conversation')).toContainText('Preserve this message');
  await expect(page.getByLabel('Included execution result')).toBeVisible();
});

test('long attachment logs flow through the Chat scroll area on mobile and desktop', async ({
  page,
  request,
}) => {
  const { job } = await fixture(page, request);
  job.stderr = Array.from(
    { length: 250 },
    (_, index) => `${index}: ${'long-stack-frame/'.repeat(8)}`,
  ).join('\n');
  await page.goto('/');
  await openExecution(page);
  await openRuns(page);
  await page
    .getByRole('region', { name: 'Execution results', exact: true })
    .getByRole('button', { name: 'Ask AI to fix' })
    .click();
  const attachment = page.getByLabel('Execution result attachment');
  await attachment.getByText('Review saved result', { exact: true }).click();
  await attachment.getByText('Execution logs', { exact: true }).click();
  const log = attachment.locator('pre').last();
  const scroller = page.locator('.pane-chat .workspace-pane-body');
  for (const width of [390, 1440]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect
      .poll(() =>
        log.evaluate(
          (node) => node.scrollHeight <= node.clientHeight + 1 && node.clientHeight > 200,
        ),
      )
      .toBe(true);
    const outer = await attachment.boundingBox();
    const box = await log.boundingBox();
    expect(box!.x + box!.width).toBeLessThanOrEqual(outer!.x + outer!.width);
    expect(box!.y + box!.height).toBeLessThanOrEqual(outer!.y + outer!.height);
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
      .toBe(true);
    await scroller.evaluate((node) => {
      node.scrollTop = node.scrollHeight;
    });
    await expect(page.getByRole('button', { name: 'Remove result', exact: true })).toBeInViewport();
    await page.getByLabel('Message', { exact: true }).scrollIntoViewIfNeeded();
    await expect(page.getByLabel('Message', { exact: true })).toBeInViewport();
    await page.locator('.pane-chat').screenshot({ path: `logs/chat-log-layout-${width}.png` });
  }
});
