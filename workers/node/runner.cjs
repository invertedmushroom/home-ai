// This program belongs in the worker image. Never run project code on the host.
const fs = require('node:fs/promises');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { stopDescendants, collectOutputs } = require('./outputs.cjs');

if (process.pid !== 1 || process.getuid?.() !== 0)
  throw new Error('Worker must run as namespace PID 1.');

const LIMIT = 65536;
// PID 1 must outlive and be unsignalable by the unprivileged project process.
// Its exit tears down the PID namespace even if the application server crashes.
setTimeout(() => process.exit(124), 15000).unref();
process.umask(0);
function capture(stream) {
  const chunks = [];
  let bytes = 0;
  let truncated = false;
  stream.on('data', (chunk) => {
    const remaining = LIMIT - bytes;
    if (chunk.length > remaining) truncated = true;
    if (remaining > 0) {
      const kept = chunk.subarray(0, remaining);
      chunks.push(kept);
      bytes += kept.length;
    }
  });
  return () => ({ text: Buffer.concat(chunks).toString('utf8'), truncated });
}

(async () => {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    if (bytes > 8 * 1024 * 1024) throw new Error('Input exceeds worker budget.');
    chunks.push(chunk);
  }
  const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  if (
    !['check', 'run'].includes(input.action) ||
    !Array.isArray(input.files) ||
    input.files.length > 64
  )
    throw new Error('Invalid worker input.');
  for (const file of input.files) {
    if (
      typeof file.content !== 'string' ||
      typeof file.path !== 'string' ||
      !/^[A-Za-z0-9._/-]{1,240}$/.test(file.path) ||
      file.path.split('/').some((part) => !part || part === '.' || part === '..')
    )
      throw new Error('Invalid project path.');
    const target = path.join('/work', file.path);
    await fs.mkdir(path.dirname(target), { recursive: true, mode: 0o777 });
    await fs.writeFile(target, file.content, { flag: 'wx', mode: 0o666 });
  }
  if (
    !input.files.some((file) => file.path === input.entry) ||
    !/\.(?:js|mjs|cjs)$/.test(input.entry)
  )
    throw new Error('Select a project JavaScript entry.');
  const args =
    input.action === 'check' ? ['--check', '/work/' + input.entry] : ['/work/' + input.entry];
  const child = spawn(process.execPath, args, {
    uid: 65534,
    gid: 65534,
    cwd: '/work',
    env: {
      PATH: '/usr/local/bin:/usr/bin:/bin',
      HOME: '/work',
      TMPDIR: '/work',
      DEEP_OUTPUT_DIR: '/output',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const stdout = capture(child.stdout);
  const stderr = capture(child.stderr);
  const closed = new Promise((resolve) => child.once('close', resolve));
  const exited = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', (code, signal) => resolve({ code, signal }));
  });
  const { code, signal } = await exited;
  let files = [];
  let artifact_error = null;
  try {
    await stopDescendants();
    await closed;
    if (code === 0 && input.action === 'run') files = await collectOutputs();
  } catch {
    artifact_error =
      'Output capture failed: all project tasks must stop; /output and /work/output must contain distinct portable regular UTF-8 files within the shared limits.';
  }
  const out = stdout();
  const err = stderr();
  process.stdout.write(
    JSON.stringify({
      exit_code: code,
      signal,
      stdout: out.text,
      stderr: err.text,
      truncated: out.truncated || err.truncated,
      files,
      artifact_error,
    }),
  );
})().catch(() => {
  process.stderr.write('Worker could not prepare this project.');
  process.exitCode = 1;
});
