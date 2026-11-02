import {
  openExecution,
  openWorkspaces,
  selectProposal,
  expectProposal,
  selectRevision,
  openRevisions,
} from './helpers';
import { test, expect } from '@playwright/test';
import type { Page, APIRequestContext } from '@playwright/test';

const counter =
  '<!doctype html><html><body><h1>Editor counter</h1><output id="count">0</output><button id="add">Increment</button><script>let n=0;document.querySelector("#add").onclick=()=>{document.querySelector("#count").textContent=String(++n);};</script></body></html>';

async function workspace(request: APIRequestContext, title: string) {
  return (await request.post('/api/workspaces', { data: { title } })).json();
}
async function typeFile(page: Page, content: string) {
  const editor = page.getByRole('textbox', { name: 'File editor', exact: true });
  await expect(editor).toBeVisible();
  await editor.focus();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.insertText(content);
}
async function addFile(page: Page, path: string, content: string) {
  await page.getByLabel('New file path').fill(path);
  await page.getByRole('button', { name: 'Add file', exact: true }).click();
  await typeFile(page, content);
}

test('Monaco loads locally on demand and manual files pass through review, apply, preview and reload', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Manual counter');
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const requests: string[] = [];
  const workers: string[] = [];
  page.on('request', (request) => requests.push(request.url()));
  page.on('worker', (worker) => workers.push(worker.url()));
  await page.goto('/');
  await expect(
    page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }),
  ).toBeEnabled();
  expect(requests.some((url) => /MonacoEditor-|editor\.api-|\.worker-/.test(url))).toBe(false);
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(page, 'index.html', counter);
  await page.getByRole('button', { name: 'Diff', exact: true }).click();
  await expect(page.getByLabel('Code comparison')).toContainText('Editor counter');
  await expect(page.locator('.monaco-diff-editor')).not.toHaveClass(/side-by-side/);
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expectProposal(page, 'Proposal 1 · pending');
  await expect(page.getByLabel('File diff')).toContainText('Editor counter');
  await expect(page.getByText(/Source: manual edit/)).toBeVisible();
  let detail = await (await request.get(`/api/workspaces/${project.id}`)).json();
  expect(detail.revisions).toHaveLength(0);
  expect(detail.runs).toHaveLength(0);
  expect(detail.messages).toHaveLength(0);
  expect(detail.proposals[0].changes).toEqual([
    { op: 'write', path: 'index.html', content: counter },
  ]);
  expect(detail.proposals[0].edit_id).toMatch(/^[0-9a-f-]{36}$/);
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.locator('.version-option[data-kind=revision]')).toHaveCount(1);
  await page.getByRole('button', { name: /^Run preview/ }).click();
  const frame = page.frameLocator('iframe');
  await frame.getByRole('button', { name: 'Increment' }).click();
  await expect(frame.locator('#count')).toHaveText('1');
  await page.reload();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Editor counter');
  await expect(page.getByLabel('Conversation').locator('.project-activity')).toHaveCount(0);
  await page.getByText('Next request context', { exact: true }).click();
  await expect(page.locator('.context-summary')).toContainText('index.html');
  await expect(page.locator('.context-summary')).toContainText('applied');
  detail = await (await request.get(`/api/workspaces/${project.id}`)).json();
  expect(detail.revisions[0].edit_id).toBe(detail.proposals[0].edit_id);
  expect(detail.revisions[0].run_id).toBeNull();
  expect(detail.revisions[0].execution_id).toBeNull();
  expect(requests.every((url) => new URL(url).hostname === '127.0.0.1')).toBe(true);
  await expect.poll(() => workers.length).toBeGreaterThan(0);
  expect(workers.every((url) => new URL(url).hostname === '127.0.0.1')).toBe(true);
  expect(errors).toEqual([]);
});

