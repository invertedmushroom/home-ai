import {
  closeWorkspaces,
  expectProposal,
  openWorkspaces,
  selectModel,
  selectProposal,
  selectRevision,
} from './helpers';
import { test, expect } from '@playwright/test';
import type { Run } from '../../apps/web/src/api';

const emptyStats = {
  reported_model: null,
  finish_reason: null,
  input_tokens: null,
  output_tokens: null,
  total_tokens: null,
  first_token_ms: null,
};

test('a streamed draft survives reload and cancellation without publishing an app', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Draft recovery' } })
  ).json();
  const run: Run = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    status: 'running',
    error: null,
    provider: 'lmstudio',
    model: 'configured-model',
    output: '```html\n<h1>Partial 界🙂',
    queue_ms: 40,
    duration_ms: 1500,
    mode: 'chat',
    base_revision_id: null,
    stats: {
      ...emptyStats,
      reported_model: 'reported-revision',
      first_token_ms: 200,
      input_tokens: 0,
    },
  };
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    route.fulfill({
      json: {
        workspace,
        messages: [{ role: 'user', content: 'Build an app', run_id: run.id }],
        runs: [run],
        revisions: [],
        proposals: [],
      },
    }),
  );
  await page.route(`**/api/runs/${run.id}`, (route) => {
    expect(route.request().method()).toBe('DELETE');
    run.status = 'cancelled';
    run.error = 'Local request cancelled; upstream GPU interruption is not confirmed.';
    return route.fulfill({ json: run });
  });
  await page.goto('/');
  await expect(page.getByLabel('Draft response').locator('pre')).toHaveText(run.output);
  await expect(page.getByLabel('Tool activity')).toContainText('0 in / unknown out tokens');
  await expect(page.getByLabel('Tool activity')).toContainText('Reported model: reported-revision');
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(0);
  await expect(page.locator('iframe')).toHaveCount(0);
  await page.reload();
  await expect(page.getByLabel('Draft response').locator('pre')).toHaveText(run.output);
  await page.locator('.composer').getByRole('button', { name: 'Stop request' }).click();
  await expect(page.getByLabel('Draft response')).toContainText('Incomplete response · cancelled');
  await page.reload();
  await expect(page.getByLabel('Draft response')).toContainText('Incomplete response · cancelled');
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(0);
  await expect(page.locator('iframe')).toHaveCount(0);
});

test('invalid streamed usage cannot replace the last valid draft', async ({ page, request }) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Draft validation' } })
  ).json();
  const run: Run = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    status: 'running',
    error: null,
    provider: 'lmstudio',
    model: 'local-model',
    output: 'Valid partial response',
    stats: emptyStats,
    queue_ms: 0,
    duration_ms: 100,
    mode: 'chat',
    base_revision_id: null,
  };
  let invalid = false;
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    route.fulfill({
      json: {
        workspace,
        messages: [{ role: 'user', content: 'Start', run_id: run.id }],
        revisions: [],
        proposals: [],
        runs: [
          invalid
            ? { ...run, output: 'Invalid replacement', stats: { ...emptyStats, input_tokens: -1 } }
            : run,
        ],
      },
    }),
  );
  await page.goto('/');
  await expect(page.getByLabel('Draft response').locator('pre')).toHaveText(
    'Valid partial response',
  );
  invalid = true;
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'incompatible shape',
  );
  await expect(page.getByLabel('Draft response').locator('pre')).toHaveText(
    'Valid partial response',
  );
  await expect(page.getByText('Invalid replacement', { exact: true })).toHaveCount(0);
});

