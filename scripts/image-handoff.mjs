import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import { parseArgs, promisify } from 'node:util';
import { setTimeout as delay } from 'node:timers/promises';
import { runImageSmoke } from './image-smoke.mjs';

const execute = promisify(execFile);
const activeImages = ['planned', 'submitting', 'running', 'cancelling', 'unresolved'];
const activeParents = ['queued', 'running'];

function loopback(value) {
  const url = new URL(value);
  assert(
    url.protocol === 'http:' &&
      ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname) &&
      !url.username &&
      !url.password &&
      !url.search &&
      !url.hash &&
      url.pathname === '/',
    'Use plain loopback HTTP origins.',
  );
  return url.origin;
}

async function request(origin, path, method = 'GET', body) {
  const response = await fetch(`${origin}${path}`, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(10_000),
  });
  assert(response.ok, `${method} ${path}: HTTP ${response.status}`);
  return response.status === 204 ? null : response.json();
}

async function residency(origin, model) {
  if (!origin) return { status: 'not_probed', reason: 'No LM Studio residency probe selected.' };
  try {
    const body = await request(origin, '/api/v1/models');
    const entry = body.models.find((entry) => entry.key === model);
    assert(entry, 'Configured model is missing from LM Studio.');
    return {
      model: entry.key,
      quantization: entry.quantization?.name,
      size_bytes: entry.size_bytes,
      loaded_instances: entry.loaded_instances,
    };
  } catch (error) {
    return { error: error.message };
  }
}

export function parseGpu(stdout) {
  const fields = stdout
    .trim()
    .split(',')
    .map((field) => field.trim());
  assert.equal(fields.length, 4, 'Unexpected nvidia-smi CSV.');
  const [name, total, used, utilization] = fields;
  const number = (value) => (value !== '' && Number.isFinite(Number(value)) ? Number(value) : null);
  const total_mib = number(total);
  const used_mib = number(used);
  assert(
    total_mib > 0 && used_mib !== null && used_mib >= 0 && used_mib <= total_mib,
    'GPU memory measurement unavailable.',
  );
  return { name, total_mib, used_mib, utilization_percent: number(utilization) };
}

async function gpuSampler(index, report) {
  let pending;
  const sample = () => {
    if (pending || report.gpu.error) return pending;
    pending = execute(
      'nvidia-smi',
      [
        `--id=${index}`,
        '--query-gpu=name,memory.total,memory.used,utilization.gpu',
        '--format=csv,noheader,nounits',
      ],
      { timeout: 5_000, maxBuffer: 8192, windowsHide: true },
    )
      .then(({ stdout }) => report.gpu.samples.push({ at_ms: Date.now(), ...parseGpu(stdout) }))
      .catch(() => {
        report.gpu.error = 'nvidia-smi unavailable or returned invalid measurements.';
      })
      .finally(() => {
        pending = undefined;
      });
    return pending;
  };
  await sample();
  const timer = setInterval(sample, 1000);
  return async () => {
    clearInterval(timer);
    await pending;
    await sample();
  };
}

// Failure cleanup targets only this sample's accepted receipt. Never resubmit generation.
async function settle(origin, run) {
  let current = await request(origin, `/api/runs/${run.id}`);
  if (activeParents.includes(current.status)) {
    await request(origin, `/api/runs/${run.id}`, 'DELETE');
  }
  const deadline = Date.now() + 35_000;
  do {
    current = await request(origin, `/api/runs/${run.id}`);
    if (
      !activeParents.includes(current.status) &&
      !activeImages.includes(current.image_job?.status)
    )
      return current;
    if (current.image_job?.status === 'unresolved') return current;
    await delay(250);
  } while (Date.now() < deadline);
  return current;
}

