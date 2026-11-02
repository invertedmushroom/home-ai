import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtemp, readFile, rm, writeFile, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { runVideoSmoke, videoSettings } from '../../scripts/video-smoke.mjs';

const settings = {
  model_type: 't2v',
  prompt: 'A paper boat rocking on calm water.',
  resolution: '256x256',
  video_length: 9,
  force_fps: '16',
  num_inference_steps: 4,
  seed: 42,
};
const playback = { width: 256, height: 256, duration_seconds: 9 / 16, playback_frames: 9 };

async function fixture(t) {
  const folder = await mkdtemp(join(tmpdir(), 'deep-video-'));
  const bytes = Buffer.from('synthetic MP4 transport bytes; playback is a separate check');
  const state = {
    calls: [],
    gets: 0,
    downloads: 0,
    cancellations: 0,
    generations: 0,
    validated: 0,
    request: null,
    context: 'owned-context',
    cancel: false,
    beforeDispatch: null,
    capabilities: {
      metadata: {
        model_type: 't2v',
        main_output: ['video'],
        capabilities: ['text_to_video'],
        frames_minimum: 5,
        frames_maximum: 97,
        frames_steps: 4,
      },
    },
    preflight: {
      valid: true,
      submitted: false,
      queue: { total_count: 0, queued_count: 0, running_count: 0 },
      runtime_config: { profile: 4, private_setting: 'must not enter the report' },
      tasks: [
        {
          model_type: 't2v',
          availability: { available: true },
          missing_loras: [],
          sizing: { initial_dimensions: { width: 256, height: 256 } },
          requested_dimensions: { width: 256, height: 256 },
          requested_video: { frames: 9, fps: 16 },
          window_plan: { windows: [{}], total_frames: 9 },
        },
      ],
    },
    gallery: [
      {
        media_id: 'visual:owned',
        media_type: 'video',
        role: 'final',
        task_id: 'mcp_owned-job_1',
      },
    ],
    descriptor: {},
    generationStatus: 200,
    pollStatus: 200,
    cancelStatus: 200,
  };
  const options = {
    settings: join(folder, 'settings.json'),
    output: join(folder, 'output.mp4'),
    report: join(folder, 'report.jsonl'),
    generate: true,
  };
  await writeFile(options.settings, JSON.stringify(settings));
  const receipt = (done) => ({
    job_id: 'owned-job',
    request_id: state.request,
    agent_context: state.context,
    done,
    progress: { task_id: 'mcp_owned-job_1', phase: 'inference' },
    result: done
      ? {
          success: !state.cancel,
          cancelled: state.cancel,
          gallery_items: state.gallery,
        }
      : null,
  });
  const server = createServer(async (req, res) => {
    if (req.url === '/movie') {
      state.downloads++;
      res.writeHead(200, { 'content-type': 'video/mp4' }).end(bytes);
      return;
    }
    if (req.method === 'DELETE') {
      res.writeHead(200).end();
      return;
    }
    let raw = '';
    for await (const chunk of req) raw += chunk;
    const message = JSON.parse(raw);
    state.calls.push(message);
    if (message.method === 'notifications/initialized') {
      res.writeHead(202).end();
      return;
    }
    let data;
    if (message.method === 'initialize') {
      data = { protocolVersion: '2025-03-26', serverInfo: { name: 'WanGP' } };
    } else {
      const { name, arguments: args } = message.params;
      if (name === 'wangp_model') data = state.capabilities;
      else if (name === 'wangp_workspace') data = { agent_context: 'owned-context' };
      else if (name === 'wangp_validate') {
        state.validated++;
        data = state.preflight;
      } else if (name === 'wangp_generate') {
        state.generations++;
        state.request = args.request_id;
        state.beforeDispatch = (await readFile(options.report, 'utf8'))
          .trim()
          .split('\n')
          .map(JSON.parse);
        if (state.generationStatus !== 200) {
          res.writeHead(state.generationStatus).end();
          return;
        }
        data = receipt(false);
        if (state.initialReceipt) data = state.initialReceipt(data);
      } else if (name === 'wangp_session' && args.action === 'get_job') {
        state.gets++;
        if (state.pollStatus !== 200) {
          res.writeHead(state.pollStatus).end();
          return;
        }
        data = receipt(true);
        if (state.pollReceipt) data = state.pollReceipt(data);
      } else if (name === 'wangp_session' && args.action === 'cancel_job') {
        state.cancellations++;
        state.cancel = true;
        if (state.cancelStatus !== 200) {
          res.writeHead(state.cancelStatus).end();
          return;
        }
        data = receipt(true);
      } else if (name === 'wangp_session' && args.action === 'create_file_download') {
        data = {
          download_url: '/movie',
          method: 'GET',
          size_bytes: bytes.length,
          mime_type: 'video/mp4',
          sha256: createHash('sha256').update(bytes).digest('hex'),
          ...state.descriptor,
        };
      } else {
        res.writeHead(400).end();
        return;
      }
      data = { structuredContent: data };
    }
    res
      .writeHead(200, { 'content-type': 'application/json', 'mcp-session-id': 'session' })
      .end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result: data }));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  options.endpoint = `http://127.0.0.1:${server.address().port}/mcp`;
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    await rm(folder, { recursive: true, force: true });
  });
  return { state, options, bytes };
}

