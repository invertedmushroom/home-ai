import { openWorkspaces, selectProposal, expectProposal, selectRevision } from './helpers';
import { test, expect } from '@playwright/test';
import type { APIRequestContext } from '@playwright/test';

async function propose(request: APIRequestContext, workspace: string, base: string | null) {
  const response = await request.post(`/api/workspaces/${workspace}/runs`, {
    data: {
      prompt: 'Propose a counter project',
      mode: 'files',
      backend: 'demo',
      base_revision: { id: base },
    },
  });
  expect(response.status()).toBe(202);
  const run = await response.json();
  await expect
    .poll(async () => (await (await request.get(`/api/runs/${run.id}`)).json()).status)
    .toBe('succeeded');
  return (await (await request.get(`/api/workspaces/${workspace}`)).json()).proposals.at(-1);
}

test('an older API rejection stays visible beside the action and retry settles once', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Old API rejection' } })
  ).json();
  const proposal = await propose(request, workspace.id, null);
  await page.goto('/');
  await selectProposal(page, 'Proposal 1 · pending');
  const action = page.getByRole('button', { name: 'Reject changes', exact: true });
  await action.scrollIntoViewIfNeeded();
  await page.route(`**/api/proposals/${proposal.id}/reject`, (route) =>
    route.fulfill({ status: 404, json: { error: 'API route not found.' } }),
  );
  await action.click();
  const error = page.locator('.proposal-review .run-error[role=alert]');
  await expect(error).toContainText('Restart the Rust server');
  await expect(error).toBeInViewport();
  await expectProposal(page, 'Proposal 1 · pending');
  await page.unroute(`**/api/proposals/${proposal.id}/reject`);
  await action.click();
  await expectProposal(page, 'Proposal 1 · rejected');
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  expect(detail.proposals[0].status).toBe('rejected');
  expect(detail.revisions).toHaveLength(0);
});

test('review multi-file diffs, preview before apply, and preserve immutable revision history', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Project review' } })
  ).json();
  await page.goto('/');
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(true);
  await page.getByRole('textbox', { name: 'Message' }).fill('Propose a counter project');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expectProposal(page, 'Proposal 1 · pending');
  const proposal = (await (await request.get(`/api/workspaces/${workspace.id}`)).json())
    .proposals[0];
  await expect(page.locator('iframe')).toHaveCount(0);
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(0);
  await expect(page.locator('.project-state strong')).toHaveText('Proposal 1 · pending');
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toBeVisible();
  await selectProposal(page, 'Proposal 1 · pending');
  await expect(page.getByLabel('File diff')).toContainText('Proposed changes against base');
  await page.getByLabel('File list').getByRole('button', { name: 'app.js', exact: true }).click();
  await expect(page.getByLabel('File diff')).toContainText('const step = 1;');
  // Preview opens the project even while reviewing JavaScript.
  await page.getByRole('button', { name: 'Run preview' }).click();
  const frame = page.frameLocator('iframe');
  await expect(frame.getByRole('heading', { name: 'Project counter' })).toBeVisible();
  await frame.getByRole('button', { name: 'Add one' }).click();
  await expect(frame.locator('#count')).toHaveText('1');
  await expect(frame.locator('body')).toHaveCSS('background-color', 'rgb(238, 243, 239)');
  const iframe = page.frames().find((frame) => frame !== page.mainFrame())!;
  expect(
    await iframe.evaluate(() => {
      try {
        void parent.document.body;
        return false;
      } catch {
        return true;
      }
    }),
  ).toBe(true);
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Choose project version', exact: true }),
  ).toHaveAttribute('value', /.+/);
  await expectProposal(page, 'Proposal 1 · applied');
  await expect(page.locator('iframe')).toHaveCount(1);
  await expect(frame.locator('#count')).toHaveText('1');
  const first = await (await request.post(`/api/proposals/${proposal.id}/apply`)).json();
  await page.reload();
  await expect(page.getByLabel('File list').getByRole('button')).toHaveCount(3);
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(true);
  await page.getByRole('textbox', { name: 'Message' }).fill('Change the counter step to two');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expectProposal(page, 'Proposal 2 · pending');
  const second = (await (await request.get(`/api/workspaces/${workspace.id}`)).json()).proposals[1];
  await selectProposal(page, 'Proposal 2 · pending');
  await expect(page.getByLabel('File list').getByRole('button')).toHaveCount(3);
  await expect(page.getByLabel('File list').getByRole('button')).toHaveCount(3);
  await expect(page.getByLabel('File list').locator('.file-changed')).toHaveText('app.js');
  await expect(page.getByLabel('File diff')).toContainText('const step = 1;');
  await expect(page.getByLabel('File diff')).toContainText('const step = 2;');
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(2);
  await selectRevision(page, { label: 'Revision 1' });
  await page.getByLabel('File list').getByRole('button', { name: 'app.js', exact: true }).click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('const step = 1;');
  await selectRevision(page, { label: 'Revision 2' });
  await page.getByLabel('File list').getByRole('button', { name: 'app.js', exact: true }).click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('const step = 2;');
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  expect(detail.proposals.find((item: { id: string }) => item.id === second.id).status).toBe(
    'applied',
  );
  expect(detail.revisions[0]).toEqual(first);
});

