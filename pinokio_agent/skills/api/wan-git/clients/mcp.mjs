import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';

const MAX_REPLY_BYTES = 2 * 1024 * 1024;
export const MAX_VIDEO_BYTES = 64 * 1024 * 1024;

async function readBytes(response, limit) {
  const chunks = [];
  let bytes = 0;
  if (!response.body) throw new Error('HTTP response has no body.');
  for await (const chunk of response.body) {
    bytes += chunk.length;
    if (bytes > limit) throw new Error(`HTTP response exceeded ${limit} bytes.`);
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

async function readReply(response, id) {
  const text = new TextDecoder('utf-8', { fatal: true }).decode(
    await readBytes(response, MAX_REPLY_BYTES),
  );
  let messages;
  if (response.headers.get('content-type')?.includes('text/event-stream')) {
    messages = text.split(/\r?\n\r?\n/).flatMap((event) => {
      const lines = event
        .split(/\r?\n/)
        .filter((line) => line.startsWith('data:'))
        .map((line) => line.slice(5).replace(/^ /, ''));
      return lines.length ? [JSON.parse(lines.join('\n'))] : [];
    });
  } else if (response.headers.get('content-type')?.includes('application/json')) {
    messages = [JSON.parse(text)];
  } else {
    throw new Error('MCP returned an unsupported content type.');
  }
  const matching = messages.filter((message) => message?.id === id);
  if (matching.length !== 1 || matching[0].jsonrpc !== '2.0') {
    throw new Error('MCP returned a missing, duplicate or invalid response identity.');
  }
  const message = matching[0];
  if (message.error) {
    throw new Error(`MCP error ${message.error.code}: ${message.error.message}`);
  }
  if (!Object.hasOwn(message, 'result')) throw new Error('MCP response has no result.');
  return message.result;
}

export class WanGpClient {
  #endpoint;
  #session;
  #protocol = '2025-03-26';
  #nextId = 0;
  #timeoutMs;

  constructor(endpoint, { timeoutMs = 30_000 } = {}) {
    const url = new URL(endpoint);
    if (
      !['http:', 'https:'].includes(url.protocol) ||
      url.username ||
      url.password ||
      url.search ||
      url.hash
    ) {
      throw new Error('Use an HTTP(S) MCP endpoint without credentials, query or fragment.');
    }
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
      throw new Error('MCP timeout must be a positive integer.');
    }
    this.#endpoint = url.href;
    this.#timeoutMs = timeoutMs;
  }

  #headers() {
    return {
      'Content-Type': 'application/json',
      Accept: 'application/json, text/event-stream',
      'MCP-Protocol-Version': this.#protocol,
      ...(this.#session ? { 'Mcp-Session-Id': this.#session } : {}),
    };
  }

  async #post(method, params, notification = false, signal) {
    const id = notification ? undefined : ++this.#nextId;
    const response = await fetch(this.#endpoint, {
      method: 'POST',
      headers: this.#headers(),
      redirect: 'error',
      signal: this.#signal(signal),
      body: JSON.stringify({ jsonrpc: '2.0', ...(notification ? {} : { id }), method, params }),
    });
    if (!response.ok) throw new Error(`MCP ${method} returned HTTP ${response.status}.`);
    const session = response.headers.get('Mcp-Session-Id');
    if (session) {
      if (this.#session && session !== this.#session) {
        throw new Error('MCP changed session identity.');
      }
      this.#session = session;
    }
    if (notification) {
      if (response.status !== 202 && response.status !== 204) {
        throw new Error('MCP did not acknowledge the notification.');
      }
      return;
    }
    return readReply(response, id);
  }

  async initialize() {
    const result = await this.#post('initialize', {
      protocolVersion: this.#protocol,
      capabilities: {},
      clientInfo: { name: 'deep-wangp-probe', version: '0.1.0' },
    });
    if (result?.protocolVersion !== this.#protocol || result?.serverInfo?.name !== 'WanGP') {
      throw new Error('Expected WanGP with MCP protocol 2025-03-26.');
    }
    await this.#post('notifications/initialized', {}, true);
    return result;
  }

  async tools() {
    const result = await this.#post('tools/list', {});
    if (!Array.isArray(result?.tools)) throw new Error('MCP returned an invalid tool list.');
    return result;
  }

  #signal(signal) {
    const timeout = AbortSignal.timeout(this.#timeoutMs);
    return signal ? AbortSignal.any([signal, timeout]) : timeout;
  }

  async call(name, args, { signal } = {}) {
    const result = await this.#post('tools/call', { name, arguments: args }, false, signal);
    if (result?.isError) {
      const text = result.content
        ?.filter((item) => item.type === 'text')
        .map((item) => item.text)
        .join('\n');
      throw new Error(`${name} failed: ${text || 'No tool error details.'}`);
    }
    let data = result?.structuredContent;
    if (!data) {
      const content = result?.content?.filter((item) => item.type === 'text');
      if (content?.length !== 1 || typeof content[0].text !== 'string') {
        throw new Error(`${name} returned no single structured text result.`);
      }
      data = JSON.parse(content[0].text);
    }
    if (!data || typeof data !== 'object' || Array.isArray(data)) {
      throw new Error(`${name} returned an invalid object result.`);
    }
    if (data.status === 'error') {
      throw new Error(`${name} failed: ${JSON.stringify(data)}`);
    }
    return data;
  }

  async downloadImage(source, agentContext) {
    return this.#download(
      source,
      agentContext,
      ['image/png', 'image/jpeg', 'image/webp'],
      8 * 1024 * 1024,
      'image',
    );
  }

  async downloadVideo(source, agentContext, { signal } = {}) {
    return this.#download(source, agentContext, ['video/mp4'], MAX_VIDEO_BYTES, 'video', signal);
  }

  async #download(source, agentContext, mediaTypes, limit, kind, signal) {
    const descriptor = await this.call(
      'wangp_session',
      {
        action: 'create_file_download',
        arguments: { source, include_sha256: true },
        ...(agentContext ? { agent_context: agentContext } : {}),
      },
      { signal },
    );
    if (
      typeof descriptor.download_url !== 'string' ||
      descriptor.method !== 'GET' ||
      !Number.isSafeInteger(descriptor.size_bytes) ||
      descriptor.size_bytes < 1 ||
      descriptor.size_bytes > limit ||
      typeof descriptor.sha256 !== 'string' ||
      !/^[a-f0-9]{64}$/.test(descriptor.sha256) ||
      !mediaTypes.includes(descriptor.mime_type)
    ) {
      throw new Error(`WanGP returned an invalid or unsupported ${kind} download descriptor.`);
    }
    const url = new URL(descriptor.download_url, this.#endpoint);
    if (url.origin !== new URL(this.#endpoint).origin || url.username || url.password || url.hash) {
      throw new Error(`${kind} transfer must remain on the configured MCP origin.`);
    }
    const response = await fetch(url, {
      redirect: 'error',
      signal: this.#signal(signal),
    });
    if (response.status !== 200) {
      throw new Error(`${kind} transfer returned HTTP ${response.status}.`);
    }
    if (response.headers.get('content-type')?.split(';')[0] !== descriptor.mime_type) {
      throw new Error(`${kind} transfer media type does not match its descriptor.`);
    }
    const bytes = await readBytes(response, descriptor.size_bytes);
    const sha256 = createHash('sha256').update(bytes).digest('hex');
    if (bytes.length !== descriptor.size_bytes || sha256 !== descriptor.sha256) {
      throw new Error(`${kind} transfer length or SHA-256 does not match its descriptor.`);
    }
    return { bytes, sha256, media_type: descriptor.mime_type };
  }

  async close() {
    if (!this.#session) return;
    const response = await fetch(this.#endpoint, {
      method: 'DELETE',
      headers: this.#headers(),
      redirect: 'error',
      signal: AbortSignal.timeout(this.#timeoutMs),
    });
    if (!response.ok) throw new Error(`MCP session close returned HTTP ${response.status}.`);
    this.#session = undefined;
  }
}

async function main() {
  const { values } = parseArgs({
    options: {
      endpoint: { type: 'string' },
      tool: { type: 'string' },
      arguments: { type: 'string' },
      'arguments-file': { type: 'string' },
      'result-file': { type: 'string' },
      download: { type: 'string' },
      'download-video': { type: 'string' },
      'agent-context': { type: 'string' },
      output: { type: 'string' },
    },
  });
  if (!values.endpoint) {
    throw new Error('Usage: node mcp.mjs --endpoint <MCP URL> [--tool <name> --arguments <JSON>]');
  }
  if (values.arguments && values['arguments-file']) {
    throw new Error('Choose --arguments or --arguments-file, not both.');
  }
  if (values.download && values['download-video']) {
    throw new Error('Choose an image download or a video download, not both.');
  }
  const download = values.download || values['download-video'];
  if (download && (values.tool || values.arguments || values['arguments-file'])) {
    throw new Error('Choose a tool call or a media download, not both.');
  }
  if (Boolean(download) !== Boolean(values.output)) {
    throw new Error('Media downloads require a download media ID and --output <file>.');
  }
  const args = JSON.parse(
    values['arguments-file']
      ? await readFile(values['arguments-file'], 'utf8')
      : (values.arguments ?? '{}'),
  );
  if (args === null || typeof args !== 'object' || Array.isArray(args)) {
    throw new Error('Tool arguments must be a JSON object.');
  }
  const client = new WanGpClient(values.endpoint);
  try {
    await client.initialize();
    let result;
    if (download) {
      const media = values['download-video']
        ? await client.downloadVideo(download, values['agent-context'])
        : await client.downloadImage(download, values['agent-context']);
      await writeFile(values.output, media.bytes, { flag: 'wx' });
      result = {
        output: values.output,
        bytes: media.bytes.length,
        sha256: media.sha256,
        media_type: media.media_type,
      };
    } else {
      result = values.tool ? await client.call(values.tool, args) : await client.tools();
    }
    const text = JSON.stringify(result, null, 2);
    if (values['result-file']) {
      await writeFile(values['result-file'], text + '\n', { flag: 'wx', mode: 0o600 });
    }
    console.log(text);
  } finally {
    await client.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
