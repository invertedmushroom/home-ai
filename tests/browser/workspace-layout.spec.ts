import { openTools, selectProposal } from './helpers';
import { test, expect } from '@playwright/test';
import type { APIRequestContext, Page } from '@playwright/test';

const html =
  '<!doctype html><html><body><h1>Workspace counter</h1><output id="count">0</output><button id="add">Add one</button><script>let n=0;document.querySelector("#add").onclick=()=>document.querySelector("#count").textContent=String(++n);</script></body></html>';

async function project(request: APIRequestContext, title: string) {
  const workspace = await (await request.post('/api/workspaces', { data: { title } })).json();
  const proposed = await request.post(`/api/workspaces/${workspace.id}/proposals`, {
    data: {
      id: crypto.randomUUID(),
      base_revision: { id: null },
      changes: [{ op: 'write', path: 'index.html', content: html }],
    },
  });
  expect(proposed.ok()).toBe(true);
  const proposal = await proposed.json();
  expect((await request.post(`/api/proposals/${proposal.id}/apply`)).ok()).toBe(true);
  return workspace;
}

async function workspace(page: Page) {
  await page.setViewportSize({ width: 1440, height: 960 });
  await page.goto('/');
  for (const name of ['Chat', 'Editor', 'Preview'])
    await expect(page.getByRole('region', { name: `${name} pane`, exact: true })).toBeVisible();
}

test('three panes each have one expand control, fill the stage, and retain their DOM', async ({
  page,
  request,
}) => {
  await project(request, 'Three pane layout');
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await workspace(page);
  await page.getByLabel('Message', { exact: true }).fill('Keep this unsent prompt.');
  await expect(page.locator('.workspace-pane > header')).toHaveCount(0);
  await expect(page.locator('.pane-expand')).toHaveCount(3);
  const source = await page.getByLabel('Code source', { exact: true }).elementHandle();
  for (const name of ['Chat', 'Editor', 'Preview']) {
    await page.getByRole('button', { name: `Expand ${name}`, exact: true }).click();
    const pane = page.getByRole('region', { name: `${name} pane`, exact: true });
    await expect(page.locator('.workspace-pane:visible')).toHaveCount(1);
    const stage = (await page.locator('.workspace-grid').boundingBox())!;
    const rect = (await pane.boundingBox())!;
    expect(rect.width).toBeGreaterThan(stage.width - 20);
    expect(rect.height).toBeGreaterThan(stage.height - 20);
    await page.getByRole('button', { name: `Restore ${name} to three panes`, exact: true }).click();
    await expect(page.locator('.workspace-pane:visible')).toHaveCount(3);
  }
  expect(await source!.evaluate((node) => node.isConnected)).toBe(true);
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Keep this unsent prompt.');
  await page.screenshot({ path: 'logs/workspace-three-pane.png', fullPage: true });
  expect(errors).toEqual([]);
});

test('an unsaved editor draft survives pane expansion without changing accepted files', async ({
  page,
  request,
}) => {
  const w = await project(request, 'Pane draft retention');
  await workspace(page);
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'File editor', exact: true });
  await editor.focus();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.insertText('<h1>Retained pane draft</h1>');
  const node = await editor.elementHandle();
  await page.getByRole('button', { name: 'Expand Chat', exact: true }).click();
  await expect(editor).not.toBeVisible();
  await page.getByRole('button', { name: 'Restore Chat to three panes', exact: true }).click();
  expect(await node!.evaluate((node) => node.isConnected)).toBe(true);
  await expect(page.getByLabel('Code source', { exact: true })).toContainText(
    'Retained pane draft',
  );
  const detail = await (await request.get(`/api/workspaces/${w.id}`)).json();
  expect(detail.revisions).toHaveLength(1);
  expect(detail.revisions[0].files[0].content).toBe(html);
});

