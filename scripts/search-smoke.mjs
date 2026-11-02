// Explicit opt-in developer probe. This is not wired into the application or models.
import { parseArgs } from 'node:util';
import { searchProbe } from '../tools/search/probe.mjs';

const controller = new AbortController();
const abort = () => controller.abort();
process.once('SIGINT', abort);
process.once('SIGTERM', abort);
process.once('SIGHUP', abort);
try {
  const { values } = parseArgs({
    options: {
      provider: { type: 'string', default: 'duckduckgo' },
      query: { type: 'string' },
      browser: { type: 'string' },
      'searxng-url': { type: 'string' },
      engine: { type: 'string' },
      model: { type: 'string' },
    },
  });
  if (values.provider === 'openai') {
    try {
      process.loadEnvFile();
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
  const result = await searchProbe(
    { provider: values.provider, query: values.query },
    {
      signal: controller.signal,
      executablePath: values.browser ?? process.env.DEEP_BROWSER_PATH,
      baseUrl: values['searxng-url'],
      engine: values.engine,
      apiKey: values.provider === 'openai' ? process.env.OPENAI_API_KEY : undefined,
      model:
        values.model ??
        (values.provider === 'openai' ? process.env.DEEP_OPENAI_SEARCH_MODEL : undefined),
    },
  );
  await new Promise((resolve, reject) =>
    process.stdout.write(JSON.stringify(result) + '\n', (error) =>
      error ? reject(error) : resolve(),
    ),
  );
  process.exitCode =
    result.status === 'succeeded'
      ? 0
      : result.code === 'invalid_input' || result.code === 'invalid_config'
        ? 2
        : 1;
  if (result.code === 'cleanup_failed') process.exit(1);
} catch {
  console.error(
    'Use --provider duckduckgo|searxng|openai --query "plain search terms". SearXNG requires --searxng-url and --engine. OpenAI requires OPENAI_API_KEY and --model or DEEP_OPENAI_SEARCH_MODEL.',
  );
  process.exitCode = 2;
} finally {
  process.removeListener('SIGINT', abort);
  process.removeListener('SIGTERM', abort);
  process.removeListener('SIGHUP', abort);
}
