import { test, expect, type Page } from '@playwright/test';
import { openWorkspaces } from './helpers';

async function workspaces(page: Page): Promise<{ id: string; title: string }[]> {
  return page.evaluate(async () => (await fetch('/api/workspaces')).json());
}

test('two browsers retain separate workspaces and delete only their own data', async ({
  page,
  browser,
  baseURL,
}) => {
  const other = await browser.newContext({ baseURL });
  try {
    const friend = await other.newPage();
    await Promise.all([page.goto('/'), friend.goto('/')]);
    for (const view of [page, friend]) {
      await expect(view.getByRole('button', { name: 'New workspace' })).toBeEnabled();
      await expect(
        view.getByText('Your workspaces belong to this browser.', { exact: false }),
      ).toBeVisible();
      await view.getByRole('button', { name: 'New workspace' }).click();
    }
    await expect.poll(async () => (await workspaces(page)).length).toBe(1);
    await expect.poll(async () => (await workspaces(friend)).length).toBe(1);
    const [mine] = await workspaces(page);
    const [theirs] = await workspaces(friend);
    expect(mine.id).not.toBe(theirs.id);
    expect(
      await friend.evaluate(async (id) => (await fetch(`/api/workspaces/${id}`)).status, mine.id),
    ).toBe(404);
    await page.reload();
    await expect(page.getByRole('button', { name: 'New workspace' })).toBeEnabled();
    expect((await workspaces(page))[0].id).toBe(mine.id);
    let deletes = 0;
    await page.route(`**/api/workspaces/${mine.id}`, async (route) => {
      if (route.request().method() !== 'DELETE') return route.continue();
      deletes++;
      if (deletes === 1) {
        expect((await route.fetch()).status()).toBe(204);
        return route.abort('failed');
      }
      return route.continue();
    });
    await openWorkspaces(page);
    await page.getByTitle(`Delete ${mine.title}`, { exact: true }).click();
    const confirmation = page.getByLabel('Delete workspace confirmation');
    await confirmation.getByRole('button', { name: 'Delete workspace', exact: true }).click();
    await expect(confirmation.getByRole('alert')).toBeVisible();
    await confirmation.getByRole('button', { name: 'Delete workspace', exact: true }).click();
    await expect(confirmation).toHaveCount(0);
    expect(await workspaces(page)).toHaveLength(0);
    expect((await workspaces(friend))[0].id).toBe(theirs.id);
    const cookie = (await page.context().cookies()).find(
      (cookie) => cookie.name === '__Host-deep_guest',
    );
    expect(cookie).toMatchObject({ httpOnly: true, secure: true, sameSite: 'Lax', path: '/' });
  } finally {
    await other.close();
  }
});

test('simultaneous first tabs serialize guest initialization and gate workspace actions', async ({
  browser,
  baseURL,
}) => {
  const context = await browser.newContext({ baseURL });
  try {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let started!: () => void;
    const firstRequest = new Promise<void>((resolve) => {
      started = resolve;
    });
    let active = 0;
    let maximum = 0;
    let calls = 0;
    await context.route('**/api/session', async (route) => {
      calls++;
      active++;
      maximum = Math.max(maximum, active);
      if (calls === 1) {
        started();
        await gate;
      }
      const response = await route.fetch();
      await route.fulfill({ response });
      active--;
    });
    const first = await context.newPage();
    const second = await context.newPage();
    await Promise.all([first.goto('/'), second.goto('/')]);
    await firstRequest;
    await expect(first.getByRole('button', { name: 'New workspace' })).toBeDisabled();
    await expect(second.getByRole('button', { name: 'New workspace' })).toBeDisabled();
    release();
    await expect(first.getByRole('button', { name: 'New workspace' })).toBeEnabled();
    await expect(second.getByRole('button', { name: 'New workspace' })).toBeEnabled();
    expect(maximum).toBe(1);
    expect(calls).toBe(2);
    await first.getByRole('button', { name: 'New workspace' }).click();
    await expect.poll(async () => (await workspaces(first)).length).toBe(1);
    await second.reload();
    await expect(second.getByRole('button', { name: 'New workspace' })).toBeEnabled();
    expect(await workspaces(second)).toEqual(await workspaces(first));
  } finally {
    await context.close();
  }
});