test('backend selection is submitted explicitly and reply provenance survives reload', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Backend selection' } })
  ).json();
  const backends = [
    { id: 'lmstudio', model: 'local-test' },
    { id: 'groq', model: 'hosted-test' },
  ];
  const runs: Run[] = [];
  const messages: { role: string; content: string; run_id: string }[] = [];
  const submitted: string[] = [];
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'lmstudio',
        model: 'local-test',
        model_health: 'not_probed',
        backends,
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    route.fulfill({
      json: {
        workspace,
        messages,
        runs,
        revisions: [],
        proposals: [],
      },
    }),
  );
  await page.route(`**/api/workspaces/${workspace.id}/runs`, async (route) => {
    const input = route.request().postDataJSON();
    submitted.push(input.backend);
    const model = backends.find((backend) => backend.id === input.backend)!.model;
    const run: Run = {
      id: crypto.randomUUID(),
      workspace_id: workspace.id,
      provider: input.backend,
      workspace_tools: input.workspace_tools,
      model,
      status: 'succeeded',
      error: null,
      output: '',
      stats: emptyStats,
      queue_ms: null,
      duration_ms: null,
      mode: 'chat',
      base_revision_id: null,
    };
    runs.push(run);
    messages.push(
      { role: 'user', content: input.prompt, run_id: run.id },
      { role: 'assistant', content: `Reply from ${input.backend}`, run_id: run.id },
    );
    await route.fulfill({ status: 202, json: run });
  });
  await page.goto('/');
  await expect(page.getByLabel('Model backend')).toHaveValue('lmstudio');
  for (const backend of ['groq', 'lmstudio']) {
    await selectModel(page, backend);
    await page.getByLabel('Message', { exact: true }).fill(`Question for ${backend}`);
    await page.getByRole('button', { name: 'Send', exact: false }).click();
    await expect(page.getByText(`Reply from ${backend}`, { exact: true })).toBeVisible();
  }
  expect(submitted).toEqual(['groq', 'lmstudio']);
  await page.reload();
  await expect(
    page.locator('.message-label').getByText('groq · hosted-test', { exact: true }),
  ).toBeVisible();
  await expect(
    page.locator('.message-label').getByText('lmstudio · local-test', { exact: true }),
  ).toBeVisible();
});

test('inconsistent backend metadata is rejected before it can select a destination', async ({
  page,
}) => {
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        provider: 'lmstudio',
        model: 'local-test',
        model_health: 'not_probed',
        backends: [{ id: 'groq', model: 'hosted-test' }],
      },
    }),
  );
  await page.goto('/');
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'incompatible shape',
  );
  await expect(page.getByLabel('Model backend')).toBeDisabled();
  await page.getByLabel('Message', { exact: true }).fill('Do not send this');
  await expect(page.getByRole('button', { name: 'Send', exact: false })).toBeDisabled();
});

test('revisions survive replies and reloads; previews require a click and have an opaque origin', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'New workspace' }).click();
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(true);
  await page.getByLabel('Message', { exact: true }).fill('Make a small counter');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(1);
  await page
    .getByLabel('File list')
    .getByRole('button', { name: 'index.html', exact: true })
    .click();
  await expect(page.locator('iframe')).toHaveCount(0);
  await page.getByRole('button', { name: 'Run preview' }).click();
  const frame = page.frameLocator('iframe');
  await expect(frame.getByRole('heading', { name: 'Project counter' })).toBeVisible();
  await frame.getByRole('button', { name: 'Add one' }).click();
  await expect(frame.locator('#count')).toHaveText('1');
  const iframe = page.frames().find((f) => f !== page.mainFrame())!;
  const isolated = await iframe.evaluate(() => {
    let parentBlocked = false,
      storageBlocked = false;
    try {
      void parent.document.body;
    } catch {
      parentBlocked = true;
    }
    try {
      void localStorage.length;
    } catch {
      storageBlocked = true;
    }
    return { parentBlocked, storageBlocked };
  });
  expect(isolated).toEqual({ parentBlocked: true, storageBlocked: true });
  expect(
    await iframe.evaluate(async () => {
      try {
        await fetch('/api/status');
        return false;
      } catch {
        return true;
      }
    }),
  ).toBe(true);

  await page.getByLabel('Message', { exact: true }).fill('Save another version');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expectProposal(page, 'Proposal 2 · pending');
  await expect(frame.locator('#count')).toHaveText('1');
  await expect(page.getByText('PREVIEWING REVISION 1')).toBeVisible();
  await selectProposal(page, 'Proposal 2 · pending');
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(2);
  await page.reload();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(2);
  await expect(page.locator('iframe')).toHaveCount(0);
  await selectRevision(page, { label: 'Revision 1' });
  await page
    .getByLabel('File list')
    .getByRole('button', { name: 'index.html', exact: true })
    .click();
  await page.getByRole('button', { name: 'Run preview' }).click();
  await expect(page.frameLocator('iframe').locator('#count')).toHaveText('0');
  await page.screenshot({ path: 'test-results/workbench.png', fullPage: true });
});

test('malformed workspace data preserves the last valid conversation', async ({
  page,
  request,
}) => {
  const created = await request.post('/api/workspaces', { data: { title: 'Validation check' } });
  const workspace = await created.json();
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    'Validation check',
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    route.fulfill({
      json: {
        workspace: { id: 42, title: 'Broken replacement' },
        messages: [],
        revisions: [],
        proposals: [],
        runs: [],
      },
    }),
  );
  await page.getByLabel('Message', { exact: true }).fill('Hello');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'incompatible shape',
  );
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    'Validation check',
  );
  await expect(page.getByText('Broken replacement')).toHaveCount(0);
});

