import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve, sep } from 'node:path';
import { runObservabilityEvidence } from '../../scripts/observability-evidence.mjs';

const runId = '11111111-1111-4111-8111-111111111111';
const workspaceId = '22222222-2222-4222-8222-222222222222';
const instance = '33333333-3333-4333-8333-333333333333';
const traceId = `0${'a'.repeat(31)}`;

async function cleanup(dir) {
  assert(resolve(dir).startsWith(`${resolve(tmpdir())}${sep}`));
  assert(basename(dir).startsWith('deep-observability-'));
  await rm(dir, { recursive: true, force: true });
}

function fixture(forbiddenLabel = false) {
  let generations = 0;
  let searches = 0;
  let generationStarted;
  const run = { id: runId, workspace_id: workspaceId, status: 'succeeded', model_rounds: 0 };
  return {
    generations: () => generations,
    searches: () => searches,
    fetch: async (value, options) => {
      const url = new URL(value);
      let body;
      if (url.pathname === '/api/status')
        body = { guest_mode: false, backends: [{ id: 'compatible' }], image_profiles: [] };
      else if (url.pathname === '/api/workspaces') body = { id: workspaceId };
      else if (url.pathname === `/api/workspaces/${workspaceId}/runs`) {
        assert.equal(options.method, 'POST');
        generations++;
        generationStarted = Date.now();
        body = { ...run, status: 'queued' };
      } else if (url.pathname === `/api/runs/${runId}`) body = run;
      else if (url.pathname === '/api/search') {
        const start = Number(url.searchParams.get('start'));
        const end = Number(url.searchParams.get('end'));
        const padding = generationStarted / 1000 - start;
        assert(padding >= 120 && padding < 123);
        assert(end > Date.now() / 1000 && end <= Date.now() / 1000 + 2);
        assert(url.searchParams.get('q').includes(runId));
        searches++;
        body = { traces: searches === 1 ? [] : [{ traceID: traceId.slice(1) }] };
      } else if (url.pathname === `/api/traces/${traceId}`)
        body = {
          batches: [
            {
              resource: {
                attributes: [
                  { key: 'service.name', value: { stringValue: 'deep-server' } },
                  { key: 'service.instance.id', value: { stringValue: instance } },
                ],
              },
              scopeSpans: [
                {
                  spans: [
                    {
                      name: 'run',
                      attributes: [
                        { key: 'run_id', value: { stringValue: runId } },
                        { key: 'outcome', value: { stringValue: 'succeeded' } },
                      ],
                    },
                    ...['resource.wait', 'model.operation', 'model_call'].map((name) => ({ name })),
                  ],
                },
              ],
            },
          ],
        };
      else if (url.pathname === '/api/v1/query') {
        const query = url.searchParams.get('query');
        let result = [];
        if (query.includes('snapshot_timestamp'))
          result = [{ metric: {}, value: [0, String(Date.now() / 1000)] }];
        else if (query.includes(`service_instance_id="${instance}"`))
          result = [
            {
              metric: {
                service_instance_id: instance,
                ...(forbiddenLabel ? { run_id: runId } : {}),
              },
              value: [0, '1'],
            },
          ];
        body = { status: 'success', data: { resultType: 'vector', result } };
      } else throw Error(`Unexpected fixture request ${url.pathname}`);
      return Response.json(body);
    },
  };
}

test('an existing observability report prevents all network requests and generation', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'deep-observability-'));
  const report = join(dir, 'existing.json');
  const original = globalThis.fetch;
  try {
    await writeFile(report, 'preserved');
    globalThis.fetch = () => {
      throw Error('No network request is permitted');
    };
    await assert.rejects(
      runObservabilityEvidence({ url: 'http://127.0.0.1:4321', backend: 'compatible', report }),
      { code: 'EEXIST' },
    );
    assert.equal(await readFile(report, 'utf8'), 'preserved');
  } finally {
    globalThis.fetch = original;
    await cleanup(dir);
  }
});

test('evidence verifies the real run trace and same-process counter, rejects identity labels and never retries generation', async () => {
  const original = globalThis.fetch;
  for (const forbidden of [false, true]) {
    const dir = await mkdtemp(join(tmpdir(), 'deep-observability-'));
    const report = join(dir, 'report.json');
    const fake = fixture(forbidden);
    try {
      globalThis.fetch = fake.fetch;
      const action = runObservabilityEvidence({
        url: 'http://127.0.0.1:4321',
        backend: 'compatible',
        report,
      });
      if (forbidden) await assert.rejects(action, /Unexpected metric label run_id/);
      else assert.equal((await action).trace_id, traceId);
      const saved = JSON.parse(await readFile(report, 'utf8'));
      assert.equal(saved.passed, !forbidden);
      assert.equal(saved.service_instance_id, instance);
      assert.equal(fake.generations(), 1);
      assert.equal(fake.searches(), 2);
    } finally {
      globalThis.fetch = original;
      await cleanup(dir);
    }
  }
});