test('the diff editor follows its host dimensions through expansion, hiding and split restoration', async ({
  page,
  request,
}) => {
  const w = await project(request, 'Diff layout restoration');
  const detail = await (await request.get(`/api/workspaces/${w.id}`)).json();
  const response = await request.post(`/api/workspaces/${w.id}/proposals`, {
    data: {
      id: crypto.randomUUID(),
      base_revision: { id: detail.revisions.at(-1).id },
      changes: [
        {
          op: 'write',
          path: 'index.html',
          content: html.replace('Workspace counter', 'Updated counter'),
        },
      ],
    },
  });
  expect(response.ok()).toBe(true);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await workspace(page);
  await selectProposal(page, 'Proposal 2 · pending');
  const surface = page.getByLabel('Code comparison', { exact: true });
  const diff = surface.locator('.monaco-diff-editor');
  await expect(diff).toBeVisible();
  const node = await diff.elementHandle();
  async function expectLayout() {
    await expect
      .poll(() =>
        surface.evaluate((host) => {
          const root = host.querySelector<HTMLElement>('.monaco-diff-editor')!;
          const rect = root.getBoundingClientRect();
          const modified = root
            .querySelector<HTMLElement>('.modified .monaco-editor')!
            .getBoundingClientRect();
          return (
            Math.abs(rect.width - host.clientWidth) <= 1 &&
            Math.abs(rect.height - host.clientHeight) <= 1 &&
            Math.abs(modified.height - host.clientHeight) <= 1 &&
            modified.width > 100 &&
            rect.height > 100
          );
        }),
      )
      .toBe(true);
  }
  await expectLayout();
  for (const pane of ['Editor', 'Chat', 'Preview']) {
    await page.getByRole('button', { name: `Expand ${pane}`, exact: true }).click();
    if (pane === 'Editor') await expectLayout();
    else await expect(surface).not.toBeVisible();
    await page.getByRole('button', { name: `Restore ${pane} to three panes`, exact: true }).click();
    await expectLayout();
    expect(await node!.evaluate((node) => node.isConnected)).toBe(true);
  }
  await page.getByRole('button', { name: 'Hide Preview', exact: true }).click();
  await expectLayout();
  await page.getByRole('button', { name: 'Show Preview', exact: true }).click();
  await expectLayout();
  await page.setViewportSize({ width: 1040, height: 960 });
  await expectLayout();
  await expect(surface.locator('.modified .monaco-editor')).toContainText('Updated counter');
  await page.setViewportSize({ width: 1440, height: 960 });
  await expectLayout();
  await page.screenshot({ path: 'logs/workspace-diff-restored.png', fullPage: true });
  expect(errors).toEqual([]);
});

test('pane dividers resize adjacent panes with pointer and keyboard and release interrupted drags', async ({
  page,
  request,
}) => {
  await project(request, 'Pane edge resizing');
  await workspace(page);
  const chat = page.getByRole('region', { name: 'Chat pane', exact: true });
  const editor = page.getByRole('region', { name: 'Editor pane', exact: true });
  const preview = page.getByRole('region', { name: 'Preview pane', exact: true });
  const divider = page.getByRole('separator', { name: 'Resize Chat and Editor', exact: true });
  const first = (await chat.boundingBox())!;
  const second = (await editor.boundingBox())!;
  const third = (await preview.boundingBox())!;
  const source = await page.getByLabel('Code source', { exact: true }).elementHandle();
  const handle = (await divider.boundingBox())!;
  await page.mouse.move(handle.x + handle.width / 2, handle.y + 80);
  await page.mouse.down();
  await page.mouse.move(handle.x + handle.width / 2 + 80, handle.y + 80, { steps: 5 });
  await page.mouse.up();
  await expect.poll(async () => (await chat.boundingBox())!.width).toBeCloseTo(first.width + 80, 0);
  await expect
    .poll(async () => (await editor.boundingBox())!.width)
    .toBeCloseTo(second.width - 80, 0);
  expect((await preview.boundingBox())!.width).toBeCloseTo(third.width, 0);
  await divider.focus();
  await page.keyboard.press('ArrowLeft');
  await expect.poll(async () => (await chat.boundingBox())!.width).toBeCloseTo(first.width + 64, 0);
  const current = (await divider.boundingBox())!;
  await page.mouse.move(current.x + 4, current.y + 90);
  await page.mouse.down();
  await page.mouse.move(current.x + 40, current.y + 90);
  await expect(divider).toHaveClass(/is-interacting/);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await expect(divider).not.toHaveClass(/is-interacting/);
  const released = (await chat.boundingBox())!.width;
  await page.mouse.move(current.x + 100, current.y + 90);
  await page.mouse.up();
  expect((await chat.boundingBox())!.width).toBe(released);
  expect(await source!.evaluate((node) => node.isConnected)).toBe(true);
  const right = page.getByRole('separator', { name: 'Resize Editor and Preview', exact: true });
  const edge = (await right.boundingBox())!;
  await page.mouse.move(edge.x + 4, edge.y + 100);
  await page.mouse.down();
  await page.mouse.move(edge.x + 3000, edge.y + 100);
  await page.mouse.up();
  expect((await preview.boundingBox())!.width).toBeGreaterThanOrEqual(259);
  expect((await editor.boundingBox())!.width).toBeGreaterThanOrEqual(379);
  await page.setViewportSize({ width: 1040, height: 960 });
  const bottom = page.getByRole('separator', { name: 'Resize Editor and Preview', exact: true });
  await expect(bottom).toHaveAttribute('aria-orientation', 'horizontal');
  const height = (await preview.boundingBox())!.height;
  await bottom.focus();
  await page.keyboard.press('ArrowUp');
  await expect.poll(async () => (await preview.boundingBox())!.height).toBeCloseTo(height + 16, 0);
  await page.setViewportSize({ width: 500, height: 700 });
  await expect(page.getByRole('separator')).toHaveCount(0);
});