test('a late read cannot replace the selected workspace', async ({ page, request }) => {
  const a = await (
    await request.post('/api/workspaces', { data: { title: 'Slow workspace' } })
  ).json();
  await request.post('/api/workspaces', { data: { title: 'Current workspace' } });
  const original = await (await request.get(`/api/workspaces/${a.id}`)).json();
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    'Current workspace',
  );
  let release!: () => void;
  let arrived!: () => void;
  const delayed = new Promise<void>((resolve) => {
    release = resolve;
  });
  const arrival = new Promise<void>((resolve) => {
    arrived = resolve;
  });
  await page.route(`**/api/workspaces/${a.id}`, async (route) => {
    arrived();
    await delayed;
    await route.fulfill({ json: original }).catch(() => {}); // The obsolete fetch may already be aborted.
  });
  await openWorkspaces(page);
  await page.getByRole('button', { name: 'Slow workspace', exact: false }).click();
  await arrival;
  await openWorkspaces(page);
  await page.getByRole('button', { name: 'Current workspace', exact: false }).click();
  release();
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    'Current workspace',
  );
});

test('an accepted run stays stoppable and recovers after its first detail read fails', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Recover accepted run' } })
  ).json();
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    workspace.title,
  );
  let reads = 0;
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => {
    if (++reads === 1)
      return route.fulfill({ status: 503, json: { error: 'Temporary detail failure' } });
    return route.continue();
  });
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(true);
  await page.getByLabel('Message', { exact: true }).fill('Recover this app');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'Temporary detail failure',
  );
  const stop = page.locator('.composer').getByRole('button', { name: 'Stop request' });
  await expect(stop).toBeVisible();
  await expect(stop).toHaveCount(0);
  await selectProposal(page, 'Proposal 1 · pending');
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(1);
  await page.getByLabel('Message', { exact: true }).fill('A second accepted app');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await selectProposal(page, 'Proposal 2 · pending');
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(2);
});

test('a late submission cannot abort a different workspace that is still loading', async ({
  page,
  request,
}) => {
  const selected = await (
    await request.post('/api/workspaces', { data: { title: 'Loading selection' } })
  ).json();
  const detail = await (await request.get(`/api/workspaces/${selected.id}`)).json();
  let releaseList!: () => void;
  let releaseReceipt!: () => void;
  let releaseDetail!: () => void;
  let receiptArrived!: () => void;
  let detailArrived!: () => void;
  const listGate = new Promise<void>((resolve) => (releaseList = resolve));
  const receiptGate = new Promise<void>((resolve) => (releaseReceipt = resolve));
  const detailGate = new Promise<void>((resolve) => (releaseDetail = resolve));
  const receiptArrival = new Promise<void>((resolve) => (receiptArrived = resolve));
  const detailArrival = new Promise<void>((resolve) => (detailArrived = resolve));
  await page.route('**/api/workspaces', async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    await listGate;
    await route.fulfill({ json: [selected] });
  });
  await page.route('**/api/workspaces/*/runs', async (route) => {
    const response = await route.fetch();
    receiptArrived();
    await receiptGate;
    await route.fulfill({ response });
  });
  await page.route(`**/api/workspaces/${selected.id}`, async (route) => {
    detailArrived();
    await detailGate;
    await route.fulfill({ json: detail }).catch(() => {}); // The regression aborts this read.
  });
  try {
    await page.goto('/');
    await page.getByLabel('Message', { exact: true }).fill('Submit before the sidebar finishes');
    await page.getByRole('button', { name: 'Send', exact: false }).click();
    await receiptArrival;
    releaseList();
    await detailArrival;
    releaseReceipt();
    await expect(page.getByRole('button', { name: /Sending/ })).toHaveCount(0);
    releaseDetail();
    await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
      selected.title,
    );
  } finally {
    releaseList();
    releaseReceipt();
    releaseDetail();
  }
});

test('Stop uses the accepted receipt even when detail reads are unavailable', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Stop without detail' } })
  ).json();
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    workspace.title,
  );
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    route.fulfill({ status: 503, json: { error: 'Detail unavailable' } }),
  );
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(true);
  await page.getByLabel('Message', { exact: true }).fill('Stop this app');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'Detail unavailable',
  );
  const stop = page.locator('.composer').getByRole('button', { name: 'Stop request' });
  await stop.click();
  await expect(stop).toHaveCount(0);
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  expect(detail.runs[0].status).toBe('cancelled');
  expect(detail.revisions).toEqual([]);
  await page.getByLabel('Message', { exact: true }).fill('Ready for another request');
  await expect(page.getByRole('button', { name: 'Send', exact: false })).toBeEnabled();
});