async function chatAfterCancellation(origin, backend, cancelled, result) {
  const start = Date.now();
  let run = await request(origin, `/api/workspaces/${cancelled.workspace_id}/runs`, 'POST', {
    backend,
    mode: 'chat',
    prompt: 'What is 19 + 23? Answer with only the number.',
  });
  try {
    while (activeParents.includes(run.status)) {
      assert(Date.now() - start < 120_000, 'Post-cancellation chat timed out.');
      await delay(250);
      run = await request(origin, `/api/runs/${run.id}`);
    }
    assert.equal(run.status, 'succeeded', run.error ?? 'Post-cancellation chat failed.');
    const detail = await request(origin, `/api/workspaces/${cancelled.workspace_id}`);
    const reply = detail.messages.find(
      (message) => message.role === 'assistant' && message.run_id === run.id,
    );
    assert.equal(reply?.content.trim(), '42', 'Post-cancellation arithmetic was incorrect.');
    const previous = await request(origin, `/api/runs/${cancelled.id}`);
    assert.equal(previous.image_job.status, 'cancelled');
    assert.equal(
      previous.image_job.output_image,
      null,
      'An image was published after cancellation.',
    );
    result.passed = true;
  } finally {
    Object.assign(result, {
      run_id: run.id,
      status: run.status,
      elapsed_ms: Date.now() - start,
      queue_ms: run.queue_ms,
      duration_ms: run.duration_ms,
      stats: run.stats,
    });
    if (activeParents.includes(run.status)) await settle(origin, run);
  }
}

