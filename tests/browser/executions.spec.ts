import {
  closeRuns,
  expectProposal,
  openExecution,
  openRuns,
  selectModel,
  selectProposal,
} from './helpers';
import { test, expect } from '@playwright/test';
import type { Execution, Proposal, Revision } from '../../apps/web/src/api';

async function project(request: import('@playwright/test').APIRequestContext) {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: `Execution ${Date.now()}` } })
  ).json();
  const response = await request.post(`/api/workspaces/${workspace.id}/runs`, {
    data: {
      prompt: 'Counter project',
      mode: 'files',
      backend: 'demo',
      base_revision: { id: null },
    },
  });
  const run = await response.json();
  await expect
    .poll(async () => (await (await request.get(`/api/runs/${run.id}`)).json()).status)
    .toBe('succeeded');
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  await request.post(`/api/proposals/${detail.proposals[0].id}/apply`);
  return workspace;
}

test('execution uses a stable receipt through lost submission, failed reads, cancellation and reload', async ({
  page,
  request,
}) => {
  const workspace = await project(request);
  let jobs: Execution[] = [];
  let failReads = false;
  const submitted: unknown[] = [];
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Node worker ready' } }),
  );
  await page.route(`**/api/workspaces/${workspace.id}/executions`, async (route) => {
    if (route.request().method() === 'GET') {
      return failReads
        ? route.fulfill({ status: 503, json: { error: 'Temporary execution read failure' } })
        : route.fulfill({ json: jobs });
    }
    const input = route.request().postDataJSON();
    submitted.push(input);
    if (!jobs.length)
      jobs = [
        {
          ...input,
          workspace_id: workspace.id,
          image_id: `sha256:${'a'.repeat(64)}`,
          status: 'running',
          stdout: '',
          stderr: '',
          exit_code: null,
          signal: null,
          truncated: false,
          error: null,
          created_at_ms: Date.now(),
          finished_at_ms: null,
          outputs: [],
        },
      ];
    if (submitted.length === 1) {
      failReads = true;
      return route.abort('failed');
    }
    failReads = false;
    return route.fulfill({ status: 202, json: jobs[0] });
  });
  await page.route('**/api/executions/*', (route) => {
    jobs[0] = { ...jobs[0], status: 'cancelled', finished_at_ms: Date.now() };
    return route.fulfill({ json: jobs[0] });
  });
  await page.goto('/');
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeEnabled();
  expect(submitted).toHaveLength(0);
  await openExecution(page);
  await page.getByRole('button', { name: 'Run script', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Retry same execution', exact: true }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Retry same execution', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Stop execution', exact: true })).toBeVisible();
  expect(submitted).toHaveLength(2);
  expect(submitted[1]).toEqual(submitted[0]);
  expect(jobs[0].entry).toBe('app.js');
  expect(jobs[0].action).toBe('run');
  await page.getByRole('button', { name: 'Stop execution', exact: true }).click();
  await expect(page.getByLabel('Execution history')).toContainText('cancelled');
  await page.reload();
  await expect(page.getByLabel('Execution history')).toContainText('cancelled');
  expect(submitted).toHaveLength(2);
});

test('script actions follow the selected saved file and results remain visible across navigation and reload', async ({
  page,
  request,
}) => {
  const workspace = await project(request);
  const path = `/api/workspaces/${workspace.id}`;
  const detail = await (await request.get(path)).json();
  const proposal = await (
    await request.post(`${path}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: detail.revisions[0].id },
        changes: ['a.cjs', 'b.cjs'].map((name) => ({
          op: 'write',
          path: name,
          content: `console.log('${name}');`,
        })),
      },
    })
  ).json();
  const revision: Revision = await (
    await request.post(`/api/proposals/${proposal.id}/apply`)
  ).json();
  const jobs: Execution[] = [];
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Node worker ready' } }),
  );
  await page.route(`**${path}/executions`, (route) => {
    if (route.request().method() === 'GET') return route.fulfill({ json: jobs });
    const input = route.request().postDataJSON();
    const job: Execution = {
      ...input,
      workspace_id: workspace.id,
      image_id: `sha256:${'c'.repeat(64)}`,
      status: 'succeeded',
      stdout: `Completed ${input.entry}`,
      stderr: '',
      exit_code: 0,
      signal: null,
      truncated: false,
      error: null,
      created_at_ms: Date.now(),
      finished_at_ms: Date.now(),
      outputs: [],
    };
    jobs.push(job);
    return route.fulfill({ status: 202, json: job });
  });
  await page.goto('/');
  await closeRuns(page);
  await page.getByRole('button', { name: 'b.cjs', exact: true }).click();
  await expect(page.locator('.execution-entry')).toHaveText('b.cjs');
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Run script', exact: true })).toBeVisible();
  expect(jobs).toHaveLength(0);
  await openExecution(page);
  await page.getByRole('button', { name: 'Check syntax', exact: true }).click();
  await openRuns(page);
  await expect(page.getByLabel('Execution results')).toContainText('Syntax check · revision 2');
  expect(jobs[0]).toMatchObject({ revision_id: revision.id, entry: 'b.cjs', action: 'check' });
  await closeRuns(page);
  await page.getByRole('button', { name: 'a.cjs', exact: true }).click();
  expect(jobs).toHaveLength(1);
  await openExecution(page);
  await page.getByRole('button', { name: 'Run script', exact: true }).click();
  await openRuns(page);
  await expect(page.getByLabel('Execution results')).toContainText('Script run · revision 2');
  expect(jobs[1]).toMatchObject({ revision_id: revision.id, entry: 'a.cjs', action: 'run' });
  await closeRuns(page);
  await page.getByRole('button', { name: 'index.html', exact: true }).click();
  await openRuns(page);
  await expect(page.getByLabel('Execution results')).toBeVisible();
  await expect(page.getByLabel('Execution results')).toHaveCount(1);
  await expect(page.locator('.execution-tools')).not.toHaveAttribute('open', '');
  await page.reload();
  await openRuns(page);
  await expect(page.getByLabel('Execution results')).toContainText('a.cjs');
  await expect(page.getByLabel('Execution results')).toBeVisible();
  expect(jobs).toHaveLength(2);
  const deletion = await (
    await request.post(`${path}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: revision.id },
        changes: [{ op: 'delete', path: 'a.cjs' }],
      },
    })
  ).json();
  await request.post(`/api/proposals/${deletion.id}/apply`);
  await closeRuns(page);
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await closeRuns(page);
  await selectProposal(page, 'Proposal 3 · applied');
  await expect(page.locator('.execution-entry')).toHaveText('a.cjs');
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Run script', exact: true })).toBeDisabled();
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeDisabled();
  await expect(
    page.getByText('This file is not present in saved revision 3.', { exact: false }),
  ).toBeVisible();
  expect(jobs).toHaveLength(2);
});

