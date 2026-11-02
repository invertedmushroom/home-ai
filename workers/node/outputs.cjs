// Trusted worker code. Quiescence must be established before walking /output.
const fs = require('node:fs/promises');
const { constants } = require('node:fs');
const path = require('node:path');

function validPath(value) {
  return (
    /^[A-Za-z0-9._/-]{1,240}$/.test(value) &&
    value
      .split('/')
      .every(
        (part) =>
          part &&
          part !== '.' &&
          part !== '..' &&
          !part.endsWith('.') &&
          !/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/i.test(part.split('.')[0]),
      )
  );
}

async function gone(pid) {
  try {
    await fs.lstat(`/proc/${pid}`);
    return false;
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ESRCH') return true;
    throw error;
  }
}

async function stopDescendants() {
  if (process.pid !== 1 || process.getuid() !== 0)
    throw new Error('Supervisor must be namespace PID 1.');
  const deadline = performance.now() + 2000;
  for (;;) {
    try {
      process.kill(-1, 'SIGKILL');
    } catch (error) {
      if (error.code !== 'ESRCH') throw error;
    }
    let live = false;
    for (const pid of await fs.readdir('/proc')) {
      if (!/^\d+$/.test(pid) || pid === '1') continue;
      try {
        // A dead group leader is not proof its other threads stopped.
        const tasks = await fs.readdir(`/proc/${pid}/task`);
        if (!tasks.length && !(await gone(pid))) live = true;
        for (const tid of tasks) {
          try {
            const stat = await fs.readFile(`/proc/${pid}/task/${tid}/stat`, 'utf8');
            // comm may contain parentheses/newlines. Only the final ')' is the
            // delimiter; the numeric/stat tail cannot contain one.
            const tail = stat.slice(stat.lastIndexOf(')') + 1);
            if (!/^ [ZX] /.test(tail)) live = true;
          } catch (error) {
            if (error.code !== 'ENOENT' && error.code !== 'ESRCH') throw error;
          }
        }
      } catch (error) {
        if ((error.code !== 'ENOENT' && error.code !== 'ESRCH') || !(await gone(pid))) throw error;
      }
    }
    if (!live) return;
    if (performance.now() >= deadline) throw new Error('Project tasks did not stop.');
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function collectOutputs() {
  const files = [];
  const paths = new Set();
  let bytes = 0;
  let entries = 0;
  async function visit(root, relative, depth) {
    if (depth > 16) throw new Error('Output nesting exceeds budget.');
    const names = await fs.readdir(path.join(root, relative));
    entries += names.length;
    if (entries > 256) throw new Error('Output traversal exceeds budget.');
    for (const name of names.sort()) {
      const filePath = relative ? `${relative}/${name}` : name;
      if (!validPath(filePath)) throw new Error('Output path is not portable.');
      const full = path.join(root, filePath);
      const stat = await fs.lstat(full);
      if (stat.isDirectory()) {
        await visit(root, filePath, depth + 1);
        continue;
      }
      if (!stat.isFile() || stat.nlink !== 1)
        throw new Error('Output must be a regular file with one link.');
      if (files.length >= 64 || paths.has(filePath.toLowerCase()))
        throw new Error('Output count or paths exceed budget.');
      const remaining = 1048576 - bytes - Buffer.byteLength(filePath);
      if (stat.size > remaining) throw new Error('Output bytes exceed budget.');
      const handle = await fs.open(
        full,
        constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
      );
      let data;
      try {
        const actual = await handle.stat();
        if (!actual.isFile() || actual.nlink !== 1 || actual.size !== stat.size)
          throw new Error('Output identity changed.');
        const buffer = Buffer.alloc(actual.size + 1);
        let count = 0;
        while (count < buffer.length) {
          const read = await handle.read(buffer, count, buffer.length - count, count);
          if (!read.bytesRead) break;
          count += read.bytesRead;
        }
        if (count !== actual.size) throw new Error('Output size changed.');
        data = buffer.subarray(0, count);
      } finally {
        await handle.close();
      }
      const content = data.toString('utf8');
      if (!Buffer.from(content, 'utf8').equals(data)) throw new Error('Output is not UTF-8 text.');
      bytes += Buffer.byteLength(filePath) + data.length;
      paths.add(filePath.toLowerCase());
      files.push({ path: filePath, content });
    }
  }
  // Both are explicit container-local output directories. They share every
  // budget; duplicate paths are rejected rather than choosing one silently.
  for (const root of ['/output', '/work/output']) {
    let stat;
    try {
      stat = await fs.lstat(root);
    } catch (error) {
      if (root === '/work/output' && error.code === 'ENOENT') continue;
      throw error;
    }
    if (!stat.isDirectory()) throw new Error('Output root must be a directory, not a link.');
    await visit(root, '', 0);
  }
  for (const name of paths) {
    const parts = name.split('/');
    for (let i = 1; i < parts.length; i++)
      if (paths.has(parts.slice(0, i).join('/'))) throw new Error('Output paths conflict.');
  }
  return files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

module.exports = { stopDescendants, collectOutputs };