export async function runHandoff(values, smoke = runImageSmoke) {
  assert(
    values.url && values.backend && values.profile && values.report,
    'Required: --url <Deep URL> --backend <id> --profile <id> --report <new JSON file>',
  );
  const origin = loopback(values.url);
  const repeat = Number(values.repeat ?? 2);
  const gpuIndex = values['gpu-index'] ?? '0';
  assert(Number.isInteger(repeat) && repeat >= 1 && repeat <= 10, '--repeat must be 1–10.');
  assert(
    gpuIndex === 'none' || /^\d+$/.test(gpuIndex),
    '--gpu-index must be a nonnegative integer or none.',
  );
  const status = await request(origin, '/api/status');
  assert(
    !status.guest_mode,
    'Use an isolated local operator instance, without DEEP_PUBLIC_ORIGIN.',
  );
  const backend = status.backends.find((entry) => entry.id === values.backend);
  assert(
    backend && ['lmstudio', 'compatible'].includes(backend.id),
    'Select a configured lmstudio or compatible backend.',
  );
  const lmstudio =
    backend.id === 'lmstudio' || values['lmstudio-url'] !== undefined
      ? loopback(values['lmstudio-url'] ?? 'http://127.0.0.1:1234')
      : null;
  const profile = status.image_profiles.find((entry) => entry.id === values.profile);
  assert(profile, 'Image profile is not configured.');
  const cases = Array.from({ length: repeat }, (_, index) => `image-${index + 1}`);
  if (values.cancel) cases.push('cancel-then-chat');
  const report = {
    started_at: new Date().toISOString(),
    deep_origin: origin,
    lmstudio_origin: lmstudio,
    backend,
    profile,
    planned_cases: cases,
    methodology: {
      generation_retries: 0,
      polling_ms: 200,
      gpu_sampling_target_ms: 1000,
      gpu_scope: 'Whole GPU, including other processes; sampled peaks are lower bounds.',
      residency:
        'LM Studio residency is optional. Other compatible runtimes remain not_probed. No forced load, unload or residency guarantees.',
      timing: 'Run/job timings are server receipts. Timeline entries are polling observations.',
      quality:
        'Tool use, saved bytes, browser decoding, reload and nonempty reply; image meaning and reply truthfulness need human review.',
    },
    gpu: { index: gpuIndex, samples: [] },
    cases: [],
  };
  // Reserve before any model work. Preserve existing reports and save failures too.
  await writeFile(values.report, JSON.stringify(report, null, 2) + '\n', { flag: 'wx' });
  const stopGpu = gpuIndex === 'none' ? async () => {} : await gpuSampler(gpuIndex, report);
  try {
    for (const name of cases) {
      const sample = { name, started_at_ms: Date.now(), timeline: [], passed: false };
      report.cases.push(sample);
      sample.lmstudio_before = await residency(lmstudio, backend.model);
      let latest;
      let lastKey;
      try {
        sample.result = await smoke(
          {
            url: origin,
            backend: backend.id,
            profile: profile.id,
            cancel: name === 'cancel-then-chat',
          },
          (run) => {
            latest = run;
            sample.run_id = run.id;
            const event = {
              run_status: run.status,
              model_rounds: run.model_rounds,
              image_status: run.image_job?.status ?? null,
              progress: run.image_job?.progress ?? null,
            };
            const key = JSON.stringify(event);
            if (key !== lastKey) {
              sample.timeline.push({
                observed_after_ms: Date.now() - sample.started_at_ms,
                ...event,
              });
              lastKey = key;
            }
          },
        );
        assert(latest, 'Smoke test returned without an accepted run.');
        if (name === 'cancel-then-chat') {
          sample.followup = { passed: false };
          await chatAfterCancellation(origin, backend.id, latest, sample.followup);
        }
        sample.passed = true;
      } catch (error) {
        sample.error = error.message;
      } finally {
        if (latest) {
          try {
            latest = await settle(origin, latest);
            sample.receipt = {
              status: latest.status,
              error: latest.error,
              model_rounds: latest.model_rounds,
              queue_ms: latest.queue_ms,
              duration_ms: latest.duration_ms,
              stats: latest.stats,
              image_job: latest.image_job && {
                id: latest.image_job.id,
                status: latest.image_job.status,
                error: latest.image_job.error,
                created_at_ms: latest.image_job.created_at_ms,
                finished_at_ms: latest.image_job.finished_at_ms,
                seed: latest.image_job.seed,
                progress: latest.image_job.progress,
              },
            };
            if (
              activeParents.includes(latest.status) ||
              activeImages.includes(latest.image_job?.status)
            ) {
              sample.passed = false;
              sample.cleanup_pending = true;
            }
          } catch (error) {
            sample.passed = false;
            sample.cleanup_error = error.message;
          }
        }
        sample.lmstudio_after = await residency(lmstudio, backend.model);
        sample.finished_at_ms = Date.now();
        await writeFile(values.report, JSON.stringify(report, null, 2) + '\n');
      }
      console.log(
        JSON.stringify({
          name,
          passed: sample.passed,
          run_id: sample.run_id,
          duration_ms: sample.receipt?.duration_ms,
          error: sample.error,
          cleanup_pending: sample.cleanup_pending,
        }),
      );
      // A failed or uncertain case must not be hidden by a later successful retry.
      if (!sample.passed) break;
    }
  } finally {
    await stopGpu();
    report.finished_at = new Date().toISOString();
    report.skipped_cases = cases.slice(report.cases.length);
    report.passed =
      report.cases.length === repeat + (values.cancel ? 1 : 0) &&
      report.cases.every((sample) => sample.passed);
    for (const sample of report.cases) {
      const measurements = report.gpu.samples.filter(
        (point) => point.at_ms >= sample.started_at_ms && point.at_ms <= sample.finished_at_ms,
      );
      sample.sampled_peak_gpu_mib = measurements.length
        ? Math.max(...measurements.map((point) => point.used_mib))
        : null;
    }
    await writeFile(values.report, JSON.stringify(report, null, 2) + '\n');
  }
  return report;
}

if (import.meta.main) {
  const { values } = parseArgs({
    options: {
      url: { type: 'string' },
      backend: { type: 'string' },
      profile: { type: 'string' },
      report: { type: 'string' },
      repeat: { type: 'string' },
      'lmstudio-url': { type: 'string' },
      'gpu-index': { type: 'string' },
      cancel: { type: 'boolean', default: false },
    },
  });
  runHandoff(values)
    .then((report) => {
      process.exitCode = report.passed ? 0 : 1;
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
