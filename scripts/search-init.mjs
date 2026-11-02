// Separate from application .env: the search container receives no chat-model keys.
import { mkdir, writeFile } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';

const directory = new URL('../data/search/', import.meta.url);
await mkdir(directory, { recursive: true });
try {
  await writeFile(
    new URL('.env', directory),
    `DEEP_SEARCH_SECRET=${randomBytes(32).toString('hex')}\n`,
    { flag: 'wx', mode: 0o600 },
  );
  console.log('Created the ignored search-service environment.');
} catch (error) {
  if (error.code !== 'EEXIST') throw error;
  console.log('Preserved the existing search-service environment.');
}
