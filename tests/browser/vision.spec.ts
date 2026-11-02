import { closeSettings, openModels, openTools, selectModel } from './helpers';
import { test, expect } from '@playwright/test';
import type { Page, APIRequestContext } from '@playwright/test';

const status = {
  provider: 'openai',
  model: 'vision-model',
  model_health: 'not_probed',
  backends: [{ id: 'openai', model: 'vision-model' }],
  asset_library_available: true,
  chat_image_input_available: true,
  model_catalog: {
    default_profile: 'vision',
    profiles: [
      {
        id: 'vision',
        label: 'Vision test',
        backend: 'openai',
        model: 'vision-model',
        tool_calls: true,
        vision: true,
      },
      {
        id: 'text',
        label: 'Text test',
        backend: 'openai',
        model: 'text-model',
        tool_calls: true,
        vision: false,
      },
    ],
    preferences: { conversation: [], coding: [] },
  },
};

async function setup(page: Page, request: APIRequestContext) {
  const png = Buffer.from(
    await page.evaluate(() => {
      const canvas = document.createElement('canvas');
      canvas.width = 8;
      canvas.height = 6;
      const context = canvas.getContext('2d')!;
      context.fillStyle = '#138a35';
      context.fillRect(0, 0, 8, 6);
      return canvas.toDataURL('image/png').split(',')[1];
    }),
    'base64',
  );
  const workspace = await (
    await request.post('/api/workspaces', { data: { title: 'Chat vision' } })
  ).json();
  const imageId = crypto.randomUUID();
  const saved = await request.post(
    `/api/workspaces/${workspace.id}/image-assets?id=${imageId}&name=scene.png`,
    {
      data: png,
      headers: { 'Content-Type': 'image/png' },
    },
  );
  expect(saved.status()).toBe(201);
  const image = await saved.json();
  const detail = await (await request.get(`/api/workspaces/${workspace.id}`)).json();
  await page.route('**/api/status', (route) => route.fulfill({ json: status }));
  await page.route(`**/api/workspaces/${workspace.id}`, (route) => route.fulfill({ json: detail }));
  await page.goto('/');
  await openTools(page);
  return { workspace, image, detail };
}

test('chat attachment survives failed sends, stays separate from editing, and renders after reload', async ({
  page,
  request,
}) => {
  const { workspace, image, detail } = await setup(page, request);
  const submissions: Record<string, unknown>[] = [];
  let fail = true;
  let malformed = false;
  await page.route(`**/api/workspaces/${workspace.id}/runs`, (route) => {
    const input = route.request().postDataJSON();
    submissions.push(input);
    if (fail) return route.fulfill({ status: 400, json: { error: 'Fixture rejected the send.' } });
    const run = {
      id: crypto.randomUUID(),
      workspace_id: workspace.id,
      status: 'succeeded',
      error: null,
      provider: 'openai',
      model: 'vision-model',
      mode: 'chat',
      workspace_tools: input.workspace_tools,
      output: 'I see the scene.',
      base_revision_id: null,
      queue_ms: 0,
      duration_ms: 5,
      image_attachments: [image],
      stats: {
        reported_model: 'vision-model',
        finish_reason: 'stop',
        input_tokens: null,
        output_tokens: null,
        total_tokens: null,
        first_token_ms: null,
      },
      model_route: {
        profile_id: 'vision',
        label: 'Vision test',
        backend: 'openai',
        model: 'vision-model',
        vision: true,
        automatic: false,
        task: null,
        reason: 'Explicit selection.',
      },
    };
    if (malformed) {
      run.image_attachments = [{ ...image, workspace_id: crypto.randomUUID() }];
      return route.fulfill({ status: 202, json: run });
    }
    detail.runs.push(run);
    detail.messages.push(
      { role: 'user', content: input.prompt, run_id: run.id },
      { role: 'assistant', content: run.output, run_id: run.id },
    );
    return route.fulfill({ status: 202, json: run });
  });
  await page.getByLabel('Attach scene.png to chat').check();
  await closeSettings(page);
  await page.getByLabel('Message', { exact: true }).fill('Describe this scene.');
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.getByText('Fixture rejected the send.')).toBeVisible();
  await expect(page.getByLabel('Attach scene.png to chat')).toBeChecked();
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Describe this scene.');
  expect(submissions[0].image_attachment_ids).toEqual([image.id]);
  expect(submissions[0].image_profile_id).toBeUndefined();
  expect(submissions[0].image_input).toBeUndefined();
  await selectModel(page, 'profile:text');
  await expect(page.getByLabel('Attach scene.png to chat')).toBeChecked();
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(
    page.getByText(
      'Choose a vision model that supports the selected tools, or remove the chat image attachments.',
    ),
  ).toBeVisible();
  expect(submissions).toHaveLength(1);
  await selectModel(page, 'profile:vision');
  fail = false;
  malformed = true;
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(
    page.getByText(
      'The server response has an incompatible shape. Refresh or check the server version.',
    ),
  ).toBeVisible();
  await expect(page.getByLabel('Attach scene.png to chat')).toBeChecked();
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Describe this scene.');
  malformed = false;
  await page.getByRole('button', { name: 'Send', exact: false }).click();
  await expect(page.getByText('I see the scene.', { exact: true })).toBeVisible();
  await expect(page.getByLabel('Attach scene.png to chat')).not.toBeChecked();
  await expect(
    page.getByLabel('Attached chat images').getByRole('img', { name: 'scene.png' }),
  ).toBeVisible();
  await page.reload();
  await expect(
    page.getByLabel('Attached chat images').getByRole('img', { name: 'scene.png' }),
  ).toBeVisible();
});

test('switching workspace clears chat attachments', async ({ page, request }) => {
  await setup(page, request);
  await page.getByLabel('Attach scene.png to chat').check();
  await closeSettings(page);
  await page.getByLabel('Message', { exact: true }).fill('Unsent request');
  await page.getByRole('button', { name: 'New workspace', exact: false }).click();
  await expect(page.getByLabel('Attach scene.png to chat')).toHaveCount(0);
  await page.reload();
  await openTools(page);
  await expect(page.getByLabel('Chat images')).toBeVisible();
  await expect(page.getByLabel('Attach scene.png to chat')).toHaveCount(0);
});

test('OpenAI image presets disclose the hosted provider separately from chat vision', async ({
  page,
}) => {
  await page.route('**/api/status', (route) =>
    route.fulfill({
      json: {
        ...status,
        image_input_available: true,
        image_profiles: [
          {
            id: 'flare',
            label: 'OpenAI Flare',
            model: 'gpt-image-2.5-flare',
            provider: 'openai',
            fingerprint: 'a'.repeat(64),
            loras: [],
            lora_strengths: null,
          },
        ],
      },
    }),
  );
  await page.goto('/');
  await openTools(page);
  await openModels(page);
  await expect(page.getByLabel('Image generation profile')).toHaveValue('flare');
  await openModels(page);
  await page.getByText('Image preset details', { exact: true }).click();
  await expect(
    page.getByText('Provider: OpenAI · hosted image generation and editing', { exact: true }),
  ).toBeVisible();
  await openTools(page);
  await expect(page.getByLabel('Chat images')).toBeVisible();
  await expect(page.getByLabel('Reference image')).toBeVisible();
});