test('draft text survives file navigation and workspace switches, and discarded edits never become accepted files', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Draft A');
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(page, 'a.txt', 'Draft A text');
  await addFile(page, 'b.txt', 'Second file');
  await page
    .getByRole('navigation', { name: 'Draft files' })
    .getByRole('button', { name: 'a.txt' })
    .click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Draft A text');
  const created = page.waitForResponse(
    (response) =>
      response.request().method() === 'POST' && response.url().endsWith('/api/workspaces'),
  );
  await page.getByRole('button', { name: 'New workspace' }).click();
  const otherWorkspace = await (await created).json();
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(page, 'b.txt', 'Draft B text');
  await openWorkspaces(page);
  await page.getByRole('button', { name: 'Draft A' }).click();
  await expect(page.getByLabel('Editor draft')).toBeVisible();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Draft A text');
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expectProposal(page, 'Proposal 1 · pending');
  const detail = await (await request.get(`/api/workspaces/${project.id}`)).json();
  expect(detail.proposals[0].changes.map((change: { path: string }) => change.path)).toEqual([
    'a.txt',
    'b.txt',
  ]);
  await openWorkspaces(page);
  await page.getByRole('button', { name: otherWorkspace.title }).click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Draft B text');
  await page.getByRole('button', { name: 'Discard draft', exact: true }).click();
  await expect(page.getByLabel('Editor draft')).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Proposal 1 · pending', exact: true, includeHidden: true }),
  ).toHaveCount(0);
});

test('manual save retries the same input after response loss and retains the accepted receipt through a failed refresh', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Save recovery');
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(page, 'index.html', counter);
  const inputs: unknown[] = [];
  let failReads = false;
  await page.route(`**/api/workspaces/${project.id}`, (route) =>
    failReads
      ? route.fulfill({ status: 503, json: { error: 'Manual refresh temporarily failed' } })
      : route.continue(),
  );
  await page.route(`**/api/workspaces/${project.id}/proposals`, async (route) => {
    inputs.push(route.request().postDataJSON());
    const response = await route.fetch();
    if (inputs.length === 1) return route.abort('failed');
    failReads = true;
    return route.fulfill({ response });
  });
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Retry save', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Discard draft', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Retry save', exact: true }).click();
  await expect(page.getByLabel('File diff')).toContainText('Editor counter');
  await expect(page.locator('.error[role=alert], .run-error[role=alert]')).toContainText(
    'Manual refresh temporarily failed',
  );
  expect(inputs).toHaveLength(2);
  expect(inputs[1]).toEqual(inputs[0]);
  const detail = await (await request.get(`/api/workspaces/${project.id}`)).json();
  expect(detail.proposals).toHaveLength(1);
  expect(detail.revisions).toHaveLength(0);
  failReads = false;
  await page.reload();
  await expectProposal(page, 'Proposal 1 · pending');
});

test('a stale editor draft remains available while the saved revision can be inspected', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Stale editor');
  const saved = await (
    await request.post(`/api/workspaces/${project.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: null },
        changes: [{ op: 'write', path: 'a.txt', content: 'Original' }],
      },
    })
  ).json();
  const revision = await (await request.post(`/api/proposals/${saved.id}/apply`)).json();
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await typeFile(page, 'My unsaved edit');
  const other = await (
    await request.post(`/api/workspaces/${project.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: revision.id },
        changes: [{ op: 'write', path: 'a.txt', content: 'Changed elsewhere' }],
      },
    })
  ).json();
  const advanced = await (await request.post(`/api/proposals/${other.id}/apply`)).json();
  await page.getByRole('button', { name: 'Refresh files', exact: true }).click();
  await expect(page.getByText(/The accepted revision changed/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Export draft', exact: true })).toBeEnabled();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('My unsaved edit');
  let submissions = 0;
  await page.route(`**/api/workspaces/${project.id}/proposals`, (route) => {
    submissions++;
    return route.fallback();
  });
  await page.getByRole('textbox', { name: 'File editor', exact: true }).focus();
  await page.keyboard.press('ControlOrMeta+s');
  expect(submissions).toBe(0);
  await selectRevision(page, advanced.id);
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Changed elsewhere');
  await page.getByRole('button', { name: 'Resume draft', exact: true }).click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('My unsaved edit');
});

