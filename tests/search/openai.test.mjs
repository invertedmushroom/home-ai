import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { openai, openaiResult, responseText, searchProbe } from '../../tools/search/probe.mjs';

const options = () => ({
  apiKey: 'synthetic-test-key',
  model: 'gpt-5.4-mini',
  signal: new AbortController().signal,
});
const fixture = () => ({
  status: 'completed',
  model: 'reported-model',
  output: [
    {
      type: 'web_search_call',
      status: 'completed',
      action: {
        type: 'search',
        sources: [
          { type: 'url', url: 'https://doc.rust-lang.org/book/', title: 'The Rust Book' },
          { type: 'url', url: 'https://doc.rust-lang.org/book/' },
          { type: 'url', url: 'https://www.rust-lang.org/' },
        ],
      },
    },
    {
      type: 'message',
      status: 'completed',
      role: 'assistant',
      content: [
        { type: 'output_text', text: 'A generated summary, not an extracted page snippet.' },
      ],
    },
  ],
  usage: { input_tokens: 50, output_tokens: 30, total_tokens: 80 },
});

async function withServer(handler, action) {
  const server = createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    return await action(new URL(`http://127.0.0.1:${server.address().port}`));
  } finally {
    server.closeAllConnections();
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}

test('OpenAI sources retain their meaning and usage; absent counts stay unknown', () => {
  const result = openaiResult(fixture());
  assert.equal(result.status, 'succeeded');
  assert.equal(result.result_kind, 'consulted_sources');
  assert.equal(result.results.length, 2);
  assert.equal(result.results[0].title, 'The Rust Book');
  assert.equal(result.results[0].snippet, '');
  assert.equal(result.results[1].title, 'www.rust-lang.org');
  assert.equal(result.reported_model, 'reported-model');
  assert.equal(result.search_calls, 1);
  assert.deepEqual(result.usage, { input_tokens: 50, output_tokens: 30, total_tokens: 80 });
  const missing = fixture();
  delete missing.usage;
  delete missing.model;
  assert.deepEqual(openaiResult(missing).usage, {
    input_tokens: null,
    output_tokens: null,
    total_tokens: null,
  });
  assert.equal(openaiResult(missing).reported_model, null);
});

test('generated prose, incomplete tools and unsafe sources cannot impersonate a successful search', () => {
  const changes = [
    (body) => {
      body.status = 'incomplete';
    },
    (body) => {
      body.error = { message: 'private provider content' };
    },
    (body) => {
      body.output.shift();
    },
    (body) => {
      body.output.push(body.output[0]);
    },
    (body) => {
      body.output[0].status = 'in_progress';
    },
    (body) => {
      body.output[0].action.type = 'open_page';
    },
    (body) => {
      delete body.output[0].action.sources;
    },
    (body) => {
      body.output[0].action.sources = [];
    },
    (body) => {
      body.output[0].action.sources[0].url = 'http://127.0.0.1/private';
    },
    (body) => {
      body.output[0].action.sources[0].type = 'unknown';
    },
    (body) => {
      body.output[1].content = [null];
    },
    (body) => {
      body.output[1].content = [{ type: 'refusal', refusal: 'No' }];
    },
    (body) => {
      body.output[1].content[0].text = 'x'.repeat(32769);
    },
    (body) => {
      body.output.push({ type: 'function_call' });
    },
    (body) => {
      body.usage.input_tokens = -1;
    },
    (body) => {
      body.usage.total_tokens = Number.MAX_SAFE_INTEGER + 1;
    },
    (body) => {
      body.usage = [];
    },
  ];
  for (const change of changes) {
    const body = fixture();
    change(body);
    const result = openaiResult(body);
    assert.equal(result.status, 'failed');
    assert.deepEqual(result.results, []);
    assert(!JSON.stringify(result).includes('private provider content'));
  }
});

test('the OpenAI adapter sends one capped Responses request to the fixed origin', async () => {
  let calls = 0;
  const result = await openai(
    'Rust documentation',
    { ...options(), baseUrl: 'http://untrusted.invalid' },
    async (url, signal, type, request) => {
      calls++;
      assert.equal(url.href, 'https://api.openai.com/v1/responses');
      assert.equal(type, 'application/json');
      assert.equal(request.method, 'POST');
      assert.equal(request.headers.Authorization, 'Bearer synthetic-test-key');
      assert.equal(signal.aborted, false);
      const body = JSON.parse(request.body);
      assert.equal(body.input, 'Rust documentation');
      assert.equal(body.model, 'gpt-5.4-mini');
      assert.equal(body.store, false);
      assert.equal(body.stream, false);
      assert.equal(body.max_tool_calls, 1);
      assert.equal(body.max_output_tokens, 2048);
      assert.deepEqual(body.tools, [{ type: 'web_search', search_context_size: 'low' }]);
      assert.deepEqual(body.tool_choice, { type: 'web_search' });
      assert.deepEqual(body.include, ['web_search_call.action.sources']);
      return { status: 200, text: JSON.stringify(fixture()) };
    },
  );
  assert.equal(calls, 1);
  assert.equal(result.status, 'succeeded');
});

