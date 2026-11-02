import { expect, type Page } from '@playwright/test';

export async function openTools(page: Page) {
  await closeModels(page);
  const panel = page.getByRole('region', { name: 'Tools & attachments window', exact: true });
  if (!(await panel.isVisible()))
    await page
      .locator('.workspace-toolbar')
      .getByRole('button', { name: 'Tools & attachments', exact: true })
      .click();
}

export async function openModels(page: Page) {
  await closeTools(page);
  const panel = page.getByRole('region', { name: 'Models window', exact: true });
  if (!(await panel.isVisible()))
    await page.getByRole('button', { name: 'Models', exact: true }).click();
}

export async function closeModels(page: Page) {
  const close = page.getByRole('button', { name: 'Close Models', exact: true });
  if (await close.isVisible()) await close.click();
}

export async function closeSettings(page: Page) {
  await closeTools(page);
  await closeModels(page);
}

export async function selectModel(page: Page, value: string) {
  await openModels(page);
  await page.getByLabel('Model backend').selectOption(value);
  await closeModels(page);
}

export async function selectTask(page: Page, value: string) {
  await openModels(page);
  await page.getByLabel('Task preference').selectOption(value);
  await closeModels(page);
}

export async function openAssets(page: Page) {
  const panel = page.getByRole('region', { name: 'Files & assets window', exact: true });
  if (!(await panel.isVisible()))
    await page.getByRole('button', { name: 'Files & assets', exact: true }).click();
}

export async function closeTools(page: Page) {
  const close = page.getByRole('button', { name: 'Close Tools & attachments', exact: true });
  if (await close.isVisible()) await close.click();
}

export async function openWorkspaces(page: Page) {
  const panel = page.getByRole('region', { name: 'Workspaces window', exact: true });
  if (!(await panel.isVisible()))
    await page.getByRole('button', { name: 'Choose workspace', exact: true }).click();
}

export async function openRevisions(page: Page) {
  const panel = page.getByRole('region', { name: 'Revisions window', exact: true });
  if (!(await panel.isVisible()))
    await page.getByRole('button', { name: 'Choose project version', exact: true }).click();
  await expect(panel).toBeVisible();
}

export async function selectProposal(page: Page, name: string) {
  await openRevisions(page);
  await page
    .getByRole('region', { name: 'Revisions window', exact: true })
    .getByRole('button', { name, exact: true })
    .click();
}

export async function expectProposal(page: Page, name: string) {
  await openRevisions(page);
  await expect(
    page
      .getByRole('region', { name: 'Revisions window', exact: true })
      .getByRole('button', { name, exact: true }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Close Revisions', exact: true }).click();
}

export async function selectRevision(page: Page, version: string | { label: string }) {
  await openRevisions(page);
  const panel = page.getByRole('region', { name: 'Revisions window', exact: true });
  if (typeof version === 'string') await panel.locator(`button[value="${version}"]`).click();
  else await panel.getByRole('button', { name: new RegExp(`^${version.label}(?: ·|$)`) }).click();
}

export async function selectCurrent(page: Page) {
  await openRevisions(page);
  await page
    .getByRole('region', { name: 'Current version', exact: true })
    .getByRole('button')
    .click();
}

export async function openExport(page: Page) {
  const menu = page.locator('.project-export');
  if ((await menu.getAttribute('open')) === null) await menu.locator('summary').click();
}

export async function openExecution(page: Page) {
  await openRuns(page);
  await expect(page.getByRole('region', { name: 'Node execution', exact: true })).toBeVisible();
  if (await page.locator('.execution-entry').isVisible()) return;
  const tools = page.locator('.execution-tools');
  await expect(tools).toBeVisible();
  if ((await tools.getAttribute('open')) === null) await tools.locator(':scope > summary').click();
}

export async function openRuns(page: Page) {
  if (!(await page.getByRole('region', { name: 'Runs window', exact: true }).isVisible()))
    await page.getByRole('button', { name: 'Runs', exact: true }).click();
}

export async function closeRuns(page: Page) {
  const close = page.getByRole('button', { name: 'Close Runs', exact: true });
  if (await close.isVisible()) await close.click();
}

export async function closeWorkspaces(page: Page) {
  const close = page.getByRole('button', { name: 'Close Workspaces', exact: true });
  if (await close.isVisible()) await close.click();
}