test('Revisions preserves a draft, deleted base files restore exactly, and save keeps the selected file', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Editor continuity');
  const initial = await (
    await request.post(`/api/workspaces/${project.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: null },
        changes: [
          { op: 'write', path: 'a.txt', content: 'Original A' },
          { op: 'write', path: 'b.txt', content: 'Original B' },
        ],
      },
    })
  ).json();
  const revision = await (await request.post(`/api/proposals/${initial.id}/apply`)).json();
  await page.goto('/');
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const files = page.getByRole('navigation', { name: 'Draft files', exact: true });
  await files.getByRole('button', { name: 'b.txt' }).click();
  await typeFile(page, 'Intermediate B');
  await page.getByRole('button', { name: 'Delete file', exact: true }).click();
  await expect(files.getByRole('button', { name: 'b.txt · deleted', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Restore file', exact: true }).click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Original B');
  await typeFile(page, 'Edited B');
  await selectRevision(page, revision.id);
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Original B');
  await openRevisions(page);
  const revisions = page.getByRole('region', { name: 'Revisions window', exact: true });
  await expect(
    revisions
      .getByRole('region', { name: 'Working versions' })
      .getByRole('button', { name: 'Unsaved draft', exact: true }),
  ).toBeVisible();
  await page.screenshot({ path: 'logs/editor-revisions.png', fullPage: true });
  await revisions.getByRole('button', { name: 'Unsaved draft', exact: true }).click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Edited B');
  await files.getByRole('button', { name: 'a.txt' }).click();
  await typeFile(page, 'Edited A');
  await files.getByRole('button', { name: 'b.txt' }).click();
  let saves = 0;
  await page.route(`**/api/workspaces/${project.id}/proposals`, (route) => {
    saves++;
    return route.fallback();
  });
  await page.getByRole('textbox', { name: 'File editor', exact: true }).focus();
  await page.keyboard.press('ControlOrMeta+s');
  await expect(page.getByLabel('Editor draft')).toHaveCount(0);
  await expect(
    page.getByLabel('File list').getByRole('button', { name: 'b.txt', exact: true }),
  ).toHaveAttribute('aria-current', 'true');
  await expect(page.getByLabel('File diff')).toContainText('Edited B');
  expect(saves).toBe(1);
  const detail = await (await request.get(`/api/workspaces/${project.id}`)).json();
  expect(detail.revisions).toHaveLength(1);
  expect(detail.proposals.at(-1).changes).toEqual([
    { op: 'write', path: 'a.txt', content: 'Edited A' },
    { op: 'write', path: 'b.txt', content: 'Edited B' },
  ]);
  await page.screenshot({ path: 'logs/editor-pending-review.png', fullPage: true });
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Edited B');
  await page.screenshot({ path: 'logs/editor-compact-review.png', fullPage: true });
});

test('preview status names its immutable capture and Update preview captures the accepted head while browsing history', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Explicit preview update');
  const initial = await (
    await request.post(`/api/workspaces/${project.id}/proposals`, {
      data: {
        id: crypto.randomUUID(),
        base_revision: { id: null },
        changes: [{ op: 'write', path: 'index.html', content: counter }],
      },
    })
  ).json();
  const revision = await (await request.post(`/api/proposals/${initial.id}/apply`)).json();
  await page.goto('/');
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  const frame = page.frameLocator('iframe');
  await frame.getByRole('button', { name: 'Increment', exact: true }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await typeFile(page, counter.replace('Editor counter', 'Updated counter'));
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await expect(page.getByLabel('Preview capture')).toContainText(
    'Previewing revision 1 · Current is revision 2',
  );
  await expect(frame.getByRole('heading', { name: 'Editor counter', exact: true })).toBeVisible();
  await expect(frame.locator('#count')).toHaveText('1');
  await selectRevision(page, revision.id);
  await expect(
    page.getByRole('button', { name: 'Choose project version', exact: true }),
  ).toContainText('Revision 1');
  await page.getByRole('button', { name: 'Update preview', exact: true }).click();
  await expect(frame.getByRole('heading', { name: 'Updated counter', exact: true })).toBeVisible();
  await expect(frame.locator('#count')).toHaveText('0');
  await expect(page.getByLabel('Preview capture')).toHaveText('Previewing revision 2');
  await expect(
    page.getByRole('button', { name: 'Choose project version', exact: true }),
  ).toContainText('Revision 1');
  await page.screenshot({ path: 'logs/editor-preview-update.png', fullPage: true });
});

test('a failed Monaco download leaves an editable plain-text recovery surface', async ({
  page,
  request,
}) => {
  await workspace(request, 'Editor recovery');
  await page.route(/\/assets\/MonacoEditor-[^/]+\.js$/, (route) => route.abort('failed'));
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await page.getByLabel('New file path').fill('notes.txt');
  await page.getByRole('button', { name: 'Add file', exact: true }).click();
  await expect(page.getByRole('alert').filter({ hasText: 'Monaco could not load' })).toBeVisible();
  await page.getByRole('textbox', { name: 'File editor', exact: true }).fill('Recovered draft');
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expectProposal(page, 'Proposal 1 · pending');
});

test('editing a pending model proposal creates a separate manual proposal and execution uses saved files', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Revise proposal');
  const run = await (
    await request.post(`/api/workspaces/${project.id}/runs`, {
      data: {
        prompt: 'A counter project',
        mode: 'files',
        backend: 'demo',
        base_revision: { id: null },
      },
    })
  ).json();
  await expect
    .poll(async () => (await (await request.get(`/api/runs/${run.id}`)).json()).status)
    .toBe('succeeded');
  const original = (await (await request.get(`/api/workspaces/${project.id}`)).json()).proposals[0];
  await page.route('**/api/worker', (route) =>
    route.fulfill({ json: { enabled: true, ready: true, message: 'Ready' } }),
  );
  await page.goto('/');
  await selectProposal(page, 'Proposal 1 · pending');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await page
    .getByRole('navigation', { name: 'Draft files' })
    .getByRole('button', { name: 'app.js' })
    .click();
  await typeFile(page, "console.log('Manual entry');");
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toHaveCount(0);
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Close Runs', exact: true }).click();
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expectProposal(page, 'Proposal 2 · pending');
  let detail = await (await request.get(`/api/workspaces/${project.id}`)).json();
  expect(detail.proposals[0]).toEqual(original);
  expect(detail.proposals[1].run_id).toBeNull();
  expect(detail.proposals[1].edit_id).toBeTruthy();
  expect(detail.runs).toHaveLength(1);
  await page.getByRole('button', { name: 'Apply changes', exact: true }).click();
  await openExecution(page);
  await expect(page.getByRole('button', { name: 'Check syntax', exact: true })).toBeEnabled();
  detail = await (await request.get(`/api/workspaces/${project.id}`)).json();
  expect(
    detail.revisions[0].files.find((file: { path: string }) => file.path === 'app.js').content,
  ).toBe("console.log('Manual entry');");
});

test('a manual save interrupted by workspace switching is recovered from its durable receipt', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Interrupted save');
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(page, 'notes.txt', 'Keep this draft');
  let release!: () => void;
  let observed!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const saved = new Promise<void>((resolve) => {
    observed = resolve;
  });
  let submissions = 0;
  await page.route(`**/api/workspaces/${project.id}/proposals`, async (route) => {
    submissions++;
    const response = await route.fetch();
    observed();
    await held;
    await route.fulfill({ response }).catch(() => {});
  });
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await saved;
  await page.getByRole('button', { name: 'New workspace' }).click();
  release();
  await expect(page.getByLabel('Editor draft')).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Proposal 1 · pending', exact: true, includeHidden: true }),
  ).toHaveCount(0);
  await openWorkspaces(page);
  await page.getByRole('button', { name: 'Interrupted save' }).click();
  await expect(page.getByLabel('Editor draft')).toHaveCount(0);
  await selectProposal(page, 'Proposal 1 · pending');
  await expect(page.getByLabel('File diff')).toContainText('Keep this draft');
  expect(submissions).toBe(1);
});

test('unsaved edits warn before reload, and discarding clears the warning', async ({
  page,
  request,
}) => {
  await workspace(request, 'Reload warning');
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(page, 'notes.txt', 'Unsaved content');
  const dialogSeen = new Promise<void>((resolve) => {
    page.once('dialog', async (dialog) => {
      expect(dialog.type()).toBe('beforeunload');
      await dialog.dismiss();
      resolve();
    });
  });
  const navigation = page.evaluate(() => window.location.reload());
  await dialogSeen;
  await navigation;
  await expect(page.getByLabel('Code source', { exact: true })).toContainText('Unsaved content');
  await page.getByRole('button', { name: 'Discard draft', exact: true }).click();
  await page.reload();
  await expect(page.getByLabel('Editor draft')).toHaveCount(0);
});

test('a mismatched manual receipt leaves the draft unconfirmed until an identical retry succeeds', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Receipt identity');
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(page, 'notes.txt', 'Original draft');
  await page.route(`**/api/workspaces/${project.id}/proposals`, (route) => {
    const input = route.request().postDataJSON();
    return route.fulfill({
      json: {
        id: crypto.randomUUID(),
        workspace_id: project.id,
        run_id: null,
        execution_id: null,
        edit_id: crypto.randomUUID(),
        base_revision_id: null,
        status: 'pending',
        changes: input.changes,
        applied_revision_id: null,
        rejection_reason: null,
      },
    });
  });
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expect(page.locator('.run-error[role=alert]')).toContainText('incompatible shape');
  await expect(page.getByRole('button', { name: 'Retry save', exact: true })).toBeEnabled();
  await expect(
    page.getByRole('button', { name: 'Proposal 1 · pending', exact: true, includeHidden: true }),
  ).toHaveCount(0);
  await page.unroute(`**/api/workspaces/${project.id}/proposals`);
  await page.getByRole('button', { name: 'Retry save', exact: true }).click();
  await expect(page.getByLabel('File diff')).toContainText('Original draft');
});

test('documentation hovers cannot load media supplied by project text', async ({
  page,
  request,
  baseURL,
}) => {
  await workspace(request, 'Private editor docs');
  const probes: string[] = [];
  await page.route('**/__editor_media_probe', (route) => {
    probes.push(route.request().url());
    return route.abort('blockedbyclient');
  });
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(
    page,
    'example.js',
    `/** DOC_CANARY ![pixel](${baseURL}/__editor_media_probe) */\nfunction documented() { return 1; }\ndocumented();`,
  );
  const hover = page.locator('.monaco-hover').filter({ hasText: 'DOC_CANARY' });
  // Monaco clears temporary sanitizer hooks after each render. Check a second
  // render to ensure the application's resource policy survives that cleanup.
  for (let render = 0; render < 2; render++) {
    await page.getByLabel('Code source', { exact: true }).scrollIntoViewIfNeeded();
    await page.locator('.view-lines').getByText('documented', { exact: true }).last().hover();
    await expect(hover).toBeVisible({ timeout: 15000 });
    await expect(hover.locator('img[src], [srcset], [poster], a[href]')).toHaveCount(0);
    expect(probes).toHaveLength(0);
    await page.mouse.move(0, 0);
    await page.keyboard.press('Escape');
    await expect(hover).toBeHidden();
  }
});

test('missing manual-save routes explain the server restart and preserve editable text', async ({
  page,
  request,
}) => {
  const project = await workspace(request, 'Old API save');
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await addFile(page, 'note.txt', 'Do not lose this draft');
  await page.route(`**/api/workspaces/${project.id}/proposals`, (route) =>
    route.fulfill({ status: 404, json: { error: 'API route not found.' } }),
  );
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expect(page.locator('.project-editor .run-error[role=alert]')).toContainText(
    'Restart the Rust server',
  );
  await expect(page.getByLabel('Code source', { exact: true })).toContainText(
    'Do not lose this draft',
  );
  await expect(page.getByRole('button', { name: 'Save for review', exact: true })).toBeEnabled();
  await page.unroute(`**/api/workspaces/${project.id}/proposals`);
  await page.getByRole('button', { name: 'Save for review', exact: true }).click();
  await expect(page.getByLabel('File diff')).toContainText('Do not lose this draft');
});