test('a stale proposal remains reviewable and can be rejected without another generation', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Stale project' } })
  ).json();
  await propose(request, workspace.id, null);
  const other = await propose(request, workspace.id, null);
  await page.goto('/');
  await selectProposal(page, 'Proposal 1 · pending');
  expect((await request.post(`/api/proposals/${other.id}/apply`)).ok()).toBe(true);
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'base revision is stale',
  );
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Reject changes', exact: true }).click();
  await expectProposal(page, 'Proposal 1 · rejected');
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  expect(detail.revisions).toHaveLength(1);
  expect(detail.runs).toHaveLength(2);
});

test('rejection feedback survives a lost response and failed refresh without changing files or starting generation', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Rejection feedback' } })
  ).json();
  const first = await propose(request, workspace.id, null);
  const accepted = await (await request.post(`/api/proposals/${first.id}/apply`)).json();
  const second = await propose(request, workspace.id, accepted.id);
  await page.goto('/');
  await selectProposal(page, 'Proposal 2 · pending');
  const reason = 'Keep the increment at one; improve the styling instead. 界🙂';
  if ((await page.locator('.rejection-feedback').getAttribute('open')) === null)
    await page.getByText('Add feedback (optional)', { exact: true }).click();
  await page.getByLabel('Rejection reason (optional)').fill(reason);
  let failReads = false;
  let attempts = 0;
  let generations = 0;
  page.on('request', (request) => {
    if (request.method() === 'POST' && request.url().endsWith('/runs')) generations++;
  });
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    failReads
      ? route.fulfill({ status: 503, json: { error: 'Temporary feedback refresh failure' } })
      : route.continue(),
  );
  await page.route(`**/api/proposals/${second.id}/reject`, async (route) => {
    attempts++;
    expect(route.request().postDataJSON()).toEqual({ reason });
    const response = await route.fetch();
    if (attempts === 1) return route.abort('failed');
    failReads = true;
    return route.fulfill({ response });
  });
  await page.getByRole('button', { name: 'Reject changes', exact: true }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toBeVisible();
  await expect(page.getByLabel('Rejection reason (optional)')).toHaveValue(reason);
  await page.getByRole('button', { name: 'Reject changes', exact: true }).click();
  await expectProposal(page, 'Proposal 2 · rejected');
  const outcome = page
    .getByLabel('Conversation')
    .getByLabel('Proposal outcome')
    .filter({ hasText: 'Rejected' });
  await expect(outcome).toContainText(reason);
  await expect(outcome).toContainText('accepted files unchanged');
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'Temporary feedback refresh failure',
  );
  await page.getByRole('checkbox', { name: /^Change project files/ }).setChecked(false);
  await page.getByText('Next request context', { exact: true }).click();
  await expect(page.locator('.context-summary')).toContainText(`Proposal 2: rejected · ${reason}`);
  await expect(page.locator('.context-summary')).toContainText(
    'The accepted file snapshot is not included in Conversation mode',
  );
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  expect(detail.revisions).toEqual([accepted]);
  expect(detail.runs).toHaveLength(2);
  expect(detail.messages).toHaveLength(4);
  expect(detail.proposals[1].rejection_reason).toBe(reason);
  expect(generations).toBe(0);
  expect(attempts).toBe(2);
  failReads = false;
  await page.reload();
  await expect(
    page.getByLabel('Conversation').getByLabel('Proposal outcome').filter({ hasText: 'Rejected' }),
  ).toContainText(reason);
  await selectProposal(page, 'Proposal 2 · rejected');
  await page.getByText('Model response', { exact: true }).last().click();
  await expect(
    page
      .locator('.message.assistant details')
      .filter({ has: page.getByText('Model response', { exact: true }) })
      .last(),
  ).toContainText('const step = 2;');
});

