import { openWorkspaces } from './helpers';
import { test, expect } from '@playwright/test';

test('workspace removal is confirmed, survives a lost response and leaves other workspaces intact', async ({
  page,
  request,
}) => {
  const other = await (
    await request.post('/api/workspaces', { data: { title: 'Keep workspace' } })
  ).json();
  const target = await (
    await request.post('/api/workspaces', { data: { title: 'Remove workspace' } })
  ).json();
  await request.post(`/api/workspaces/${target.id}/proposals`, {
    data: {
      id: crypto.randomUUID(),
      base_revision: { id: null },
      changes: [{ op: 'write', path: 'file.txt', content: 'Delete with workspace' }],
    },
  });
  await page.goto('/');
  await openWorkspaces(page);
  await page.getByTitle('Delete Remove workspace', { exact: true }).click();
  const confirmation = page.getByLabel('Delete workspace confirmation');
  await expect(confirmation).toContainText('permanently removes');
  await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect((await request.get(`/api/workspaces/${target.id}`)).status()).toBe(200);
  let deletes = 0;
  await page.route(`**/api/workspaces/${target.id}`, async (route) => {
    if (route.request().method() !== 'DELETE') return route.continue();
    deletes++;
    if (deletes === 1) {
      const response = await route.fetch();
      expect(response.status()).toBe(204);
      return route.abort('failed');
    }
    return route.continue();
  });
  await openWorkspaces(page);
  await page.getByTitle('Delete Remove workspace', { exact: true }).click();
  await confirmation.getByRole('button', { name: 'Delete workspace', exact: true }).click();
  await expect(confirmation.getByRole('alert')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toHaveText(
    'Remove workspace ⌄',
  );
  await confirmation.getByRole('button', { name: 'Delete workspace', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toHaveText(
    'Keep workspace ⌄',
  );
  await expect(page.getByTitle('Delete Remove workspace', { exact: true })).toHaveCount(0);
  expect(deletes).toBe(2);
  expect((await request.get(`/api/workspaces/${target.id}`)).status()).toBe(404);
  expect((await request.get(`/api/workspaces/${other.id}`)).status()).toBe(200);
  await page.reload();
  await expect(page.getByTitle('Delete Remove workspace', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Choose workspace', exact: true })).toHaveText(
    'Keep workspace ⌄',
  );
  await expect(page.getByText(/SAVED ON THIS SERVER|YOUR MACHINE\. YOUR WORK\./i)).toHaveCount(0);
});

test('failed removal preserves an unsaved draft and its selected workspace', async ({
  page,
  request,
}) => {
  const target = await (
    await request.post('/api/workspaces', { data: { title: 'Busy removal' } })
  ).json();
  await page.goto('/');
  await page.getByRole('button', { name: /^(Edit|Edit copy|Resume draft)$/, exact: true }).click();
  await page.getByLabel('New file path').fill('draft.txt');
  await page.getByRole('button', { name: 'Add file', exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'File editor', exact: true });
  await expect(editor).toBeVisible();
  await editor.focus();
  await page.keyboard.insertText('Keep this unsaved text');
  await page.route(`**/api/workspaces/${target.id}`, (route) =>
    route.request().method() === 'DELETE'
      ? route.fulfill({ status: 409, json: { error: 'Stop active jobs before deleting.' } })
      : route.continue(),
  );
  await openWorkspaces(page);
  await page.getByTitle('Delete Busy removal', { exact: true }).click();
  const confirmation = page.getByLabel('Delete workspace confirmation');
  await expect(confirmation).toContainText('including the unsaved draft');
  await confirmation.getByRole('button', { name: 'Delete workspace', exact: true }).click();
  await expect(confirmation.getByRole('alert')).toContainText('Stop active jobs');
  await expect(page.getByLabel('Code source', { exact: true })).toContainText(
    'Keep this unsaved text',
  );
  await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Close Workspaces', exact: true }).click();
  await page.getByRole('button', { name: 'Discard draft', exact: true }).click();
});