test('invalid execution observations preserve the last good output; switching workspaces discards late responses', async ({
  page,
  request,
}) => {
  const workspace = await project(request);
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const job: Execution = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    revision_id: detail.revisions[0].id,
    entry: 'app.js',
    action: 'check',
    image_id: `sha256:${'b'.repeat(64)}`,
    status: 'succeeded',
    stdout: 'Good output',
    stderr: '',
    exit_code: 0,
    signal: null,
    truncated: false,
    error: null,
    created_at_ms: Date.now(),
    finished_at_ms: Date.now(),
    outputs: [],
  };
  let invalid = false;
  let hold = false;
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let observed!: () => void;
  const waiting = new Promise<void>((resolve) => {
    observed = resolve;
  });
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Ready' } }),
  );
  await page.route(`**/api/workspaces/${workspace.id}/executions`, async (route) => {
    if (hold) {
      observed();
      await held;
    }
    await route
      .fulfill({ json: [invalid ? { ...job, exit_code: 'zero', stdout: 'Bad replacement' } : job] })
      .catch(() => {});
  });
  await page.goto('/');
  await openExecution(page);
  await expect(page.getByLabel('Execution stdout')).toHaveText('Good output');
  invalid = true;
  await page.getByRole('button', { name: 'Refresh worker', exact: true }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'incompatible shape',
  );
  await expect(page.getByLabel('Execution stdout')).toHaveText('Good output');
  invalid = false;
  hold = true;
  await page.getByRole('button', { name: 'Refresh worker', exact: true }).click();
  await waiting;
  await page.getByRole('button', { name: 'New workspace' }).click();
  release();
  await expect(page.getByLabel('Execution stdout')).toHaveCount(0);
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toHaveCount(0);
});

