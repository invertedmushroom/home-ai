import assert from 'node:assert/strict';
import { open, readFile } from 'node:fs/promises';
import { parseArgs } from 'node:util';

const { values } = parseArgs({
  options: {
    url: { type: 'string', default: 'http://127.0.0.1:13000' },
    report: { type: 'string' },
    'require-runtime-logs': { type: 'boolean', default: false },
  },
});
const origin = new URL(values.url);
assert(
  origin.protocol === 'http:' &&
    ['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname) &&
    !origin.username &&
    !origin.password &&
    origin.pathname === '/' &&
    !origin.search &&
    !origin.hash,
  'Use a loopback HTTP Grafana origin.',
);
const file = values.report ? await open(values.report, 'wx') : null;
const report = { passed: false, checked_at: new Date().toISOString(), queries: [], runtimes: [] };
try {
  const dashboard = JSON.parse(
    await readFile(new URL('../deploy/observability/deep-overview.json', import.meta.url), 'utf8'),
  );
  const panels = (items) => items.flatMap((panel) => [panel, ...panels(panel.panels ?? [])]);
  const interpolate = (query) =>
    query
      .replaceAll('${instance:regex}', '.*')
      .replaceAll('${runtime:regex}', 'llama-server|wangp')
      .replaceAll('${log_search:doublequote}', '""')
      .replaceAll('$__rate_interval', '1m');
  const now = Date.now();
  const runtimes = new Set();
  const checks = panels(dashboard.panels);
  if (values['require-runtime-logs']) {
    // Probe each feed separately: a busy runtime can fill the combined panel's limit.
    for (const name of ['llama-server', 'wangp']) {
      checks.push({
        title: `Verify ${name} log feed`,
        datasource: { type: 'loki', uid: 'loki' },
        targets: [
          {
            refId: 'A',
            expr: `{service_namespace="deep-runtime",service_name="${name}"}`,
            queryType: 'range',
            maxLines: 1,
          },
        ],
      });
    }
  }
  for (const panel of checks) {
    if (!panel.targets?.length) continue;
    const queries = panel.targets.map((query) => ({
      ...query,
      datasource: query.datasource ?? panel.datasource,
      ...(query.expr ? { expr: interpolate(query.expr) } : {}),
      ...(query.query ? { query: interpolate(query.query) } : {}),
      intervalMs: 5000,
      maxDataPoints: 360,
    }));
    const response = await fetch(new URL('/api/ds/query', origin), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ queries, from: String(now - 30 * 60_000), to: String(now) }),
      signal: AbortSignal.timeout(15_000),
    });
    assert(response.ok, `${panel.title}: Grafana returned HTTP ${response.status}.`);
    const data = await response.json();
    for (const query of queries) {
      const result = data.results?.[query.refId];
      const row = {
        panel: panel.title,
        ref: query.refId,
        datasource: query.datasource.type,
        status: result?.status,
        rows: (result?.frames ?? []).reduce(
          (n, frame) => n + (frame.data?.values?.[0]?.length ?? 0),
          0,
        ),
      };
      report.queries.push(row);
      assert(
        result && !result.error && (result.status ?? 200) < 400,
        `${panel.title}/${query.refId}: ${result?.error ?? 'No successful query result.'}`,
      );
      for (const frame of result.frames ?? []) {
        const labels = frame.schema.fields.findIndex((field) => field.name === 'labels');
        if (query.datasource.type === 'loki' && labels >= 0) {
          for (const value of frame.data.values[labels] ?? []) {
            if (value.service_namespace === 'deep-runtime') runtimes.add(value.service_name);
          }
        }
      }
    }
  }
  report.runtimes = [...runtimes].sort();
  if (values['require-runtime-logs']) {
    for (const name of ['llama-server', 'wangp'])
      assert(runtimes.has(name), `No ${name} log arrived in the last 30 minutes.`);
  }
  report.passed = true;
} catch (error) {
  report.error = String(error);
  process.exitCode = 1;
} finally {
  if (file) {
    await file.writeFile(`${JSON.stringify(report, null, 2)}\n`);
    await file.close();
  }
  console.log(
    JSON.stringify({
      passed: report.passed,
      queries: report.queries.length,
      runtimes: report.runtimes,
      error: report.error,
    }),
  );
}
