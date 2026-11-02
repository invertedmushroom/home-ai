import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { open, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const active = (run) => run && ['queued', 'running'].includes(run.status);

// A case checks task outcomes; successful inference alone never passes a case.
export async function runWorkspaceEval({ url, profile, caseFile, timeoutMs = 600_000 }) {
  const base = new URL(url);
  assert(
    base.protocol === 'http:' &&
      ['127.0.0.1', '[::1]', 'localhost'].includes(base.hostname) &&
      !base.username &&
      !base.password &&
      !base.search &&
      !base.hash &&
      base.pathname === '/',
    'Use a plain loopback HTTP Deep URL.',
  );
  assert(Number.isFinite(timeoutMs) && timeoutMs > 0, 'Use a positive timeout.');
  const task = JSON.parse(await readFile(caseFile, 'utf8'));
  assert(
    task.id && task.prompt && task.setup?.files && task.checks?.files,
    'The case needs an id, prompt, setup.files and checks.files.',
  );
  assert(!task.setup.assets?.length, 'This runner handles workspace text files only.');
  for (const rule of Object.values(task.checks.files)) {
    assert(
      rule === null ||
        typeof rule === 'string' ||
        (typeof rule === 'object' && rule.nonempty === true && Object.keys(rule).length === 1),
      'File checks must be exact text, null (absent), or {nonempty:true}.',
    );
  }

  async function request(path, method = 'GET', body) {
    const response = await fetch(new URL(`/api${path}`, base), {
      method,
      redirect: 'error',
      signal: AbortSignal.timeout(15_000),
      headers: { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    assert(response.ok, `${method} ${path} returned HTTP ${response.status}.`);
    return response.status === 204 ? null : response.json();
  }

  const status = await request('/status');
  assert(status.guest_mode === false, 'Use a local operator installation.');
  const selected = status.model_catalog?.profiles.find((p) => p.id === profile);
  const backend = status.backends.find((b) => b.id === selected?.backend);
  assert(
    selected?.tool_calls && ['compatible', 'lmstudio'].includes(backend?.kind),
    'Choose a configured local-runtime profile declaring tool_calls support.',
  );
  const report = {
    case: task.id,
    profile,
    provider: selected.backend,
    model: selected.model,
    passed: false,
    workspace_id: null,
    run_id: null,
    model_rounds: null,
    tool_failures: null,
    user_interventions: 0,
    note: 'Per-call file-tool failures are not persisted by Deep; tool_failures is unknown.',
  };
  const started = Date.now();
  let run;
  try {
    const workspace = await request('/workspaces', 'POST', { title: `Eval ${task.id}` });
    report.workspace_id = workspace.id;
    const initial = Object.entries(task.setup.files).map(([path, content]) => ({ path, content }));
    let baseRevision = null;
    if (initial.length) {
      const proposal = await request(`/workspaces/${workspace.id}/proposals`, 'POST', {
        id: randomUUID(),
        base_revision: { id: null },
        changes: initial.map((file) => ({ op: 'write', ...file })),
      });
      baseRevision = await request(`/proposals/${proposal.id}/apply`, 'POST');
    }
    run = await request(`/workspaces/${workspace.id}/runs`, 'POST', {
      prompt: task.prompt,
      mode: 'chat',
      workspace_tools: true,
      model_selection: { mode: 'profile', profile_id: profile },
    });
    report.run_id = run.id;
    const deadline = Date.now() + timeoutMs;
    while (active(run)) {
      assert(Date.now() < deadline, 'Evaluation deadline exceeded.');
      await new Promise((r) => setTimeout(r, 250));
      run = await request(`/runs/${run.id}`);
    }
    assert.equal(run.status, 'succeeded', run.error || `Run ended ${run.status}.`);
    assert.equal(run.provider, selected.backend);
    assert.equal(run.model, selected.model);
    assert.equal(run.model_route?.profile_id, profile);
    const detail = await request(`/workspaces/${workspace.id}`);
    const revision = detail.revisions.at(-1);
    const before = baseRevision ? 1 : 0;
    const delta = task.checks.revision_delta ?? 1;
    assert.equal(detail.revisions.length, before + delta, 'Unexpected number of revisions.');
    const files = new Map((revision?.files || []).map((f) => [f.path, f.content]));
    const expectedPaths = new Set(Object.keys(task.setup.files));
    for (const [path, rule] of Object.entries(task.checks.files)) {
      if (rule === null) {
        expectedPaths.delete(path);
        assert(!files.has(path), `${path} should be absent.`);
      } else {
        expectedPaths.add(path);
        assert(files.has(path), `${path} was not saved.`);
        if (typeof rule === 'string')
          assert.equal(files.get(path), rule, `${path} has wrong contents.`);
        else assert(files.get(path).trim().length > 0, `${path} is empty.`);
      }
    }
    assert.deepEqual(
      [...files.keys()].sort(),
      [...expectedPaths].sort(),
      'Unexpected project paths.',
    );
    for (const file of initial) {
      if (!Object.hasOwn(task.checks.files, file.path))
        assert.equal(files.get(file.path), file.content, `Unrelated ${file.path} changed.`);
    }
    if (baseRevision) {
      const savedBase = detail.revisions.find((r) => r.id === baseRevision.id);
      const byPath = (a, b) => a.path.localeCompare(b.path);
      assert.deepEqual(
        [...savedBase.files].sort(byPath),
        [...initial].sort(byPath),
        'The old revision changed.',
      );
    }
    if (delta === 1) {
      assert.equal(revision.run_id, run.id, 'Revision belongs to another run.');
      assert(
        detail.proposals.some(
          (p) =>
            p.run_id === run.id && p.status === 'applied' && p.applied_revision_id === revision.id,
        ),
        'Missing atomic applied-proposal receipt.',
      );
    }
    report.passed = true;
  } catch (error) {
    report.error = error.message;
  } finally {
    if (active(run)) {
      try {
        await request(`/runs/${run.id}`, 'DELETE');
      } catch (error) {
        report.cleanup_error = error.message;
      }
    }
    report.status = run?.status ?? null;
    report.model_rounds = run?.model_rounds ?? null;
    report.stats = run?.stats ?? null;
    report.elapsed_ms = Date.now() - started;
  }
  return report;
}

async function main() {
  const { values } = parseArgs({
    options: {
      url: { type: 'string' },
      profile: { type: 'string' },
      case: { type: 'string', multiple: true },
      report: { type: 'string' },
    },
  });
  assert(
    values.url && values.profile,
    'Usage: node scripts/eval-workspace.mjs --url <Deep URL> --profile <local profile ID> [--case <case ID>] [--report <new file>]',
  );
  const ids = values.case || [
    'workspace-targeted-refactor',
    'workspace-literal-search',
    'workspace-delete-last-file',
  ];
  assert(
    ids.every((id) => /^[a-z0-9-]+$/.test(id)),
    'Use case IDs from evals/cases.',
  );
  const report = { started_at: new Date().toISOString(), cases: [], skipped_cases: [] };
  const output = values.report ? await open(values.report, 'wx') : null;
  try {
    for (const [index, id] of ids.entries()) {
      let result;
      try {
        result = await runWorkspaceEval({
          url: values.url,
          profile: values.profile,
          caseFile: new URL(`../evals/cases/${id}.json`, import.meta.url),
        });
      } catch (error) {
        report.error = error.message;
        report.skipped_cases = ids.slice(index);
        break;
      }
      report.cases.push(result);
      if (!result.passed) {
        report.skipped_cases = ids.slice(index + 1);
        break;
      }
    }
    report.passed = report.cases.length === ids.length && report.cases.every((r) => r.passed);
    const serialized = `${JSON.stringify(report, null, 2)}\n`;
    if (output) await output.writeFile(serialized);
    console.log(serialized.trimEnd());
    if (!report.passed) process.exitCode = 1;
  } finally {
    await output?.close();
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
