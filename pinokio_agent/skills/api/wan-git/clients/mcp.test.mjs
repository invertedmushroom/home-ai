import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { WanGpClient, MAX_VIDEO_BYTES } from './mcp.mjs';

async function fixture(
  run,
  toolResult,
  { reply, image, descriptor = {}, status = 200, mediaType = 'image/jpeg' } = {},
) {
  const calls = [];
  const bytes = image ?? Buffer.from('synthetic image transfer');
  const server = createServer(async (req, res) => {
    if (req.url === '/image') {
      res.writeHead(200, { 'content-type': mediaType });
      res.end(bytes);
      return;
    }
    if (req.method === 'DELETE') {
      assert.equal(req.headers['mcp-session-id'], 'test-session');
      res.writeHead(200);
      res.end();
      return;
    }
    let body = '';
    for await (const chunk of req) body += chunk;
    const message = JSON.parse(body);
    calls.push(message);
    if (message.method !== 'initialize') {
      assert.equal(req.headers['mcp-session-id'], 'test-session');
      assert.equal(req.headers['mcp-protocol-version'], '2025-03-26');
    }
    if (message.method === 'notifications/initialized') {
      assert.equal(message.id, undefined);
      res.writeHead(202);
      res.end();
      return;
    }
    if (message.method !== 'initialize' && status !== 200) {
      res.writeHead(status, { location: '/must-not-follow' });
      res.end();
      return;
    }
    const result =
      message.method === 'initialize'
        ? { protocolVersion: '2025-03-26', serverInfo: { name: 'WanGP' } }
        : (toolResult?.(message) ?? {
            content: [
              {
                type: 'text',
                text: JSON.stringify({
                  download_url: '/image',
                  method: 'GET',
                  size_bytes: bytes.length,
                  sha256: createHash('sha256').update(bytes).digest('hex'),
                  mime_type: mediaType,
                  ...descriptor,
                }),
              },
            ],
          });
    const response = { jsonrpc: '2.0', id: message.id, result };
    const wire = reply?.(response, message);
    res.writeHead(200, {
      'content-type': wire ? 'text/event-stream' : 'application/json',
      'mcp-session-id': 'test-session',
    });
    if (wire) {
      const data = Buffer.from(wire);
      const step = data.length > 4096 ? 4096 : 1;
      for (let offset = 0; offset < data.length; offset += step) {
        res.write(data.subarray(offset, offset + step));
      }
      res.end();
    } else {
      res.end(JSON.stringify(response));
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const endpoint = `http://127.0.0.1:${server.address().port}/mcp`;
  const client = new WanGpClient(endpoint);
  try {
    await client.initialize();
    await run(client, calls, bytes);
  } finally {
    await client.close();
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}

test('JSON and fragmented UTF-8 SSE preserve protocol identity, session and structured tool results', async () => {
  for (const sse of [false, true]) {
    await fixture(
      async (client, calls) => {
        assert.deepEqual(await client.call('wangp_models', { query: '界' }), {
          status: 'done',
          label: '界',
        });
        assert.equal(calls[0].method, 'initialize');
        assert.equal(calls[1].method, 'notifications/initialized');
        assert.deepEqual(calls[2].params.arguments, { query: '界' });
      },
      () => ({ structuredContent: { status: 'done', label: '界' } }),
      sse
        ? {
            reply: (response) =>
              `: heartbeat\r\n\r\nevent: message\r\ndata: ${JSON.stringify(response)}\r\n\r\n`,
          }
        : {},
    );
  }
});

test('MCP errors, tool errors and structured operation errors fail explicitly', async () => {
  for (const result of [
    { isError: true, content: [{ type: 'text', text: 'request_id payload conflict' }] },
    { structuredContent: { status: 'error', error: 'request_id payload conflict' } },
    {
      content: [{ type: 'text', text: '{"status":"error","error":"request_id payload conflict"}' }],
    },
  ]) {
    await fixture(
      (client) => assert.rejects(client.call('wangp_generate', {}), /payload conflict/),
      () => result,
    );
  }
  await fixture(
    (client) => assert.rejects(client.call('wangp_generate', {}), /MCP error -32602/),
    () => ({}),
    {
      reply: (response, message) =>
        `data: ${JSON.stringify(
          message.method === 'initialize'
            ? response
            : {
                jsonrpc: '2.0',
                id: response.id,
                error: { code: -32602, message: 'invalid input' },
              },
        )}\n\n`,
    },
  );
});

test('missing and duplicate response IDs cannot be accepted', async () => {
  for (const duplicate of [false, true]) {
    await fixture(
      (client) => assert.rejects(client.call('wangp_models', {}), /response identity/),
      () => ({}),
      {
        reply: (response, message) => {
          if (message.method === 'initialize') return `data: ${JSON.stringify(response)}\n\n`;
          const event = `data: ${JSON.stringify({
            ...response,
            id: duplicate ? response.id : response.id + 1,
          })}\n\n`;
          return duplicate ? event + event : event;
        },
      },
    );
  }
});

test('HTTP errors and redirects fail without retrying a tool call', async () => {
  for (const status of [307, 503]) {
    await fixture(
      async (client, calls) => {
        await assert.rejects(client.call('wangp_generate', {}), /HTTP 503|fetch failed/);
        assert.equal(calls.filter((call) => call.method === 'tools/call').length, 1);
      },
      undefined,
      { status },
    );
  }
});

test('oversized and malformed SSE replies fail explicitly', async () => {
  for (const wire of ['data: invalid-json\n\n', `data: ${'x'.repeat(2 * 1024 * 1024)}\n\n`]) {
    await fixture(
      (client) => assert.rejects(client.call('wangp_models', {}), /JSON|Unexpected|exceeded/),
      undefined,
      {
        reply: (response, message) =>
          message.method === 'initialize' ? `data: ${JSON.stringify(response)}\n\n` : wire,
      },
    );
  }
});

test('relative image grants stay on the MCP origin and verify exact length and SHA-256', async () => {
  await fixture(async (client, calls, bytes) => {
    const image = await client.downloadImage('visual:test', 'context');
    assert.deepEqual(image.bytes, bytes);
    assert.equal(image.media_type, 'image/jpeg');
    assert.deepEqual(calls.at(-1).params.arguments, {
      action: 'create_file_download',
      arguments: { source: 'visual:test', include_sha256: true },
      agent_context: 'context',
    });
  });
});

test('MP4 grants share exact origin, MIME, length and digest checks without broadening image transfers', async () => {
  await fixture(
    async (client, calls, bytes) => {
      const video = await client.downloadVideo('visual:owned', 'context');
      assert.deepEqual(video.bytes, bytes);
      assert.equal(video.media_type, 'video/mp4');
      assert.equal(calls.at(-1).params.arguments.agent_context, 'context');
      await assert.rejects(client.downloadImage('visual:owned'), /descriptor/);
    },
    undefined,
    { mediaType: 'video/mp4' },
  );
  for (const descriptor of [
    { mime_type: 'video/webm' },
    { size_bytes: MAX_VIDEO_BYTES + 1 },
    { size_bytes: 0 },
    { size_bytes: 1 },
    { size_bytes: 100 },
    { sha256: '0'.repeat(64) },
    { mime_type: 'image/png' },
    { download_url: 'https://example.com/movie.mp4' },
  ]) {
    await fixture(
      (client) =>
        assert.rejects(
          client.downloadVideo('visual:owned'),
          /descriptor|exceeded|length|SHA-256|origin/,
        ),
      undefined,
      { mediaType: 'video/mp4', descriptor },
    );
  }
});

test('video transfer accepts the exact 64 MiB limit', async () => {
  const image = Buffer.alloc(MAX_VIDEO_BYTES, 42);
  await fixture(
    async (client) => {
      const video = await client.downloadVideo('visual:owned');
      assert.equal(video.bytes.length, MAX_VIDEO_BYTES);
    },
    undefined,
    { image, mediaType: 'video/mp4' },
  );
});

test('an aborted owner prevents both video grant calls and native tool dispatch', async () => {
  await fixture(async (client, calls) => {
    const controller = new AbortController();
    controller.abort();
    await assert.rejects(
      client.downloadVideo('visual:owned', 'context', {
        signal: controller.signal,
      }),
      /abort/i,
    );
    await assert.rejects(
      client.call(
        'wangp_generate',
        {},
        {
          signal: controller.signal,
        },
      ),
      /abort/i,
    );
    assert.equal(calls.filter((call) => call.method === 'tools/call').length, 0);
  });
});

test('foreign grants, invalid budgets and unsupported media fail before download', async () => {
  for (const descriptor of [
    { download_url: 'https://example.com/image' },
    { size_bytes: 0 },
    { size_bytes: 8 * 1024 * 1024 + 1 },
    { mime_type: 'text/html' },
    { sha256: 'not-a-digest' },
  ]) {
    await fixture(
      (client) => assert.rejects(client.downloadImage('visual:test'), /origin|descriptor/),
      undefined,
      { descriptor },
    );
  }
});

test('short, excessive and changed image bytes cannot pass descriptor integrity checks', async () => {
  for (const descriptor of [
    { size_bytes: 1 },
    { size_bytes: 100 },
    { sha256: '0'.repeat(64) },
    { mime_type: 'image/png' },
  ]) {
    await fixture(
      (client) =>
        assert.rejects(client.downloadImage('visual:test'), /exceeded|length|SHA-256|media type/),
      undefined,
      { descriptor },
    );
  }
});

test('image transfer accepts the exact 8 MiB limit', async () => {
  const bytes = Buffer.alloc(8 * 1024 * 1024, 42);
  await fixture(
    async (client) => {
      const image = await client.downloadImage('visual:test');
      assert.equal(image.bytes.length, bytes.length);
    },
    undefined,
    { image: bytes },
  );
});

test('invalid endpoint configuration fails without making a request', () => {
  for (const endpoint of [
    'file:///private',
    'https://user:password@example.com/mcp',
    'http://example.com/mcp?token=secret',
    'http://example.com/mcp#fragment',
  ]) {
    assert.throws(() => new WanGpClient(endpoint), /HTTP/);
  }
  assert.throws(() => new WanGpClient('http://127.0.0.1/mcp', { timeoutMs: 0 }), /timeout/);
});