test('rejection validates UTF-8 feedback and response evidence before publishing a decision', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Feedback boundaries' } })
  ).json();
  const proposal = await propose(request, workspace.id, null);
  await page.goto('/');
  await selectProposal(page, 'Proposal 1 · pending');
  const oversized = '界'.repeat(1366);
  if ((await page.locator('.rejection-feedback').getAttribute('open')) === null)
    await page.getByText('Add feedback (optional)', { exact: true }).click();
  await page.getByLabel('Rejection reason (optional)').fill(oversized);
  await page.getByRole('button', { name: 'Reject changes', exact: true }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    '4096 UTF-8 bytes',
  );
  await expect(page.getByLabel('Rejection reason (optional)')).toHaveValue(oversized);
  let invalid = true;
  let attempts = 0;
  await page.route(`**/api/proposals/${proposal.id}/reject`, (route) => {
    attempts++;
    return invalid
      ? route.fulfill({ json: { ...proposal, status: 'rejected', rejection_reason: 42 } })
      : route.continue();
  });
  if ((await page.locator('.rejection-feedback').getAttribute('open')) === null)
    await page.getByText('Add feedback (optional)', { exact: true }).click();
  await page.getByLabel('Rejection reason (optional)').fill('Use a Node generator');
  await page.getByRole('button', { name: 'Reject changes', exact: true }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'incompatible shape',
  );
  await expectProposal(page, 'Proposal 1 · pending');
  await expect(page.getByLabel('Conversation').getByLabel('Proposal outcome')).toContainText(
    'Pending review',
  );
  invalid = false;
  await page.getByRole('button', { name: 'Reject changes', exact: true }).click();
  await expect(page.getByLabel('Conversation').getByLabel('Proposal outcome')).toContainText(
    'Rejected',
  );
  expect(attempts).toBe(2);
});

test('a late rejection receipt cannot publish feedback in another workspace', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Late rejection' } })
  ).json();
  const proposal = await propose(request, workspace.id, null);
  await page.goto('/');
  await selectProposal(page, 'Proposal 1 · pending');
  if ((await page.locator('.rejection-feedback').getAttribute('open')) === null)
    await page.getByText('Add feedback (optional)', { exact: true }).click();
  await page.getByLabel('Rejection reason (optional)').fill('Use a Node generator');
  let release!: () => void;
  let observed!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const started = new Promise<void>((resolve) => {
    observed = resolve;
  });
  await page.route(`**/api/proposals/${proposal.id}/reject`, async (route) => {
    const response = await route.fetch();
    observed();
    await held;
    await route.fulfill({ response }).catch(() => {});
  });
  await page.getByRole('button', { name: 'Reject changes', exact: true }).click();
  await started;
  await page.getByRole('button', { name: 'New workspace' }).click();
  release();
  await expect(page.getByLabel('Proposal outcome')).toHaveCount(0);
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toHaveCount(0);
  await openWorkspaces(page);
  await page.getByRole('button', { name: 'Late rejection' }).click();
  await expect(page.getByLabel('Conversation').getByLabel('Proposal outcome')).toContainText(
    'Use a Node generator',
  );
});

