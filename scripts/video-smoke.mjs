import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { open, rm } from 'node:fs/promises';
import { resolve, extname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { setTimeout as delay } from 'node:timers/promises';
import { WanGpClient } from '../pinokio_agent/skills/api/wan-git/clients/mcp.mjs';

const MAX_PIXELS = 832 * 480;
const MAX_DURATION = 8;
const MAX_SETTINGS_BYTES = 16 * 1024;
const activePhases = new Set([
  'loading_model',
  'encoding_text',
  'inference',
  'inference_stage_1',
  'inference_stage_2',
  'inference_stage_3',
  'decoding',
]);
const configurable = new Set([
  'model_type',
  'prompt',
  'resolution',
  'video_length',
  'force_fps',
  'num_inference_steps',
  'seed',
  'activated_loras',
  'loras_multipliers',
  'guidance_scale',
  'guidance_phases',
  'sample_solver',
  'flow_shift',
  'negative_prompt',
]);

export function videoSettings(input) {
  assert(input && typeof input === 'object' && !Array.isArray(input), 'Use one settings object.');
  assert(
    Object.keys(input).every((key) => configurable.has(key)),
    'Unsupported video probe setting.',
  );
  assert(
    typeof input.model_type === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(input.model_type),
    'Select an exact model_type.',
  );
  assert(
    typeof input.prompt === 'string' &&
      input.prompt.trim() &&
      Buffer.byteLength(input.prompt) <= 4096 &&
      !/[\x00-\x1f\x7f]|@file\s*\(/i.test(input.prompt),
    'Use a single literal prompt of 1-4096 UTF-8 bytes without controls or file references.',
  );
  const match =
    typeof input.resolution === 'string' && /^([1-9]\d*)x([1-9]\d*)$/.exec(input.resolution);
  assert(match, 'Supply explicit WIDTHxHEIGHT resolution.');
  const [width, height] = match.slice(1).map(Number);
  assert(
    [width, height].every((n) => n >= 128 && n <= 832 && n % 16 === 0) &&
      width * height <= MAX_PIXELS,
    'Video probe dimensions must align to 16, be 128-832 per side and at most 399360 pixels.',
  );
  assert(
    Number.isInteger(input.video_length) && input.video_length >= 5 && input.video_length <= 97,
    'Video probe length must be 5-97 frames.',
  );
  assert(
    typeof input.force_fps === 'string' &&
      /^(?:[89]|[12]\d|30)$/.test(input.force_fps) &&
      input.video_length / Number(input.force_fps) <= MAX_DURATION,
    'Supply explicit force_fps from 8 to 30 with at most eight seconds.',
  );
  assert(
    Number.isInteger(input.num_inference_steps) &&
      input.num_inference_steps >= 1 &&
      input.num_inference_steps <= 8,
    'Video probes permit 1-8 inference steps.',
  );
  assert(
    Number.isInteger(input.seed) && input.seed >= 0 && input.seed <= 0xffffffff,
    'Supply an explicit unsigned 32-bit seed.',
  );
  for (const key of ['guidance_scale', 'flow_shift']) {
    if (input[key] !== undefined)
      assert(Number.isFinite(input[key]) && input[key] >= 0 && input[key] <= 30, `Invalid ${key}.`);
  }
  if (input.guidance_phases !== undefined)
    assert(
      Number.isInteger(input.guidance_phases) &&
        input.guidance_phases >= 1 &&
        input.guidance_phases <= 3,
      'Invalid guidance_phases.',
    );
  if (input.sample_solver !== undefined)
    assert(
      typeof input.sample_solver === 'string' && /^[a-zA-Z0-9_-]{1,32}$/.test(input.sample_solver),
      'Invalid sample_solver.',
    );
  if (input.negative_prompt !== undefined)
    assert(
      typeof input.negative_prompt === 'string' &&
        Buffer.byteLength(input.negative_prompt) <= 4096 &&
        !/[\x00-\x1f\x7f]|@file\s*\(/i.test(input.negative_prompt),
      'Invalid negative_prompt.',
    );
  const loras = input.activated_loras ?? [];
  const strengths = input.loras_multipliers ?? '';
  assert(
    Array.isArray(loras) &&
      loras.length <= 8 &&
      loras.every(
        (file) => typeof file === 'string' && /^[a-zA-Z0-9_.-]{1,244}\.safetensors$/.test(file),
      ) &&
      new Set(loras).size === loras.length,
    'Use up to eight distinct local LoRA filenames.',
  );
  assert(typeof strengths === 'string', 'LoRA strengths must be a string.');
  const parts = strengths.trim() ? strengths.trim().split(/\s+/) : [];
  assert(
    parts.length === loras.length &&
      parts.every(
        (part) => /^-?(?:\d+(?:\.\d+)?|\.\d+)$/.test(part) && Math.abs(Number(part)) <= 10,
      ),
    'Supply one scalar strength from -10 to 10 per LoRA.',
  );
  return {
    ...input,
    activated_loras: [...loras],
    loras_multipliers: strengths,
    batch_size: 1,
    repeat_generation: 1,
    multi_prompts_gen_type: 'FG',
    image_mode: 0,
    image_prompt_type: '',
    video_prompt_type: '',
    audio_prompt_type: '',
    spatial_upsampling: '',
    temporal_upsampling: '',
    output_filename: '',
  };
}

function remoteId(value) {
  return typeof value === 'string' && /^[a-zA-Z0-9_:-]{1,128}$/.test(value);
}

function jobReceipt(value, context, request, job) {
  assert(
    value && remoteId(value.job_id) && typeof value.done === 'boolean',
    'WanGP returned an invalid job receipt.',
  );
  assert(
    value.agent_context === undefined || value.agent_context === context,
    'WanGP returned a foreign job context.',
  );
  assert(
    job ? value.job_id === job : value.request_id === request,
    'WanGP returned a foreign job or request identity.',
  );
  assert(
    value.request_id === undefined || value.request_id === request,
    'WanGP changed the request identity.',
  );
  return value;
}

function terminalKnown(job) {
  return (
    job.done &&
    typeof job.result?.success === 'boolean' &&
    typeof job.result?.cancelled === 'boolean' &&
    !(job.result.success && job.result.cancelled)
  );
}

function activePhase(job) {
  return job.progress?.task_id === `mcp_${job.job_id}_1` && activePhases.has(job.progress.phase)
    ? job.progress.phase
    : undefined;
}

function publicJob(job) {
  return {
    job_id: job.job_id,
    done: job.done,
    phase: activePhase(job),
    success: terminalKnown(job) ? job.result.success : undefined,
    cancelled: terminalKnown(job) ? job.result.cancelled : undefined,
  };
}

function finalVideo(job) {
  assert(
    job.done && job.result.success === true && job.result.cancelled === false,
    'WanGP did not complete the video successfully.',
  );
  const items = job.result.gallery_items;
  assert(Array.isArray(items) && items.length === 1, 'WanGP must return exactly one video output.');
  const [item] = items;
  assert(
    item.media_type === 'video' &&
      item.role === 'final' &&
      item.task_id === `mcp_${job.job_id}_1` &&
      remoteId(item.media_id),
    'WanGP video is not bound to the exact final task.',
  );
  return item.media_id;
}

function checkModel(value, settings) {
  const m = value?.metadata;
  assert(
    m?.model_type === settings.model_type &&
      Array.isArray(m.main_output) &&
      m.main_output.includes('video') &&
      Array.isArray(m.capabilities) &&
      m.capabilities.includes('text_to_video'),
    'Select a model that advertises text-to-video.',
  );
  assert(
    Number.isInteger(m.frames_minimum) &&
      Number.isInteger(m.frames_maximum) &&
      Number.isInteger(m.frames_steps) &&
      m.frames_steps > 0 &&
      settings.video_length >= m.frames_minimum &&
      settings.video_length <= m.frames_maximum &&
      (settings.video_length - m.frames_minimum) % m.frames_steps === 0,
    'Video length does not match the model frame grid.',
  );
}

function checkPreflight(value, settings) {
  assert(
    value?.valid === true &&
      value.submitted === false &&
      Array.isArray(value.tasks) &&
      value.tasks.length === 1,
    'WanGP rejected the single-task video preflight.',
  );
  const [task] = value.tasks;
  assert(
    task.model_type === settings.model_type && task.availability?.available === true,
    'The selected video checkpoint is not locally available.',
  );
  assert(
    Array.isArray(task.missing_loras) && task.missing_loras.length === 0,
    'A configured video LoRA is missing.',
  );
  const dimensions = task.sizing?.initial_dimensions;
  const [width, height] = settings.resolution.split('x').map(Number);
  assert(
    dimensions?.width === width &&
      dimensions.height === height &&
      task.requested_video?.frames === settings.video_length &&
      task.requested_video?.fps === Number(settings.force_fps) &&
      task.window_plan?.windows?.length === 1 &&
      task.window_plan.total_frames === settings.video_length,
    'WanGP changed the requested canvas, frame count, FPS or single-window budget.',
  );
  assert(
    value.queue?.total_count === 0 &&
      value.queue.queued_count === 0 &&
      value.queue.running_count === 0,
    'WanGP is busy. Wait for its current work without clearing the queue.',
  );
}

export async function verifyVideoFile(filename) {
  const { chromium } = await import('@playwright/test');
  const browser = await chromium.launch({
    executablePath: process.env.DEEP_BROWSER_PATH || undefined,
  });
  try {
    const page = await browser.newPage();
    await page.goto(pathToFileURL(resolve(filename)).href, { timeout: 20_000 });
    return await page.locator('video').evaluate(
      async (video) => {
        await new Promise((resolve, reject) => {
          const timer = setTimeout(() => reject(new Error('Video playback timed out.')), 15_000);
          const failed = () => {
            clearTimeout(timer);
            reject(new Error('Browser could not decode MP4.'));
          };
          video.addEventListener('error', failed, { once: true });
          video.addEventListener(
            'ended',
            () => {
              clearTimeout(timer);
              resolve();
            },
            { once: true },
          );
          video.muted = true;
          video.currentTime = 0;
          video.play().catch(failed);
        });
        return {
          width: video.videoWidth,
          height: video.videoHeight,
          duration_seconds: video.duration,
          playback_frames: video.getVideoPlaybackQuality().totalVideoFrames,
        };
      },
      undefined,
      { timeout: 20_000 },
    );
  } finally {
    await browser.close();
  }
}

export async function runVideoSmoke(values, verify = verifyVideoFile) {
  assert(values.endpoint, 'Supply --endpoint <loopback MCP URL>.');
  const endpoint = new URL(values.endpoint);
  assert(
    endpoint.protocol === 'http:' &&
      ['127.0.0.1', 'localhost', '[::1]'].includes(endpoint.hostname) &&
      endpoint.pathname === '/mcp' &&
      !endpoint.username &&
      !endpoint.password &&
      !endpoint.search &&
      !endpoint.hash,
    'Use an explicit loopback HTTP /mcp endpoint.',
  );
  assert(values.report && values.settings, 'Supply --settings <JSON> and a new --report <JSONL>.');
  assert(!values.cancel || values.generate, '--cancel requires --generate.');
  assert(
    values.generate
      ? values.output && extname(values.output).toLowerCase() === '.mp4'
      : !values.output,
    '--generate requires a new --output <file.mp4>; preflight writes no video.',
  );
  const timeoutSeconds = Number(values['timeout-seconds'] ?? 900);
  assert(
    Number.isInteger(timeoutSeconds) && timeoutSeconds >= 10 && timeoutSeconds <= 1800,
    'Video timeout must be 10-1800 seconds.',
  );
  const input = await open(values.settings, 'r');
  let settings;
  try {
    assert((await input.stat()).size <= MAX_SETTINGS_BYTES, 'Settings exceed 16 KiB.');
    const bytes = Buffer.alloc(MAX_SETTINGS_BYTES + 1);
    const { bytesRead } = await input.read(bytes);
    assert(bytesRead <= MAX_SETTINGS_BYTES, 'Settings exceed 16 KiB.');
    settings = videoSettings(
      JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, bytesRead))),
    );
  } finally {
    await input.close();
  }
  const report = await open(values.report, 'wx', 0o600);
  const record = async (event, data = {}) => {
    await report.writeFile(JSON.stringify({ event, at_ms: Date.now(), ...data }) + '\n');
    await report.sync();
  };
  const result = {
    passed: false,
    mode: values.generate ? (values.cancel ? 'cancel' : 'generate') : 'preflight',
    request_id: `deep-video-${randomUUID()}`,
    outcome: 'not_submitted',
    output: null,
  };
  const client = new WanGpClient(endpoint.href);
  let file;
  let job;
  let dispatch = false;
  let cancelSent = false;
  const observe = async (receipt) => {
    job = jobReceipt(receipt, result.agent_context, result.request_id, result.job_id);
    result.job_id = job.job_id;
    await record('job', publicJob(job));
    assert(!job.done || terminalKnown(job), 'WanGP terminal outcome is unknown.');
  };
  const cancel = async (signal) => {
    cancelSent = true;
    await record('cancel_intent', { job_id: result.job_id });
    await observe(
      await client.call(
        'wangp_session',
        {
          action: 'cancel_job',
          arguments: { job_id: result.job_id },
          agent_context: result.agent_context,
        },
        { signal },
      ),
    );
  };
  const readJob = async (signal) => {
    await observe(
      await client.call(
        'wangp_session',
        {
          action: 'get_job',
          arguments: { job_id: result.job_id, wait_s: 1 },
          agent_context: result.agent_context,
        },
        { signal },
      ),
    );
  };
  try {
    await record('request', {
      schema: 1,
      request_id: result.request_id,
      mode: result.mode,
      settings,
      endpoint: endpoint.href,
      timeout_seconds: timeoutSeconds,
    });
    if (values.generate) file = await open(values.output, 'wx', 0o600);
    await client.initialize();
    checkModel(
      await client.call('wangp_model', {
        model_type: settings.model_type,
        action: 'capabilities',
        arguments: {},
      }),
      settings,
    );
    const context = await client.call('wangp_workspace', {});
    assert(remoteId(context.agent_context), 'WanGP did not return an owned agent context.');
    result.agent_context = context.agent_context;
    await record('context', { agent_context: result.agent_context });
    const validation = await client.call('wangp_validate', {
      settings,
      agent_context: result.agent_context,
    });
    checkPreflight(validation, settings);
    await record('preflight', {
      model_type: settings.model_type,
      available: true,
      queue_idle: true,
      requested_dimensions: validation.tasks[0].requested_dimensions,
      requested_video: validation.tasks[0].requested_video,
      runtime_config: Object.fromEntries(
        [
          'transformer_quantization',
          'text_encoder_quantization',
          'mixed_precision',
          'profile',
          'attention_mode',
        ]
          .filter((key) => ['string', 'number'].includes(typeof validation.runtime_config?.[key]))
          .map((key) => [key, validation.runtime_config[key]]),
      ),
    });
    if (!values.generate) {
      result.outcome = 'preflight_passed';
      result.passed = true;
    } else {
      const signal = AbortSignal.timeout(timeoutSeconds * 1000);
      await record('dispatch_intent', { request_id: result.request_id });
      dispatch = true;
      result.outcome = 'submission_unknown';
      await observe(
        await client.call(
          'wangp_generate',
          {
            settings,
            wait: false,
            request_id: result.request_id,
            agent_context: result.agent_context,
          },
          { signal },
        ),
      );
      result.outcome = 'pending';
      while (!job.done) {
        signal.throwIfAborted();
        if (values.cancel && !cancelSent && activePhase(job)) await cancel(signal);
        if (!job.done) {
          await delay(250, undefined, { signal });
          await readJob(signal);
        }
      }
      if (values.cancel) {
        assert(
          cancelSent && job.result.cancelled && !job.result.success,
          'Video completed before the running-job cancellation could be verified.',
        );
        result.outcome = 'cancelled';
        result.passed = true;
      } else {
        const mediaId = finalVideo(job);
        await record('output_selected', { media_id: mediaId, job_id: result.job_id });
        const video = await client.downloadVideo(mediaId, result.agent_context, { signal });
        await file.writeFile(video.bytes);
        await file.sync();
        const playback = await verify(values.output);
        const [width, height] = settings.resolution.split('x').map(Number);
        assert(
          playback.width === width &&
            playback.height === height &&
            playback.playback_frames > 0 &&
            Number.isFinite(playback.duration_seconds) &&
            playback.duration_seconds > 0 &&
            Math.abs(
              playback.duration_seconds - settings.video_length / Number(settings.force_fps),
            ) <= 0.1,
          'Decoded video did not match the requested dimensions or duration.',
        );
        result.output = {
          path: resolve(values.output),
          bytes: video.bytes.length,
          sha256: video.sha256,
          media_type: video.media_type,
          ...playback,
        };
        result.outcome = 'succeeded';
        result.passed = true;
      }
    }
  } catch (error) {
    result.error = error.message;
    result.outcome = dispatch ? (job ? 'failed' : 'submission_unknown') : 'not_submitted';
    if (job?.done && !terminalKnown(job)) result.outcome = 'unresolved';
    if (result.job_id && !job.done) {
      result.outcome = 'unresolved';
      try {
        const cleanupSignal = AbortSignal.timeout(30_000);
        if (!cancelSent) await cancel(cleanupSignal);
        while (!job.done) {
          await delay(250, undefined, { signal: cleanupSignal });
          await readJob(cleanupSignal);
        }
        result.outcome = job.result.cancelled
          ? 'cancelled'
          : job.result.success
            ? 'completed_after_stop'
            : 'failed';
      } catch (cleanupError) {
        result.cleanup_error = cleanupError.message;
      }
    }
  } finally {
    try {
      await client.close();
    } catch (error) {
      result.session_close_error = error.message;
      result.passed = false;
    }
    try {
      if (file) {
        await file.close();
        if (!result.output) await rm(values.output);
      }
      await record('result', result);
    } finally {
      await report.close();
    }
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({
    options: {
      endpoint: { type: 'string' },
      settings: { type: 'string' },
      report: { type: 'string' },
      output: { type: 'string' },
      generate: { type: 'boolean', default: false },
      cancel: { type: 'boolean', default: false },
      'timeout-seconds': { type: 'string' },
    },
  });
  runVideoSmoke(values)
    .then((result) => {
      console.log(JSON.stringify(result, null, 2));
      if (!result.passed) process.exitCode = 1;
    })
    .catch((error) => {
      console.error(error.message);
      process.exitCode = 1;
    });
}