for (const encoding of ['ASCII', 'CJK']) {
  test(`${encoding} prompts use the UTF-8 byte limit without silently truncating input`, async ({
    page,
    request,
  }) => {
    const workspace = await (
      await request.post('/api/workspaces', { data: { title: `${encoding} input boundary` } })
    ).json();
    await page.goto('/');
    await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
      workspace.title,
    );
    const submitted: string[] = [];
    await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
      submitted.push(route.request().postDataJSON().prompt);
      return route.continue();
    });
    const invalid = encoding === 'ASCII' ? 'a'.repeat(32_769) : '界'.repeat(10_923);
    const valid = encoding === 'ASCII' ? 'a'.repeat(32_768) : `${'界'.repeat(10_922)}ab`;
    expect(new TextEncoder().encode(invalid).length).toBe(32_769);
    expect(new TextEncoder().encode(valid).length).toBe(32_768);
    const composer = page.getByLabel('Message', { exact: true });
    await composer.fill(invalid);
    await expect(composer).toHaveValue(invalid);
    await page.getByRole('button', { name: 'Send', exact: false }).click();
    await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
      '32768 UTF-8 bytes',
    );
    await expect(composer).toHaveValue(invalid);
    expect(submitted).toEqual([]);
    await composer.fill(valid);
    await page.getByRole('button', { name: 'Send', exact: false }).click();
    await expect(page.locator('.message.user pre')).toHaveText(valid);
    await expect(page.locator('.message.assistant')).toHaveCount(1);
    expect(submitted).toEqual([valid]);
  });
}

test('double-clicking New workspace creates only one workspace', async ({ page, request }) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Before double-click' } })
  ).json();
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    workspace.title,
  );
  await openWorkspaces(page);
  const navigation = page.getByRole('navigation', { name: 'Workspaces' });
  const initialCount = await navigation.locator('.workspace').count();
  await closeWorkspaces(page);
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  let creations = 0;
  await page.route('**/api/workspaces', async (route) => {
    if (route.request().method() !== 'POST') return route.continue();
    creations++;
    await gate;
    await route.continue();
  });
  try {
    const button = page.getByRole('button', { name: 'New workspace' });
    await button.dblclick();
    await expect(button).toBeDisabled();
    release();
    await expect(button).toBeEnabled();
    await openWorkspaces(page);
    await expect(navigation.locator('.workspace')).toHaveCount(initialCount + 1);
    expect(creations).toBe(1);
  } finally {
    release();
  }
});

test('preview CSP precedes parser-inserted resources and still allows inline apps', async ({
  page,
  request,
  baseURL,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'CSP boundary' } })
  ).json();
  const run = {
    id: crypto.randomUUID(),
    workspace_id: workspace.id,
    status: 'succeeded',
    error: null,
    provider: 'demo',
    model: 'demo-model',
    output: '',
    stats: emptyStats,
    queue_ms: null,
    duration_ms: null,
    mode: 'chat',
    base_revision_id: null,
  };
  const html = `<!doctype html><html><head>
<script>
document.documentElement.dataset.inline = 'ran';
document.documentElement.dataset.csp = '';
addEventListener('securitypolicyviolation', (event) => {
  document.documentElement.dataset.csp += event.effectiveDirective + ',';
});
</script>
<script src="${baseURL}/csp-probe.js"></script>
<link rel="stylesheet" href="${baseURL}/csp-probe.css">
</head><body><h1>Inline app</h1><img src="${baseURL}/csp-probe.png">
<script>fetch('${baseURL}/csp-probe-fetch').catch(() => {});</script>
</body></html>`;
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    route.fulfill({
      json: {
        workspace,
        messages: [{ role: 'assistant', content: html, run_id: run.id }],
        runs: [run],
        revisions: [
          {
            id: crypto.randomUUID(),
            run_id: run.id,
            execution_id: null,
            number: 1,
            html,
            files: [{ path: 'index.html', content: html }],
          },
        ],
        proposals: [],
      },
    }),
  );
  const resourceRequests: string[] = [];
  await page.route('**/csp-probe*', (route) => {
    resourceRequests.push(route.request().url());
    const script = route.request().url().endsWith('.js');
    return route.fulfill({
      contentType: script ? 'text/javascript' : 'text/plain',
      body: script ? "document.documentElement.dataset.external = 'ran';" : '',
    });
  });
  await page.goto('/');
  await expect(
    page.getByRole('button', { name: 'Choose project version', exact: true }),
  ).toBeVisible();
  await expect(page.locator('iframe')).toHaveCount(0);
  await page.getByRole('button', { name: 'Run preview' }).click();
  const document = page.frameLocator('iframe').locator('html');
  await expect(document).toHaveAttribute('data-inline', 'ran');
  for (const directive of ['script-src-elem', 'style-src-elem', 'img-src', 'connect-src'])
    await expect(document).toHaveAttribute('data-csp', new RegExp(directive));
  await expect(document).not.toHaveAttribute('data-external');
  expect(resourceRequests).toEqual([]);
});