test('settings freeze one literal bounded video and reject unsupported media or execution inputs', () => {
  const actual = videoSettings(settings);
  assert.equal(actual.batch_size, 1);
  assert.equal(actual.repeat_generation, 1);
  assert.equal(actual.multi_prompts_gen_type, 'FG');
  assert.equal(actual.image_mode, 0);
  assert.equal(actual.image_prompt_type, '');
  assert.equal(actual.video_prompt_type, '');
  assert.equal(actual.output_filename, '');
  assert.deepEqual(actual.activated_loras, []);
  for (const changed of [
    { model_type: undefined },
    { prompt: '@file("@documents/prompt.txt")' },
    { prompt: 'first\nsecond' },
    { prompt: 'x'.repeat(4097) },
    { image_start: '@documents/uploads/image.png' },
    { template: 'gen_video' },
    { overrides: 'host code' },
    { output_filename: 'elsewhere' },
    { batch_size: 2 },
    { resolution: '833x480' },
    { resolution: '832x496' },
    { resolution: '112x128' },
    { video_length: 4 },
    { video_length: 98 },
    { video_length: 9.5 },
    { force_fps: '' },
    { force_fps: 16 },
    { force_fps: '31' },
    { video_length: 97, force_fps: '8' },
    { seed: -1 },
    { seed: 0x100000000 },
    { num_inference_steps: 0 },
    { num_inference_steps: 9 },
    { activated_loras: ['..\\private.safetensors'] },
    { activated_loras: ['a.safetensors'], loras_multipliers: '' },
    { activated_loras: ['a.safetensors'], loras_multipliers: '1:2' },
    { activated_loras: ['a.safetensors'], loras_multipliers: '10.1' },
    { guidance_scale: Infinity },
    { negative_prompt: '@file("private")' },
  ])
    assert.throws(
      () => videoSettings({ ...settings, ...changed }),
      undefined,
      JSON.stringify(changed),
    );
  assert.doesNotThrow(() =>
    videoSettings({
      ...settings,
      resolution: '832x480',
      video_length: 97,
      prompt: 'x'.repeat(4096),
      num_inference_steps: 8,
      seed: 0xffffffff,
      activated_loras: ['local.safetensors'],
      loras_multipliers: '-10',
    }),
  );
});

test('preflight performs no generation or download and explicitly records its limited evidence', async (t) => {
  const { options, state } = await fixture(t);
  delete options.output;
  options.generate = false;
  const result = await runVideoSmoke(options, () => assert.fail('No playback during preflight.'));
  assert.equal(result.passed, true);
  assert.equal(result.outcome, 'preflight_passed');
  assert.equal(state.generations, 0);
  assert.equal(state.downloads, 0);
  const events = (await readFile(options.report, 'utf8')).trim().split('\n').map(JSON.parse);
  assert.deepEqual(
    events.map((e) => e.event),
    ['request', 'context', 'preflight', 'result'],
  );
  assert.equal(JSON.stringify(events).includes('must not enter the report'), false);
});