test('hiding Preview gives Editor its space and preserves the running frame until explicitly stopped', async ({
  page,
  request,
}) => {
  await project(request, 'Hide Preview');
  await workspace(page);
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  const frame = page.frameLocator('iframe');
  await frame.getByRole('button', { name: 'Add one', exact: true }).click();
  const node = await page.locator('iframe').elementHandle();
  const editor = page.getByRole('region', { name: 'Editor pane', exact: true });
  const chat = page.getByRole('region', { name: 'Chat pane', exact: true });
  const width = (await editor.boundingBox())!.width;
  const chatWidth = (await chat.boundingBox())!.width;
  await page.getByRole('button', { name: 'Hide Preview', exact: true }).click();
  await expect(page.locator('.workspace-pane:visible')).toHaveCount(2);
  expect((await editor.boundingBox())!.width).toBeGreaterThan(width + 200);
  expect((await chat.boundingBox())!.width).toBeCloseTo(chatWidth, -1);
  await page.getByRole('button', { name: 'Show Preview', exact: true }).click();
  expect(await node!.evaluate((node) => node.isConnected)).toBe(true);
  await expect(frame.locator('#count')).toHaveText('1');
  await page.getByRole('button', { name: 'Expand Preview', exact: true }).click();
  await page.getByRole('button', { name: 'Hide Preview', exact: true }).click();
  await expect(page.locator('.workspace-pane:visible')).toHaveCount(2);
  await expect(chat).toBeVisible();
  await expect(editor).toBeVisible();
  await expect(page.getByRole('button', { name: 'Expand Editor', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Preview pane', exact: true })).toBeVisible();
  await expect(frame.locator('#count')).toHaveText('0');
  await page.setViewportSize({ width: 390, height: 700 });
  await page.getByRole('button', { name: 'Hide Preview', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Show Preview', exact: true })).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('long replies flow through one Chat scroll area while the composer stays available', async ({
  page,
  request,
}) => {
  const w = await project(request, 'Chat scrolling');
  const run = await (
    await request.post(`/api/workspaces/${w.id}/runs`, {
      data: { prompt: 'Long reply fixture', mode: 'chat', backend: 'demo' },
    })
  ).json();
  await expect
    .poll(async () => (await (await request.get(`/api/runs/${run.id}`)).json()).status)
    .toBe('succeeded');
  const detail = await (await request.get(`/api/workspaces/${w.id}`)).json();
  const content = Array.from(
    { length: 120 },
    (_, i) => `Paragraph ${i}: A complete reply remains readable while scrolling the conversation.`,
  ).join('\n\n');
  detail.messages.find(
    (m: { role: string; run_id: string }) => m.role === 'assistant' && m.run_id === run.id,
  ).content = content;
  detail.runs.find((r: { id: string }) => r.id === run.id).output = content;
  await page.route(`**/api/workspaces/${w.id}`, (route) => route.fulfill({ json: detail }));
  await workspace(page);
  const scroller = page.locator('.pane-chat .workspace-pane-body');
  const reply = page.locator('.message.assistant > pre');
  await expect(reply).toHaveText(content);
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 960 });
    await scroller.evaluate((node) => {
      node.scrollTop = 0;
    });
    expect(await reply.evaluate((node) => node.scrollHeight <= node.clientHeight + 1)).toBe(true);
    const pane = (await scroller.boundingBox())!;
    const composer = (await page.getByLabel('Message', { exact: true }).boundingBox())!;
    expect(composer.y).toBeGreaterThanOrEqual(pane.y);
    expect(composer.y + composer.height).toBeLessThanOrEqual(pane.y + pane.height);
    await page.mouse.move(pane.x + 60, pane.y + 120);
    await page.mouse.wheel(0, 350);
    await expect.poll(() => scroller.evaluate((node) => node.scrollTop)).toBeGreaterThan(0);
    expect(await page.locator('.messages').evaluate((node) => node.scrollTop)).toBe(0);
  }
});

test('preview runs only on request, survives expansion, and stops explicitly', async ({
  page,
  request,
}) => {
  await project(request, 'Pane preview lifecycle');
  await workspace(page);
  await expect(page.locator('iframe')).toHaveCount(0);
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  const frame = page.frameLocator('iframe');
  await frame.getByRole('button', { name: 'Add one', exact: true }).click();
  await expect(frame.locator('#count')).toHaveText('1');
  const node = await page.locator('iframe').elementHandle();
  await page.getByRole('button', { name: 'Expand Chat', exact: true }).click();
  await expect(page.locator('iframe')).toHaveCount(1);
  await page.getByRole('button', { name: 'Restore Chat to three panes', exact: true }).click();
  await page.getByRole('button', { name: 'Expand Preview', exact: true }).click();
  expect(await node!.evaluate((node) => node.isConnected)).toBe(true);
  await expect(frame.locator('#count')).toHaveText('1');
  await page.getByRole('button', { name: 'Stop preview', exact: true }).click();
  await expect(page.locator('iframe')).toHaveCount(0);
  await page.getByRole('button', { name: 'Restore Preview to three panes', exact: true }).click();
  await page.getByRole('button', { name: 'Run preview ↗', exact: true }).click();
  await expect(frame.locator('#count')).toHaveText('0');
  await page.screenshot({ path: 'logs/workspace-preview.png', fullPage: true });
  await page.reload();
  await expect(page.locator('iframe')).toHaveCount(0);
});

test('floating tools move, resize, recover interrupted gestures and retain their inputs when closed', async ({
  page,
  request,
}) => {
  await project(request, 'Floating workspace tools');
  await workspace(page);
  await openTools(page);
  const tools = page.getByRole('region', { name: 'Tools & attachments window', exact: true });
  const handle = tools.getByRole('button', {
    name: 'Move Tools & attachments window',
    exact: true,
  });
  const before = (await tools.boundingBox())!;
  await handle.focus();
  await page.keyboard.press('ArrowRight');
  await expect.poll(async () => (await tools.boundingBox())!.x).toBe(before.x + 16);
  await tools
    .getByRole('button', { name: 'Resize Tools & attachments window', exact: true })
    .focus();
  await page.keyboard.press('ArrowRight');
  await expect.poll(async () => (await tools.boundingBox())!.width).toBe(before.width + 16);
  const title = (await handle.boundingBox())!;
  await page.mouse.move(title.x + 80, title.y + 12);
  await page.mouse.down();
  await page.mouse.move(title.x + 110, title.y + 12);
  await expect(tools).toHaveClass(/is-interacting/);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await expect(tools).not.toHaveClass(/is-interacting/);
  const released = (await tools.boundingBox())!;
  await page.mouse.move(title.x + 200, title.y + 12);
  await page.mouse.up();
  expect((await tools.boundingBox())!.x).toBe(released.x);
  await tools.getByRole('button', { name: 'Close Tools & attachments', exact: true }).click();
  await expect(tools).not.toBeVisible();
  await openTools(page);
  await expect(tools).toBeVisible();
  expect((await tools.boundingBox())!.x).toBe(released.x);
  await page.setViewportSize({ width: 500, height: 700 });
  await expect
    .poll(async () => {
      const r = (await tools.boundingBox())!;
      return r.x >= 0 && r.x + r.width <= 501 && r.y >= 0 && r.y + r.height <= 701;
    })
    .toBe(true);
});

test('run polling and Stop remain available while Chat is hidden by an expanded pane', async ({
  page,
  request,
}) => {
  const w = await project(request, 'Pane owned run');
  let current: Record<string, unknown>;
  let polls = 0,
    cancellations = 0;
  await page.route(`**/api/workspaces/${w.id}`, async (route) => {
    const response = await route.fetch();
    const detail = await response.json();
    if (current) {
      polls++;
      detail.runs = detail.runs.map((run: { id: string }) =>
        run.id === current.id ? current : run,
      );
      detail.messages = detail.messages.filter(
        (m: { run_id: string; role: string }) => m.run_id !== current.id || m.role === 'user',
      );
      detail.proposals = detail.proposals.filter(
        (p: { run_id: string }) => p.run_id !== current.id,
      );
    }
    await route.fulfill({ json: detail });
  });
  await page.route(`**/api/workspaces/${w.id}/runs`, async (route) => {
    const response = await route.fetch();
    current = await response.json();
    await route.fulfill({ status: 202, json: current });
  });
  await page.route('**/api/runs/*', async (route) => {
    if (route.request().method() === 'DELETE') {
      cancellations++;
      await route.fetch();
      current = { ...current, status: 'cancelled' };
    }
    await route.fulfill({ status: 200, json: current });
  });
  await workspace(page);
  await page.getByLabel('Message', { exact: true }).fill('Build a counter.');
  await page.getByRole('button', { name: /^Send/ }).click();
  const composerStop = page.locator('.composer').getByRole('button', { name: 'Stop request' });
  const toolbarStop = page
    .locator('.workspace-toolbar')
    .getByRole('button', { name: 'Stop request', exact: true });
  await expect(composerStop).toBeVisible();
  await expect(toolbarStop).toHaveCount(0);
  await page.getByRole('button', { name: 'Expand Editor', exact: true }).click();
  await expect(page.getByLabel('Message', { exact: true })).not.toBeVisible();
  await expect(composerStop).toHaveCount(0);
  await expect(toolbarStop).toBeVisible();
  await expect.poll(() => polls).toBeGreaterThan(0);
  expect(cancellations).toBe(0);
  await toolbarStop.click();
  await expect.poll(() => cancellations).toBe(1);
  await expect(toolbarStop).not.toBeVisible();
});

test('a chat change link selects its saved proposal and reveals Editor from expanded Chat', async ({
  page,
  request,
}) => {
  const w = await project(request, 'Chat proposal navigation');
  await workspace(page);
  await page.getByRole('button', { name: 'Expand Chat', exact: true }).click();
  await page.getByLabel('Message', { exact: true }).fill('Build a counter app.');
  await page.getByRole('button', { name: /^Send/ }).click();
  const link = page.getByRole('button', { name: 'Review changes', exact: true }).last();
  await expect(link).toBeVisible();
  await link.click();
  await expect(page.getByRole('region', { name: 'Editor pane', exact: true })).toBeVisible();
  await expect(page.locator('.workspace-pane:visible')).toHaveCount(1);
  const detail = await (await request.get(`/api/workspaces/${w.id}`)).json();
  const latest = detail.proposals.at(-1);
  await expect(
    page.getByRole('button', {
      name: `Proposal ${detail.proposals.length} · pending`,
      exact: true,
      includeHidden: true,
    }),
  ).toHaveAttribute('aria-current', 'true');
  expect(latest.run_id).toBe(detail.runs.at(-1).id);
  await expect(page.getByText('Review proposed changes', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Restore Editor to three panes', exact: true }).click();
  await selectProposal(page, 'Proposal 1 · applied');
  await expect(page.locator('.project-state strong')).toHaveText('Proposal 1 · applied');
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Review changes', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Apply changes', exact: true })).toBeVisible();
});