test('unknown API routes never fall through to the production SPA', async ({ request }) => {
  for (const path of ['/api', '/api/', '/api/not-a-route']) {
    const response = await request.get(path);
    expect(response.status()).toBe(404);
    expect(response.headers()['content-type']).toContain('application/json');
    expect(await response.json()).toEqual({ error: 'API route not found.' });
  }
});

for (const failing of ['status', 'workspaces']) {
  test(`initial ${failing} failure does not block the other read and retries automatically`, async ({
    page,
    request,
  }) => {
    const workspace = await (
      await request.post('/api/workspaces', { data: { title: `${failing} recovery` } })
    ).json();
    let reads = 0;
    let release!: () => void;
    const retryGate = new Promise<void>((resolve) => (release = resolve));
    await page.route(`**/api/${failing}`, async (route) => {
      if (++reads === 1)
        return route.fulfill({ status: 503, json: { error: `Temporary ${failing} failure` } });
      await retryGate;
      return route.continue();
    });
    try {
      await page.goto('/');
      await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
        `Temporary ${failing} failure`,
      );
      if (failing === 'status')
        await expect(
          page.getByRole('button', { name: 'Choose workspace', exact: true }),
        ).toContainText(workspace.title);
      else {
        await openWorkspaces(page);
        await expect(page.getByText('No model or GPU required', { exact: true })).toBeVisible();
        await closeWorkspaces(page);
      }
      release();
      await expect(
        page.getByRole('button', { name: 'Choose workspace', exact: true }),
      ).toContainText(workspace.title);
      {
        await openWorkspaces(page);
        await expect(page.getByText('No model or GPU required', { exact: true })).toBeVisible();
        await closeWorkspaces(page);
      }
      await expect.poll(() => reads).toBe(2);
    } finally {
      release();
    }
  });
}

test('a failed initial workspace detail read retries without resubmitting', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Detail read recovery' } })
  ).json();
  let reads = 0;
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => {
    if (++reads === 1)
      return route.fulfill({ status: 503, json: { error: 'Temporary initial detail failure' } });
    return route.continue();
  });
  await page.goto('/');
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'Temporary initial detail failure',
  );
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    workspace.title,
  );
  expect(reads).toBe(2);
});

test('selecting the already selected workspace does not abort its initial read', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Keep the current read' } })
  ).json();
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  let release!: () => void;
  let arrived!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  const arrival = new Promise<void>((resolve) => (arrived = resolve));
  await page.route(`**/api/workspaces/${workspace.id}`, async (route) => {
    arrived();
    await gate;
    await route.fulfill({ json: detail }).catch(() => {}); // The regression aborts this read.
  });
  try {
    await page.goto('/');
    await arrival;
    await openWorkspaces(page);
    await page.getByRole('button', { name: workspace.title, exact: false }).click();
    release();
    await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
      workspace.title,
    );
  } finally {
    release();
  }
});

test('Vite proxy preserves local browser access and rejects untrusted hosts', async ({
  page,
  request,
  baseURL,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Vite authority check' } })
  ).json();
  const viteURL = `http://127.0.0.1:${new URL(baseURL!).port === '14328' ? 14329 : 14319}`;
  await page.goto(viteURL);
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    workspace.title,
  );
  await page.getByLabel('Message', { exact: true }).fill('Hello through the local proxy');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.locator('.message.assistant')).toHaveCount(1);
  const blocked = await request.get(`${viteURL}/api/status`, {
    headers: { Host: `rebind.invalid:${new URL(viteURL).port}` },
  });
  expect(blocked.status()).toBe(403);
  expect(await blocked.text()).toContain('Blocked request');
});