test('generation commits its exact intent first, downloads one owned MP4 and verifies decoded playback', async (t) => {
  const { options, state, bytes } = await fixture(t);
  let verifications = 0;
  const result = await runVideoSmoke(options, async (path) => {
    verifications++;
    assert.deepEqual(await readFile(path), bytes);
    return playback;
  });
  assert.equal(result.passed, true);
  assert.equal(result.outcome, 'succeeded');
  assert.deepEqual(await readFile(options.output), bytes);
  assert.equal(result.output.sha256, createHash('sha256').update(bytes).digest('hex'));
  assert.equal(state.generations, 1);
  assert.equal(state.cancellations, 0);
  assert.equal(verifications, 1);
  assert.equal(state.beforeDispatch.at(-1).event, 'dispatch_intent');
  assert.equal(state.beforeDispatch[0].request_id, result.request_id);
  assert.equal(state.beforeDispatch[0].settings.video_length, 9);
  assert.equal(
    state.beforeDispatch.some((e) => e.event === 'context'),
    true,
  );
});

test('busy, unavailable, adjusted, multi-task or incapable preflights never dispatch', async (t) => {
  for (const alter of [
    (s) => {
      s.preflight.queue.total_count = 1;
    },
    (s) => {
      s.preflight.tasks[0].availability.available = false;
    },
    (s) => {
      s.preflight.tasks[0].missing_loras = ['missing'];
    },
    (s) => {
      s.preflight.tasks[0].sizing.initial_dimensions.width = 512;
    },
    (s) => {
      s.preflight.tasks[0].requested_video.fps = 24;
    },
    (s) => {
      s.preflight.tasks.push(s.preflight.tasks[0]);
    },
    (s) => {
      s.capabilities.metadata.main_output = ['image'];
    },
    (s) => {
      s.capabilities.metadata.frames_minimum = 8;
    },
  ]) {
    const { options, state } = await fixture(t);
    alter(state);
    const result = await runVideoSmoke(options);
    assert.equal(result.passed, false);
    assert.equal(result.outcome, 'not_submitted');
    assert.equal(state.generations, 0);
    assert.equal(state.cancellations, 0);
    await assert.rejects(access(options.output), /ENOENT/);
  }
});

test('lost submission or foreign initial receipt remains unknown and never replays or guesses a job', async (t) => {
  for (const alter of [
    (s) => {
      s.generationStatus = 503;
    },
    (s) => {
      s.initialReceipt = (r) => ({ ...r, request_id: 'foreign' });
    },
    (s) => {
      s.initialReceipt = (r) => ({ ...r, agent_context: 'foreign' });
    },
  ]) {
    const { options, state } = await fixture(t);
    alter(state);
    const result = await runVideoSmoke(options);
    assert.equal(result.passed, false);
    assert.equal(result.outcome, 'submission_unknown');
    assert.equal(state.generations, 1);
    assert.equal(state.cancellations, 0);
    assert.equal(state.gets, 0);
    assert.equal(result.agent_context, 'owned-context');
    assert.equal(result.job_id, undefined);
    await assert.rejects(access(options.output), /ENOENT/);
    const log = await readFile(options.report, 'utf8');
    assert(log.includes('"event":"dispatch_intent"'));
    assert(log.includes('"outcome":"submission_unknown"'));
  }
});

test('post-admission failure cancels only the owned receipt, once, and retains the original error', async (t) => {
  const { options, state } = await fixture(t);
  state.pollReceipt = (r) => ({ ...r, job_id: 'foreign' });
  const result = await runVideoSmoke(options);
  assert.equal(result.passed, false);
  assert.equal(result.outcome, 'cancelled');
  assert.match(result.error, /foreign/);
  assert.equal(state.generations, 1);
  assert.equal(state.cancellations, 1);
  assert.equal(state.downloads, 0);
  assert.equal(
    state.calls
      .filter((c) => c.params?.name === 'wangp_session')
      .every((c) => c.params.arguments.arguments.job_id === 'owned-job'),
    true,
  );
  await assert.rejects(access(options.output), /ENOENT/);
});

