import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseGpu, runHandoff } from '../../scripts/image-handoff.mjs';

async function fixture(t, run) {
  const folder = await mkdtemp(join(tmpdir(), 'deep-handoff-'));
  const state = {
    run,
    cancellations: 0,
    followupReply: '42',
    backend: 'lmstudio',
    residencyRequests: 0,
  };
  const server = createServer((req, res) => {
    let body;
    if (req.url === '/api/status') {
      body = {
        guest_mode: false,
        backends: [{ id: state.backend, model: 'fixture' }],
        image_profiles: [{ id: 'fixture', model: 'fixture', fingerprint: 'fixture' }],
      };
    } else if (req.url === '/api/v1/models') {
      state.residencyRequests++;
      body = { models: [{ key: 'fixture', loaded_instances: [] }] };
    } else if (req.url === '/api/runs/owned-run' && req.method === 'DELETE') {
      state.cancellations++;
      state.run = { ...state.run, status: 'cancelled', image_job: null };
      res.writeHead(204).end();
      return;
    } else if (req.url === '/api/runs/owned-run') {
      body = state.run;
    } else if (req.url === '/api/workspaces/owned-workspace/runs' && req.method === 'POST') {
      req.resume();
      body = { id: 'followup-run', status: 'succeeded', output: '', duration_ms: 123 };
    } else if (req.url === '/api/workspaces/owned-workspace') {
      body = {
        messages: [
          { role: 'assistant', run_id: 'older-run', content: 'Incorrect older reply' },
          { role: 'user', run_id: 'followup-run', content: '19 + 23?' },
          { role: 'assistant', run_id: 'followup-run', content: state.followupReply },
        ],
      };
    } else {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify(body));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    await rm(folder, { recursive: true, force: true });
  });
  const origin = `http://127.0.0.1:${server.address().port}`;
  return {
    state,
    options: {
      url: origin,
      backend: 'lmstudio',
      profile: 'fixture',
      report: join(folder, 'report.json'),
      'lmstudio-url': origin,
      'gpu-index': 'none',
      repeat: '3',
    },
  };
}

test('unknown GPU memory is never recorded as zero or a feasible budget', () => {
  assert.throws(() => parseGpu('GPU, 8192, N/A, 0'), /unavailable/);
  assert.throws(() => parseGpu('GPU, 8192, , 0'), /unavailable/);
  assert.throws(() => parseGpu('GPU, 8192, 9000, 0'), /unavailable/);
  assert.deepEqual(parseGpu('GPU, 8192, 0, N/A'), {
    name: 'GPU',
    total_mib: 8192,
    used_mib: 0,
    utilization_percent: null,
  });
});

test('failure after admission cancels only its owned run, saves the failure and never retries', async (t) => {
  const run = { id: 'owned-run', status: 'running', image_job: null, model_rounds: 1 };
  const { options, state } = await fixture(t, run);
  let calls = 0;
  const report = await runHandoff(options, async (_, observe) => {
    calls++;
    observe(run);
    throw new Error('Browser disconnected after admission');
  });
  assert.equal(calls, 1);
  assert.equal(state.cancellations, 1);
  assert.equal(report.passed, false);
  assert.equal(report.cases.length, 1);
  assert.deepEqual(report.planned_cases, ['image-1', 'image-2', 'image-3']);
  assert.deepEqual(report.skipped_cases, ['image-2', 'image-3']);
  const saved = JSON.parse(await readFile(options.report, 'utf8'));
  assert.equal(saved.cases[0].receipt.status, 'cancelled');
  assert.match(saved.cases[0].error, /Browser disconnected/);
  assert.equal(saved.cases[0].sampled_peak_gpu_mib, null);
});

test('uncertain remote cleanup stops subsequent cases and remains visible', async (t) => {
  const run = {
    id: 'owned-run',
    status: 'cancelled',
    model_rounds: 1,
    image_job: { id: 'owned-image', status: 'unresolved' },
  };
  const { options, state } = await fixture(t, run);
  let calls = 0;
  const report = await runHandoff(options, async (_, observe) => {
    calls++;
    observe(run);
    throw new Error('Remote outcome is unknown');
  });
  assert.equal(calls, 1);
  assert.equal(state.cancellations, 0);
  assert.equal(report.passed, false);
  assert.equal(report.cases[0].cleanup_pending, true);
  assert.equal(report.cases[0].receipt.image_job.status, 'unresolved');
});

test('an existing report is preserved and prevents any inference', async (t) => {
  const { options } = await fixture(t, null);
  await writeFile(options.report, 'keep this evidence');
  let calls = 0;
  await assert.rejects(
    runHandoff(options, async () => {
      calls++;
    }),
    { code: 'EEXIST' },
  );
  assert.equal(calls, 0);
  assert.equal(await readFile(options.report, 'utf8'), 'keep this evidence');
});

test('compatible runtime works without assuming LM Studio or probing its native API', async (t) => {
  const run = { id: 'owned-run', status: 'succeeded', model_rounds: 2, image_job: null };
  const { options, state } = await fixture(t, run);
  options.backend = state.backend = 'compatible';
  options.repeat = '1';
  delete options['lmstudio-url'];
  const report = await runHandoff(options, async (_, observe) => {
    observe(run);
    return {};
  });
  assert.equal(report.passed, true);
  assert.equal(state.residencyRequests, 0);
  assert.equal(report.lmstudio_origin, null);
  assert.equal(report.cases[0].lmstudio_before.status, 'not_probed');
  assert.equal(report.cases[0].lmstudio_after.status, 'not_probed');
});

for (const correct of [true, false]) {
  test(`post-cancellation chat checks its saved reply; correct=${correct}`, async (t) => {
    const { options, state } = await fixture(t, null);
    options.repeat = '1';
    options.cancel = true;
    if (!correct) state.followupReply = '41';
    const report = await runHandoff(options, async (values, observe) => {
      state.run = {
        id: 'owned-run',
        workspace_id: 'owned-workspace',
        model_rounds: 2,
        status: values.cancel ? 'cancelled' : 'succeeded',
        image_job: {
          id: 'owned-image',
          status: values.cancel ? 'cancelled' : 'succeeded',
          output_image: null,
        },
      };
      observe(state.run);
      return {};
    });
    assert.equal(report.passed, correct);
    assert.equal(report.cases[1].followup.passed, correct);
    assert.equal(report.cases[1].followup.run_id, 'followup-run');
    if (!correct) assert.match(report.cases[1].error, /arithmetic was incorrect/);
  });
}
