import assert from 'node:assert/strict';
import { open } from 'node:fs/promises';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

function origin(value) {
  const url = new URL(value);
  assert(
    url.protocol === 'http:' &&
      ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname) &&
      !url.username &&
      !url.password &&
      url.pathname === '/' &&
      !url.search &&
      !url.hash,
    'Use loopback HTTP origins for Deep, Tempo and Prometheus.',
  );
  return url.origin;
}

async function request(base, path, method = 'GET', body) {
  const response = await fetch(new URL(path, base), {
    method,
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(5000),
  });
  assert(response.ok, `Diagnostic request returned HTTP ${response.status}.`);
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const active = (run) => ['queued', 'running'].includes(run.status);
const attributes = (values) =>
  Object.fromEntries(
    (values ?? []).map((item) => [
      item.key,
      item.value?.stringValue ?? item.value?.intValue ?? item.value?.boolValue,
    ]),
  );

function runTrace(trace, id) {
  for (const batch of trace.batches ?? trace.resourceSpans ?? trace.trace?.resourceSpans ?? []) {
    const resource = attributes(batch.resource?.attributes);
    for (const scope of batch.scopeSpans ?? batch.instrumentationLibrarySpans ?? []) {
      for (const span of scope.spans ?? []) {
        if (span.name === 'run' && attributes(span.attributes).run_id === id) {
          assert.equal(resource['service.name'], 'deep-server');
          assert.match(resource['service.instance.id'] ?? '', /^[a-f0-9-]{36}$/);
          return { instance: resource['service.instance.id'], root: span };
        }
      }
    }
  }
  return null;
}

async function metrics(prometheus, query) {
  const response = await request(prometheus, `/api/v1/query?query=${encodeURIComponent(query)}`);
  assert.equal(response.status, 'success');
  assert.equal(response.data.resultType, 'vector');
  return response.data.result;
}

export async function runObservabilityEvidence(values) {
  assert(
    values.url && values.backend && values.report,
    'Usage: node scripts/observability-evidence.mjs --url http://127.0.0.1:4321 --backend compatible --report <new.json> [--profile flux-klein-4b]',
  );
  assert(
    ['demo', 'compatible', 'lmstudio'].includes(values.backend),
    'Select a configured local backend explicitly.',
  );
  const deep = origin(values.url);
  const tempo = origin(values.tempo ?? 'http://127.0.0.1:13200');
  const prometheus = origin(values.prometheus ?? 'http://127.0.0.1:19090');
  const file = await open(values.report, 'wx');
  const report = {
    passed: false,
    started_at: new Date().toISOString(),
    deep_origin: deep,
    backend: values.backend,
  };
  let accepted;
  try {
    const status = await request(deep, '/api/status');
    assert.equal(status.guest_mode, false, 'Use an isolated local operator listener for evidence.');
    assert(
      status.backends.some((backend) => backend.id === values.backend),
      'Selected backend is not configured.',
    );
    if (values.profile) {
      assert.notEqual(values.backend, 'demo');
      assert(
        status.image_profiles.some((profile) => profile.id === values.profile),
        'Selected image profile is not configured.',
      );
    }
    const selector = `service_name="deep-server",backend=${JSON.stringify(values.backend)},outcome="succeeded"`;
    const before = await metrics(prometheus, `deep_run_finished_total{${selector}}`);
    const previous = new Map(
      before.map((series) => [series.metric.service_instance_id, Number(series.value[1])]),
    );
    const workspace = await request(deep, '/api/workspaces', 'POST', {
      title: 'Observability evidence',
    });
    // Leave room around Tempo's recent-data search boundary, including short runs.
    const traceStart = Math.floor(Date.now() / 1000) - 120;
    accepted = await request(deep, `/api/workspaces/${workspace.id}/runs`, 'POST', {
      backend: values.backend,
      mode: 'chat',
      prompt: values.profile
        ? 'Use the image tool to create an illustration of a small orange robot watering a seedling on a wooden windowsill, soft morning light, no text.'
        : 'What is 19 + 23? Answer with only the number.',
      ...(values.profile ? { image_profile_id: values.profile } : {}),
    });
    report.run_id = accepted.id;
    const deadline = Date.now() + (values.profile ? 390_000 : 120_000);
    let run = accepted;
    while (active(run)) {
      assert(Date.now() < deadline, 'Generation exceeded the evidence deadline.');
      await delay(200);
      run = await request(deep, `/api/runs/${run.id}`);
    }
    report.receipt = {
      status: run.status,
      error: run.error,
      queue_ms: run.queue_ms,
      duration_ms: run.duration_ms,
      model_rounds: run.model_rounds,
      image_status: run.image_job?.status ?? null,
    };
    assert.equal(run.status, 'succeeded', run.error ?? 'Run failed.');
    if (values.profile) {
      assert.equal(run.image_job?.status, 'succeeded');
      assert.equal(run.model_rounds, 2);
    }
    const traceLookupStarted = Date.now();
    const traceDeadline = traceLookupStarted + 60_000;
    const query = `{ resource.service.name = "deep-server" && span.run_id = ${JSON.stringify(run.id)} }`;
    let found;
    while (!found) {
      assert(
        Date.now() < traceDeadline,
        'The run trace was not searchable in Tempo within 60 seconds.',
      );
      const searchParams = new URLSearchParams({
        q: query,
        start: String(traceStart),
        end: String(Math.ceil(Date.now() / 1000) + 1),
      });
      const search = await request(tempo, `/api/search?${searchParams}`);
      for (const result of search.traces ?? []) {
        assert.match(result.traceID, /^[a-f0-9]{1,32}$/i);
        const id = result.traceID.padStart(32, '0').toLowerCase();
        assert.notEqual(id, '0'.repeat(32));
        const trace = await request(tempo, `/api/traces/${id}`);
        const root = runTrace(trace, run.id);
        if (root) {
          found = { id, ...root, trace };
          break;
        }
      }
      if (!found) await delay(500);
    }
    report.trace_id = found.id;
    report.trace_lookup_ms = Date.now() - traceLookupStarted;
    report.service_instance_id = found.instance;
    const spans = (found.trace.batches ?? found.trace.resourceSpans ?? []).flatMap((batch) =>
      (batch.scopeSpans ?? batch.instrumentationLibrarySpans ?? []).flatMap(
        (scope) => scope.spans ?? [],
      ),
    );
    report.span_names = [...new Set(spans.map((span) => span.name))];
    assert.equal(attributes(found.root.attributes).outcome, 'succeeded');
    if (values.backend !== 'demo') {
      for (const name of ['resource.wait', 'model.operation', 'model_call'])
        assert(report.span_names.includes(name), `Missing ${name} in the run trace.`);
    }
    if (values.profile)
      assert(report.span_names.includes('image_job'), 'Image work detached from the run trace.');
    const metricDeadline = Date.now() + 20_000;
    const metricQuery = `deep_run_finished_total{${selector},service_instance_id=${JSON.stringify(found.instance)}}`;
    while (true) {
      const series = await metrics(prometheus, metricQuery);
      const value = Number(series[0]?.value?.[1]);
      if (Number.isFinite(value) && value >= (previous.get(found.instance) ?? 0) + 1) {
        for (const forbidden of [
          'run_id',
          'job_id',
          'operation_id',
          'workspace_id',
          'prompt',
          'response',
          'url',
          'cookie',
        ]) {
          assert(
            !Object.hasOwn(series[0].metric, forbidden),
            `Unexpected metric label ${forbidden}.`,
          );
        }
        report.metric = {
          query: metricQuery,
          before: previous.get(found.instance) ?? 0,
          after: value,
        };
        break;
      }
      assert(
        Date.now() < metricDeadline,
        'No matching outcome counter increment reached Prometheus.',
      );
      await delay(500);
    }
    const heartbeat = await metrics(
      prometheus,
      `deep_observability_snapshot_timestamp_seconds{service_name="deep-server",service_instance_id=${JSON.stringify(found.instance)}}`,
    );
    assert(
      heartbeat.length && Date.now() / 1000 - Number(heartbeat[0].value[1]) < 20,
      'The state sampler has no fresh observation.',
    );
    report.passed = true;
    return report;
  } catch (error) {
    report.error = String(error);
    if (accepted) {
      try {
        let run = await request(deep, `/api/runs/${accepted.id}`);
        if (active(run)) run = await request(deep, `/api/runs/${accepted.id}`, 'DELETE');
        report.cleanup = { parent: run.status, image: run.image_job?.status ?? null };
      } catch {
        report.cleanup = { status: 'unconfirmed' };
      }
    }
    throw error;
  } finally {
    report.finished_at = new Date().toISOString();
    await file.writeFile(`${JSON.stringify(report, null, 2)}\n`);
    await file.close();
    console.log(
      JSON.stringify({
        passed: report.passed,
        run_id: report.run_id,
        trace_id: report.trace_id,
        error: report.error,
      }),
    );
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { values } = parseArgs({
    options: {
      url: { type: 'string' },
      backend: { type: 'string' },
      report: { type: 'string' },
      profile: { type: 'string' },
      tempo: { type: 'string' },
      prometheus: { type: 'string' },
    },
  });
  await runObservabilityEvidence(values);
}