function outputJob(workspace: string, revision: string): Execution {
  return {
    id: crypto.randomUUID(),
    workspace_id: workspace,
    revision_id: revision,
    entry: 'app.js',
    action: 'run',
    image_id: `sha256:${'c'.repeat(64)}`,
    status: 'succeeded',
    stdout: 'built',
    stderr: '',
    exit_code: 0,
    signal: null,
    truncated: false,
    error: null,
    created_at_ms: Date.now(),
    finished_at_ms: Date.now(),
    outputs: [{ path: 'index.html', bytes: 22 }],
  };
}

test('HTML-only projects guide users to preview and keep Node tools secondary', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'HTML entry guidance' } })
  ).json();
  const run = await (
    await request.post(`/api/workspaces/${workspace.id}/runs`, {
      data: { prompt: 'A counter', mode: 'app', backend: 'demo' },
    })
  ).json();
  await expect
    .poll(async () => (await (await request.get(`/api/runs/${run.id}`)).json()).status)
    .toBe('succeeded');
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  expect(detail.revisions).toHaveLength(0);
  await request.post(`/api/proposals/${detail.proposals[0].id}/apply`);
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Node worker ready' } }),
  );
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'demo',
        model: 'demo-model',
        model_health: 'not_probed',
        backends: [
          { id: 'demo', model: 'demo-model' },
          { id: 'openai', model: 'gpt-5.4-mini' },
        ],
      },
    }),
  );
  await page.goto('/');
  await expect(page.locator('.execution-tools')).not.toHaveAttribute('open', '');
  await openRuns(page);
  await page.getByText('Run JavaScript', { exact: true }).click();
  await expect(page.getByLabel('Execution entry')).toBeDisabled();
  await expect(page.getByLabel('Execution entry').locator('option')).toHaveText(
    'No JavaScript files in this revision',
  );
  await expect(page.getByText(/Revision 1 has no .js, .mjs or .cjs files/)).toBeVisible();
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Run script', exact: true })).toBeDisabled();
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeDisabled();
  await closeRuns(page);
  await selectModel(page, 'openai');
  await expect(page.getByRole('button', { name: 'Draft Node generator', exact: true })).toHaveCount(
    0,
  );
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(true);
  await expect(page.getByRole('checkbox', { name: /^Change project files/ })).toBeChecked();
  await expect(page.getByLabel('Model backend')).toHaveValue('openai');
});

test('pending proposals block execution while saved and applied proposal views retain valid entries', async ({
  page,
  request,
}) => {
  const workspace = await project(request);
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const run = await (
    await request.post(`/api/workspaces/${workspace.id}/runs`, {
      data: {
        prompt: 'Update counter',
        mode: 'files',
        backend: 'demo',
        base_revision: { id: detail.revisions[0].id },
      },
    })
  ).json();
  await expect
    .poll(async () => (await (await request.get(`/api/runs/${run.id}`)).json()).status)
    .toBe('succeeded');
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Node worker ready' } }),
  );
  let launched = 0;
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) => {
    if (route.request().method() === 'POST') launched++;
    return route.fulfill({ json: [] });
  });
  await page.goto('/');
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeEnabled();
  await closeRuns(page);
  await selectProposal(page, 'Proposal 2 · pending');
  await openRuns(page);
  await expect(page.getByText('Node runs saved JavaScript files.', { exact: false })).toBeVisible();
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeDisabled();
  await closeRuns(page);
  await page.getByRole('button', { name: 'View current', exact: true }).click();
  await expect(page.locator('.execution-entry')).toHaveText('app.js');
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeEnabled();
  await closeRuns(page);
  await selectProposal(page, 'Proposal 1 · applied');
  await closeRuns(page);
  await page.getByRole('button', { name: 'app.js', exact: true }).click();
  await expect(page.locator('.execution-entry')).toHaveText('app.js');
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeEnabled();
  expect(launched).toBe(0);
});

function outputProposal(job: Execution): Proposal {
  return {
    id: crypto.randomUUID(),
    workspace_id: job.workspace_id,
    run_id: null,
    edit_id: null,
    execution_id: job.id,
    base_revision_id: job.revision_id,
    status: 'pending',
    changes: [{ op: 'write', path: 'index.html', content: '<h1>Worker output</h1>' }],
    applied_revision_id: null,
    rejection_reason: null,
  };
}