test('provider errors and malformed JSON produce no fallback or credential echo', async () => {
  for (const reply of [
    { status: 429, text: 'synthetic-test-key' },
    { status: 401, text: 'synthetic-test-key' },
    { status: 200, text: 'synthetic-test-key' },
  ]) {
    let calls = 0;
    const result = await openai('Rust', options(), async () => {
      calls++;
      return reply;
    });
    assert.equal(calls, 1);
    assert.equal(result.status, 'failed');
    assert(!JSON.stringify(result).includes('synthetic-test-key'));
  }
});

test('invalid configuration and pre-cancellation make no request; late results cannot succeed', async () => {
  let calls = 0;
  const send = async () => {
    calls++;
    return { status: 200, text: JSON.stringify(fixture()) };
  };
  for (const config of [
    { ...options(), apiKey: '' },
    { ...options(), apiKey: 'bad\nkey' },
    { ...options(), model: undefined },
    { ...options(), signal: AbortSignal.abort() },
  ]) {
    assert.equal((await openai('Rust', config, send)).status, 'failed');
  }
  assert.equal(calls, 0);
  const controller = new AbortController();
  const result = await openai('Rust', { ...options(), signal: controller.signal }, async () => {
    controller.abort();
    return { status: 200, text: JSON.stringify(fixture()) };
  });
  assert.equal(result.code, 'cancelled');
  assert.equal(
    (await searchProbe({ provider: 'openai', query: '!g private' }, options())).code,
    'invalid_input',
  );
});

test('real POST transport sends only explicit credentials and does not follow redirects', async () => {
  let calls = 0;
  await withServer(
    async (request, response) => {
      calls++;
      assert.equal(request.url, '/responses');
      assert.equal(request.method, 'POST');
      assert.equal(request.headers.authorization, 'Bearer synthetic-test-key');
      assert.equal(request.headers.cookie, undefined);
      let body = '';
      for await (const chunk of request) body += chunk;
      assert.equal(JSON.parse(body).input, 'Rust');
      response.writeHead(302, { location: '/credential-trap' }).end('secret body');
    },
    async (base) => {
      const reply = await responseText(
        new URL('/responses', base),
        AbortSignal.timeout(2000),
        'application/json',
        {
          method: 'POST',
          headers: {
            Authorization: 'Bearer synthetic-test-key',
            'Content-Type': 'application/json',
          },
          body: JSON.stringify({ input: 'Rust' }),
        },
      );
      assert.deepEqual(reply, { status: 302, text: '' });
    },
  );
  assert.equal(calls, 1);
});

test('in-flight OpenAI cancellation closes the owned HTTP connection', async () => {
  let arrived;
  const arrival = new Promise((resolve) => {
    arrived = resolve;
  });
  let closed;
  const close = new Promise((resolve) => {
    closed = resolve;
  });
  await withServer(
    (request, response) => {
      response.on('close', closed);
      arrived();
    },
    async (base) => {
      const controller = new AbortController();
      const pending = openai(
        'Rust',
        { ...options(), signal: controller.signal },
        (_url, signal, type, init) => responseText(base, signal, type, init),
      );
      await arrival;
      controller.abort();
      assert.equal((await pending).code, 'cancelled');
      await close;
    },
  );
});

test('CLI loads .env only for explicit OpenAI search and never prints its key', async () => {
  const cwd = await mkdtemp(join(tmpdir(), 'deep-search-'));
  const script = fileURLToPath(new URL('../../scripts/search-smoke.mjs', import.meta.url));
  async function run(provider) {
    const env = { ...process.env };
    delete env.OPENAI_API_KEY;
    delete env.DEEP_OPENAI_SEARCH_MODEL;
    const child = spawn(process.execPath, [script, '--provider', provider, '--query', 'Rust'], {
      cwd,
      env,
      windowsHide: true,
    });
    let output = '';
    child.stdout.on('data', (chunk) => {
      output += chunk;
    });
    child.stderr.on('data', (chunk) => {
      output += chunk;
    });
    const code = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', resolve);
    });
    return { code, output };
  }
  try {
    await writeFile(join(cwd, '.env'), 'OPENAI_API_KEY=synthetic-test-key\n');
    const result = await run('openai');
    assert.equal(result.code, 2);
    assert.equal(JSON.parse(result.output).code, 'invalid_config');
    assert(!result.output.includes('synthetic-test-key'));
    await rm(join(cwd, '.env'));
    await mkdir(join(cwd, '.env'));
    const keyless = await run('searxng');
    assert.equal(keyless.code, 2);
    assert.equal(JSON.parse(keyless.output).code, 'invalid_config');
  } finally {
    const target = await realpath(cwd);
    assert.equal(dirname(target), await realpath(tmpdir()));
    assert(basename(target).startsWith('deep-search-'));
    await rm(cwd, { recursive: true, force: true });
  }
});
