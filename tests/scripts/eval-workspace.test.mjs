import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { runWorkspaceEval } from '../../scripts/eval-workspace.mjs';

async function fixture(
  t,
  { wrongFiles = false, changedBase = false, running = false, hosted = false } = {},
) {
  const state = { submissions: [], cancellations: 0, created: 0, requests: 0 };
  const run = {
    id: 'owned-run',
    status: running ? 'running' : 'succeeded',
    provider: 'local',
    model: 'fixture-model',
    model_route: { profile_id: 'fixture' },
    model_rounds: 3,
  };
  const initial = [{ path: 'scratch.txt', content: 'Temporary notes.\n' }];
  const server = createServer(async (req, res) => {
    state.requests++;
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const input = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : null;
    let body;
    if (req.url === '/api/status') {
      body = {
        guest_mode: false,
        backends: [{ id: 'local', kind: hosted ? 'openai' : 'compatible' }],
        model_catalog: {
          profiles: [{ id: 'fixture', backend: 'local', model: 'fixture-model', tool_calls: true }],
        },
      };
    } else if (req.url === '/api/workspaces' && req.method === 'POST') {
      state.created++;
      body = { id: 'owned-workspace' };
    } else if (req.url === '/api/workspaces/owned-workspace/proposals') {
      assert.deepEqual(input.changes, [{ op: 'write', ...initial[0] }]);
      body = { id: 'setup-proposal' };
    } else if (req.url === '/api/proposals/setup-proposal/apply') {
      body = { id: 'base', files: initial };
    } else if (req.url === '/api/workspaces/owned-workspace/runs' && req.method === 'POST') {
      state.submissions.push(input);
      body = run;
    } else if (req.url === '/api/runs/owned-run' && req.method === 'DELETE') {
      state.cancellations++;
      res.writeHead(204).end();
      return;
    } else if (req.url === '/api/runs/owned-run') {
      body = run;
    } else if (req.url === '/api/workspaces/owned-workspace') {
      body = {
        revisions: [
          { id: 'base', files: changedBase ? [] : initial },
          { id: 'result', run_id: run.id, files: wrongFiles ? initial : [] },
        ],
        proposals: [{ run_id: run.id, status: 'applied', applied_revision_id: 'result' }],
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
  });
  return {
    state,
    options: {
      url: `http://127.0.0.1:${server.address().port}`,
      profile: 'fixture',
      caseFile: new URL('../../evals/cases/workspace-delete-last-file.json', import.meta.url),
    },
  };
}

test('task evaluation verifies published files and provenance, using only the selected local profile', async (t) => {
  const { options, state } = await fixture(t);
  const result = await runWorkspaceEval(options);
  assert.equal(result.passed, true, result.error);
  assert.equal(result.model_rounds, 3);
  assert.equal(result.tool_failures, null);
  assert.equal(result.workspace_id, 'owned-workspace');
  assert.equal(state.submissions.length, 1);
  assert.deepEqual(state.submissions[0].model_selection, {
    mode: 'profile',
    profile_id: 'fixture',
  });
  assert.equal(state.submissions[0].workspace_tools, true);
  assert.equal(state.submissions[0].node_scope, undefined);
  assert.equal(state.cancellations, 0);
});

test('a successful model run with wrong saved files fails the task evaluation', async (t) => {
  const { options } = await fixture(t, { wrongFiles: true });
  const result = await runWorkspaceEval(options);
  assert.equal(result.status, 'succeeded');
  assert.equal(result.passed, false);
  assert.match(result.error, /scratch.txt should be absent/);
});

test('task evaluation detects a mutated historical revision', async (t) => {
  const { options } = await fixture(t, { changedBase: true });
  const result = await runWorkspaceEval(options);
  assert.equal(result.passed, false);
  assert.match(result.error, /old revision changed/);
});

test('an evaluation timeout cancels its owned run once without retrying generation', async (t) => {
  const { options, state } = await fixture(t, { running: true });
  const result = await runWorkspaceEval({ ...options, timeoutMs: 1 });
  assert.equal(result.passed, false);
  assert.match(result.error, /deadline exceeded/);
  assert.equal(state.submissions.length, 1);
  assert.equal(state.cancellations, 1);
});

test('the local evaluation runner rejects a hosted profile before creating any workspace', async (t) => {
  const { options, state } = await fixture(t, { hosted: true });
  await assert.rejects(runWorkspaceEval(options), /local-runtime profile/);
  assert.equal(state.created, 0);
  assert.equal(state.submissions.length, 0);
});

test('an existing report prevents all network requests and remains unchanged', async (t) => {
  const { options, state } = await fixture(t);
  const folder = await mkdtemp(join(tmpdir(), 'deep-workspace-eval-'));
  t.after(() => rm(folder, { recursive: true, force: true }));
  const report = join(folder, 'report.json');
  await writeFile(report, 'Keep the earlier evaluation.');
  await assert.rejects(
    promisify(execFile)(process.execPath, [
      fileURLToPath(new URL('../../scripts/eval-workspace.mjs', import.meta.url)),
      '--url',
      options.url,
      '--profile',
      options.profile,
      '--report',
      report,
    ]),
    /EEXIST/,
  );
  assert.equal(state.requests, 0);
  assert.equal(await readFile(report, 'utf8'), 'Keep the earlier evaluation.');
});