test('worker output review retains its receipt on failed refresh and applies with execution provenance', async ({
  page,
  request,
}) => {
  const workspace = await project(request);
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const job = outputJob(workspace.id, detail.revisions[0].id);
  const proposal = outputProposal(job);
  let failReads = false;
  let submissions = 0;
  await page.route('**/api/worker', (route) =>
    route.fulfill({
      json: {
        enabled: false,
        ready: false,
        message: 'Execution disabled; saved output remains available',
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) =>
    route.fulfill({ json: [job] }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    failReads
      ? route.fulfill({ status: 503, json: { error: 'Output refresh temporarily failed' } })
      : route.fulfill({ json: detail }),
  );
  await page.route(`**/api/executions/${job.id}/proposal`, (route) => {
    submissions++;
    if (submissions === 1) {
      detail.proposals.push(proposal);
      return route.abort('failed');
    }
    failReads = true;
    return route.fulfill({ json: proposal });
  });
  await page.route(`**/api/proposals/${proposal.id}/apply`, (route) => {
    const revision: Revision = {
      id: crypto.randomUUID(),
      run_id: null,
      edit_id: null,
      execution_id: job.id,
      number: 2,
      html: '<h1>Worker output</h1>',
      files: detail.revisions[0].files.map((file: { path: string; content: string }) =>
        file.path === 'index.html' ? { ...file, content: '<h1>Worker output</h1>' } : file,
      ),
    };
    detail.revisions.push(revision);
    proposal.status = 'applied';
    proposal.applied_revision_id = revision.id;
    return route.fulfill({ json: revision });
  });
  await page.goto('/');
  await openExecution(page);
  await expect(
    page.getByRole('button', { name: 'Review output changes', exact: true }),
  ).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Review output changes', exact: true }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toBeVisible();
  await page.getByRole('button', { name: 'Review output changes', exact: true }).click();
  await expect(page.getByLabel('File diff')).toContainText('<h1>Worker output</h1>');
  await expect(page.getByText(/Source: execution output/)).toBeVisible();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'Output refresh temporarily failed',
  );
  expect(submissions).toBe(2);
  expect(detail.proposals).toHaveLength(2);
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(2);
  await closeRuns(page);
  await expectProposal(page, 'Proposal 2 · applied');
  failReads = false;
  await page.reload();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(2);
  await page
    .getByLabel('File list')
    .getByRole('button', { name: 'index.html', exact: true })
    .click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Worker output');
});

test('an output proposal waits for its exact base before rendering a diff after a failed refresh', async ({
  page,
  request,
}) => {
  const workspace = await project(request);
  const initial = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const response = await request.post(`/api/workspaces/${workspace.id}/runs`, {
    data: {
      prompt: 'Change step',
      mode: 'files',
      backend: 'demo',
      base_revision: { id: initial.revisions[0].id },
    },
  });
  const run = await response.json();
  await expect
    .poll(async () => (await (await request.get(`/api/runs/${run.id}`)).json()).status)
    .toBe('succeeded');
  const updated = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  await request.post(`/api/proposals/${updated.proposals.at(-1).id}/apply`);
  const full = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const job = outputJob(workspace.id, full.revisions.at(-1).id);
  const proposal = outputProposal(job);
  let fail = false;
  let latest = false;
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: false, ready: false, message: 'Saved output' } }),
  );
  await page.route(`**/api/workspaces/${workspace.id}/executions`, (route) =>
    route.fulfill({ json: [job] }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    fail
      ? route.fulfill({ status: 503, json: { error: 'Base temporarily unavailable' } })
      : route.fulfill({ json: latest ? full : initial }),
  );
  await page.route(`**/api/executions/${job.id}/proposal`, (route) => {
    fail = true;
    full.proposals.push(proposal);
    return route.fulfill({ json: proposal });
  });
  await page.goto('/');
  await openExecution(page);
  await page.getByRole('button', { name: 'Review output changes', exact: true }).click();
  await expect(
    page.getByText(
      'Proposal saved. Refresh files to load its base revision before reviewing or applying it.',
    ),
  ).toBeVisible();
  await expect(page.getByLabel('File diff')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toBeDisabled();
  fail = false;
  latest = true;
  await closeRuns(page);
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(page.getByLabel('File diff')).toContainText('Worker output');
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toBeEnabled();
});