test('an accepted apply receipt keeps the new files visible after a failed refresh', async ({
  page,
  request,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Apply recovery' } })
  ).json();
  await propose(request, workspace.id, null);
  await page.goto('/');
  await selectProposal(page, 'Proposal 1 · pending');
  let fail = true;
  await page.route(`**/api/workspaces/${workspace.id}`, (route) =>
    fail
      ? route.fulfill({ status: 503, json: { error: 'Temporary refresh failure' } })
      : route.continue(),
  );
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'Temporary refresh failure',
  );
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(1);
  await expect(page.getByLabel('File list').getByRole('button')).toHaveCount(3);
  await expectProposal(page, 'Proposal 1 · applied');
  fail = false;
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toHaveCount(0);
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(1);
});

test('a delayed apply receipt cannot replace another workspace', async ({ page, request }) => {
  const title = `Delayed apply ${Date.now()}`;
  const workspace = await (await request.post('/api/workspaces', { data: { title } })).json();
  const proposal = await propose(request, workspace.id, null);
  await page.goto('/');
  await selectProposal(page, 'Proposal 1 · pending');
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let started!: () => void;
  const applying = new Promise<void>((resolve) => {
    started = resolve;
  });
  await page.route(`**/api/proposals/${proposal.id}/apply`, async (route) => {
    const response = await route.fetch();
    started();
    await held;
    await route.fulfill({ response }).catch(() => {});
  });
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await applying;
  await page.getByRole('button', { name: 'New workspace' }).click();
  await expect(
    page.getByRole('button', { name: 'Proposal 1 · pending', exact: true, includeHidden: true }),
  ).toHaveCount(0);
  release();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(0);
  await expect(page.getByLabel('File list').getByRole('button')).toHaveCount(0);
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toHaveCount(0);
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  expect(detail.revisions).toHaveLength(1);
  await openWorkspaces(page);
  await page.getByRole('button', { name: title }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(1);
});

test('multi-file previews inline local assets while blocking ordinary external resources and fetches', async ({
  page,
  request,
  baseURL,
}) => {
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Project CSP' } })
  ).json();
  const proposal = await propose(request, workspace.id, null);
  await request.post(`/api/proposals/${proposal.id}/apply`);
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  const html = `<!doctype html><html><head><link rel="stylesheet" href="style.css">
    <script src="${baseURL}/project-probe.js"></script><link rel="stylesheet" href="${baseURL}/project-probe.css">
    </head><body><h1>Safe project</h1><img src="${baseURL}/project-probe.png"><script src="app.js"></script></body></html>`;
  detail.proposals = [];
  detail.revisions[0].html = html;
  detail.revisions[0].files = [
    { path: 'index.html', content: html },
    { path: 'style.css', content: 'h1{color:rgb(12, 34, 56)}' },
    {
      path: 'app.js',
      content: `window.literal = "</script>"; document.documentElement.dataset.ran='yes'; fetch('${baseURL}/project-probe-fetch').catch(()=>{});`,
    },
  ];
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  const resources: string[] = [];
  await page.route('**/project-probe*', (route) => {
    resources.push(route.request().url());
    return route.fulfill({ body: '' });
  });
  await page.goto('/');
  await page
    .getByLabel('File list')
    .getByRole('button', { name: 'index.html', exact: true })
    .click();
  await page.getByRole('button', { name: 'Run preview' }).click();
  const frame = page.frameLocator('iframe');
  await expect(frame.getByRole('heading', { name: 'Safe project' })).toHaveCSS(
    'color',
    'rgb(12, 34, 56)',
  );
  await expect(frame.locator('html')).toHaveAttribute('data-ran', 'yes');
  const child = page.frames().find((frame) => frame !== page.mainFrame())!;
  expect(await child.evaluate('window.literal')).toBe('</script>');
  expect(resources).toEqual([]);
});