test('failed cleanup preserves unresolved ownership and never resubmits generation', async (t) => {
  const { options, state } = await fixture(t);
  state.pollStatus = 503;
  state.cancelStatus = 503;
  const result = await runVideoSmoke(options);
  assert.equal(result.passed, false);
  assert.equal(result.outcome, 'unresolved');
  assert.equal(result.job_id, 'owned-job');
  assert.match(result.error, /503/);
  assert.match(result.cleanup_error, /503/);
  assert.equal(state.generations, 1);
  assert.equal(state.cancellations, 1);
});

test('explicit cancellation proves a terminal cancelled receipt and publishes no MP4', async (t) => {
  const { options, state } = await fixture(t);
  options.cancel = true;
  const result = await runVideoSmoke(options);
  assert.equal(result.passed, true);
  assert.equal(result.outcome, 'cancelled');
  assert.equal(result.output, null);
  assert.equal(state.generations, 1);
  assert.equal(state.cancellations, 1);
  assert.equal(state.downloads, 0);
  await assert.rejects(access(options.output), /ENOENT/);
});

test('foreign progress cannot trigger running-job cancellation', async (t) => {
  const { options, state } = await fixture(t);
  options.cancel = true;
  state.initialReceipt = (r) => ({
    ...r,
    progress: { task_id: 'mcp_foreign_1', phase: 'inference' },
  });
  const result = await runVideoSmoke(options);
  assert.equal(result.passed, false);
  assert.equal(state.cancellations, 0);
  assert.equal(state.downloads, 0);
});

test('unknown terminal outcomes retain the verified job ID without replay or assumed success', async (t) => {
  for (const alter of [
    (s) => {
      s.initialReceipt = (r) => ({ ...r, done: true, result: null });
    },
    (s) => {
      s.pollReceipt = (r) => ({ ...r, result: { success: true, cancelled: true } });
    },
  ]) {
    const { options, state } = await fixture(t);
    alter(state);
    const result = await runVideoSmoke(options);
    assert.equal(result.passed, false);
    assert.equal(result.outcome, 'unresolved');
    assert.equal(result.job_id, 'owned-job');
    assert.match(result.error, /terminal outcome is unknown/);
    assert.equal(state.generations, 1);
    assert.equal(state.cancellations, 0);
    assert.equal(state.downloads, 0);
  }
});

test('foreign, extra and nonfinal output receipts cannot be downloaded', async (t) => {
  for (const alter of [
    (s) => {
      s.gallery[0].task_id = 'mcp_foreign_1';
    },
    (s) => {
      s.gallery[0].media_type = 'image';
    },
    (s) => {
      s.gallery[0].role = 'preview';
    },
    (s) => {
      s.gallery.push(s.gallery[0]);
    },
  ]) {
    const { options, state } = await fixture(t);
    alter(state);
    const result = await runVideoSmoke(options);
    assert.equal(result.passed, false);
    assert.equal(state.downloads, 0);
    assert.equal(state.cancellations, 0);
    await assert.rejects(access(options.output), /ENOENT/);
  }
});

test('invalid playback dimensions or duration remove only the owned incomplete output', async (t) => {
  for (const altered of [
    { ...playback, width: 512 },
    { ...playback, duration_seconds: 10 },
    { ...playback, playback_frames: 0 },
  ]) {
    const { options, state } = await fixture(t);
    const result = await runVideoSmoke(options, async () => altered);
    assert.equal(result.passed, false);
    assert.match(result.error, /Decoded video/);
    assert.equal(result.output, null);
    assert.equal(state.cancellations, 0);
    await assert.rejects(access(options.output), /ENOENT/);
  }
});

test('existing output or receipt files are preserved before any runtime work', async (t) => {
  const { options, state } = await fixture(t);
  await writeFile(options.output, 'keep');
  const result = await runVideoSmoke(options);
  assert.equal(result.passed, false);
  assert.equal(await readFile(options.output, 'utf8'), 'keep');
  assert.equal(state.calls.length, 0);
  await assert.rejects(runVideoSmoke(options), /EEXIST/);
  assert.equal(state.calls.length, 0);
});