test('branching a workspace and deleting chat turns keep workspace files and unapplied proposals intact', async ({
  page,
  request,
}) => {
  const title = `Branch & Delete ${Date.now()}`;
  const workspace = await (await request.post('/api/workspaces', { data: { title } })).json();
  const first = await propose(request, workspace.id, null);
  const accepted = await (await request.post(`/api/proposals/${first.id}/apply`)).json();
  await propose(request, workspace.id, accepted.id);
  const chatRes = await request.post(`/api/workspaces/${workspace.id}/runs`, {
    data: {
      prompt: 'Third turn ordinary chat note',
      mode: 'chat',
      backend: 'demo',
    },
  });
  expect(chatRes.status()).toBe(202);
  const chatRun = await chatRes.json();
  await expect
    .poll(async () => (await (await request.get(`/api/runs/${chatRun.id}`)).json()).status)
    .toBe('succeeded');

  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    title,
  );
  await expect(page.getByLabel('Conversation').locator('.message')).toHaveCount(6);
  await expectProposal(page, 'Proposal 2 · pending');
  await expect(page.getByLabel('File list').getByRole('button')).toHaveCount(3);

  // Branch from the first turn ("Branch from here" on the first assistant turn).
  await page
    .getByLabel('Conversation')
    .getByRole('button', { name: 'Branch from here', exact: true })
    .first()
    .click();

  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toContainText(
    `${title} (branch)`,
  );
  // The branch keeps the first conversation turn. Preserved proposals remain in Revisions.
  await expect(page.getByLabel('Conversation').locator('.message.user')).toHaveCount(1);
  await expect(page.getByLabel('Conversation').locator('.project-activity')).toHaveCount(0);
  await expect(page.getByLabel('File list').getByRole('button')).toHaveCount(3);
  await expectProposal(page, 'Proposal 2 · pending');
  await page.getByText('Next request context', { exact: true }).click();
  await expect(page.locator('.context-summary')).toContainText('Proposal 2: pending');
  await page.getByText('Next request context', { exact: true }).click();

  // Apply the unapplied proposal in the branched workspace.
  await selectProposal(page, 'Proposal 2 · pending');
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(2);

  // Switch back to the original workspace; its unapplied Proposal 2 is still pending.
  await openWorkspaces(page);
  await page.getByRole('button', { name: `▧ ${title}`, exact: true }).click();
  await expect(page.getByLabel('Conversation').locator('.message.user')).toHaveCount(3);
  await expectProposal(page, 'Proposal 2 · pending');

  // Delete the second turn (which proposed Proposal 2) and the third turn.
  await page
    .getByLabel('Conversation')
    .getByRole('button', { name: 'Delete turn', exact: true })
    .nth(1)
    .click();
  await expect(page.getByLabel('Conversation').locator('.message.user')).toHaveCount(2);

  await page
    .getByLabel('Conversation')
    .getByRole('button', { name: 'Delete turn', exact: true })
    .nth(1)
    .click();
  await expect(page.getByLabel('Conversation').locator('.message.user')).toHaveCount(1);
  await expect(page.getByLabel('Conversation').locator('.project-activity')).toHaveCount(0);

  // Even after deleting the turn that created Proposal 2, the accepted workspace files
  // AND the unapplied Proposal 2 remain visible and reviewable!
  await expect(page.getByLabel('File list').getByRole('button')).toHaveCount(3);
  await expectProposal(page, 'Proposal 2 · pending');
  await page.getByText('Next request context', { exact: true }).click();
  await expect(page.locator('.context-summary')).toContainText('Proposal 2: pending');
  await page.getByText('Next request context', { exact: true }).click();

  await selectProposal(page, 'Proposal 2 · pending');
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(2);
});
