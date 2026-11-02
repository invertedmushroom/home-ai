import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { chromium } from '@playwright/test';
import {
  closeBrowser,
  duckDuckGoResult,
  normalizeResults,
  readDuckDuckGoDocument,
  searchProbe,
  searxngEndpoint,
  searxngResult,
  sourceUrl,
  validQuery,
} from '../../tools/search/probe.mjs';

test('queries and candidate URLs cannot become routing instructions or active/local links', () => {
  for (const query of [
    '',
    'x'.repeat(513),
    '界'.repeat(171),
    'hello\nworld',
    '!g secrets',
    ':all query',
    '\\first',
  ])
    assert.equal(validQuery(query), false, query);
  assert(validQuery('site:rust-lang.org ownership 界'));
  for (const link of [
    'javascript:alert(1)',
    'data:text/html,x',
    'file:///c:/secret',
    'https://user:secret@example.com',
    'http://127.0.0.1',
    'http://0x7f000001',
    'http://[::1]/',
    'https://localhost/x',
    'https://private.local/x',
    'https://docs.example.com:8443/x',
    'https://example.com/\n',
    'http://localhost./',
    'http://printer.local./',
    'https://foo..com/',
    'https://-foo.com/',
    'https://foo-.com/',
  ])
    assert.equal(sourceUrl(link), null, link);
  const destination = 'https://docs.rust-lang.org/book/a%2Fb?x=%25#section';
  const wrapper = '//duckduckgo.com/l/?uddg=' + encodeURIComponent(destination);
  assert.equal(sourceUrl(wrapper, true), 'https://docs.rust-lang.org/book/a%2Fb?x=%25');
  assert.equal(
    sourceUrl('https://user@duckduckgo.com/l/?uddg=' + encodeURIComponent(destination), true),
    null,
  );
});

test('results are bounded, deduplicated, and preserve literal text', () => {
  const items = Array.from({ length: 20 }, (_, i) => ({
    title: '界'.repeat(500),
    url: `https://docs.rust-lang.org/${i}`,
    snippet: '<script>alert(1)</script>' + '🙂'.repeat(1000),
  }));
  items.unshift(items[0]);
  const result = normalizeResults(items);
  assert.equal(result.length, 5);
  assert.equal(new Set(result.map((r) => r.url)).size, 5);
  assert.equal([...result[0].title].length, 200);
  assert.equal([...result[0].snippet].length, 700);
  assert(result[0].snippet.startsWith('<script>'));
});

test('blocked and malformed pages never become a successful empty search', () => {
  assert.equal(duckDuckGoResult(403, { kind: 'empty' }).code, 'blocked');
  assert.equal(duckDuckGoResult(429, { kind: 'results', items: [] }).code, 'rate_limited');
  assert.equal(duckDuckGoResult(202, { kind: 'empty' }).code, 'challenge');
  assert.equal(duckDuckGoResult(200, { kind: 'unknown' }).code, 'layout_changed');
  assert.equal(
    duckDuckGoResult(200, { kind: 'results', items: [{ title: 'bad', url: 'javascript:x' }] }).code,
    'invalid_results',
  );
  assert.equal(duckDuckGoResult(200, { kind: 'empty', items: [] }).status, 'succeeded');
});

test('SearXNG configuration is one explicit engine on a literal loopback endpoint', () => {
  for (const value of [
    'https://example.com',
    'http://localhost:8888',
    'http://user:pass@127.0.0.1',
    'http://127.0.0.1/path',
    'http://127.0.0.1/?secret=1',
    'http://127.0.0.1/#x',
  ])
    assert.throws(() => searxngEndpoint(value, 'wikipedia', 'rust'));
  for (const engine of ['', ' ', 'google,bing', ' google'])
    assert.throws(() => searxngEndpoint('http://127.0.0.1:8888', engine, 'rust'));
  const url = searxngEndpoint('http://127.0.0.1:8888', 'wikipedia', 'rust & lang');
  assert.equal(url.pathname, '/search');
  assert.equal(url.searchParams.get('q'), 'rust & lang');
  assert.equal(url.searchParams.get('engines'), 'wikipedia');
  assert.equal(url.searchParams.get('format'), 'json');
});

test('SearXNG engine failures, missing evidence and unexpected engines are explicit failures', () => {
  assert.equal(
    searxngResult(200, { results: [], unresponsive_engines: [], infoboxes: [{}] }, 'wikipedia')
      .code,
    'unsupported_result_type',
  );
  assert.equal(
    searxngResult(
      200,
      { results: [], unresponsive_engines: [['duckduckgo', 'CAPTCHA']] },
      'duckduckgo',
    ).code,
    'upstream_failed',
  );
  assert.equal(searxngResult(200, { results: [] }, 'duckduckgo').code, 'invalid_response');
  assert.equal(
    searxngResult(
      200,
      {
        results: [{ engine: 'bing', url: 'https://example.com', title: 'x' }],
        unresponsive_engines: [],
      },
      'duckduckgo',
    ).code,
    'unexpected_engine',
  );
  assert.deepEqual(searxngResult(200, { results: [], unresponsive_engines: [] }, 'duckduckgo'), {
    status: 'succeeded',
    results: [],
  });
  const good = {
    engine: 'brave',
    title: 'Good',
    url: 'https://www.rust-lang.org/',
    content: 'Text',
  };
  for (const bad of [
    { ...good, url: 42, title: [] },
    { ...good, engines: 'brave' },
    { ...good, content: 42 },
  ])
    assert.equal(
      searxngResult(200, { results: [good, bad], unresponsive_engines: [] }, 'brave').status,
      'failed',
    );
});

async function service(handler, action) {
  const server = createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    return await action(`http://127.0.0.1:${server.address().port}`);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

test('real loopback HTTP returns keyless normalized results with no cookies or auth', async () => {
  let calls = 0;
  await service(
    (request, response) => {
      calls++;
      const url = new URL(request.url, 'http://127.0.0.1');
      assert.equal(request.headers.authorization, undefined);
      assert.equal(request.headers.cookie, undefined);
      assert.equal(url.pathname, '/search');
      assert.equal(url.searchParams.get('engines'), 'wikipedia');
      response.writeHead(200, { 'Content-Type': 'application/json' });
      response.end(
        JSON.stringify({
          results: [
            {
              engine: 'wikipedia',
              engines: ['wikipedia'],
              title: 'Rust',
              url: 'https://www.rust-lang.org/',
              content: 'A language.',
            },
          ],
          unresponsive_engines: [],
        }),
      );
    },
    async (baseUrl) => {
      const result = await searchProbe(
        { provider: 'searxng', query: 'Rust' },
        { baseUrl, engine: 'wikipedia' },
      );
      assert.equal(result.status, 'succeeded');
      assert.equal(result.results[0].url, 'https://www.rust-lang.org/');
      assert.equal(result.query, 'Rust');
      assert(result.elapsed_ms >= 0);
    },
  );
  assert.equal(calls, 1);
});

test('redirects are not followed and invalid or oversized bodies fail', async () => {
  for (const kind of ['redirect', 'html', 'json', 'utf8', 'oversized']) {
    let calls = 0;
    await service(
      (request, response) => {
        calls++;
        if (kind === 'redirect') {
          response.writeHead(302, { location: '/other' });
          response.end();
          return;
        }
        response.writeHead(200, {
          'Content-Type': kind === 'html' ? 'text/html' : 'application/json',
        });
        response.end(
          kind === 'html'
            ? '<html>blocked</html>'
            : kind === 'json'
              ? '{'
              : kind === 'utf8'
                ? Buffer.from([123, 34, 120, 34, 58, 34, 255, 34, 125])
                : 'x'.repeat(262145),
        );
      },
      async (baseUrl) => {
        const result = await searchProbe(
          { provider: 'searxng', query: 'Rust' },
          { baseUrl, engine: 'wikipedia' },
        );
        assert.equal(result.status, 'failed', kind);
        assert.equal(result.results.length, 0);
        assert.equal(calls, 1);
      },
    );
  }
});

test('pre-cancelled requests make no traffic; in-flight cancellation closes the request', async () => {
  let calls = 0;
  let disconnected;
  const closed = new Promise((resolve) => {
    disconnected = resolve;
  });
  await service(
    (request) => {
      calls++;
      request.on('close', disconnected);
    },
    async (baseUrl) => {
      const cancelled = new AbortController();
      cancelled.abort();
      const first = await searchProbe(
        { provider: 'searxng', query: 'Rust' },
        { baseUrl, engine: 'wikipedia', signal: cancelled.signal },
      );
      assert.equal(first.code, 'cancelled');
      assert.equal(calls, 0);
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), 100);
      const result = await searchProbe(
        { provider: 'searxng', query: 'Rust' },
        { baseUrl, engine: 'wikipedia', signal: controller.signal },
      );
      clearTimeout(timer);
      assert.equal(result.code, 'cancelled');
      assert.equal(calls, 1);
      await closed;
    },
  );
});

test('SearXNG stalled responses reach the independent eight-second deadline', async () => {
  await service(
    () => {},
    async (baseUrl) => {
      const result = await searchProbe(
        { provider: 'searxng', query: 'Rust' },
        { baseUrl, engine: 'wikipedia' },
      );
      assert.equal(result.code, 'timeout');
      assert(result.elapsed_ms >= 7900 && result.elapsed_ms < 12000);
    },
  );
});

test('graceful browser-close failure falls back to killing the owned process', async () => {
  let killed = 0;
  const child = { exitCode: null, signalCode: null };
  await closeBrowser({
    close: async () => {
      throw Error('disconnected');
    },
    kill: async () => {
      killed++;
      child.signalCode = 'SIGKILL';
    },
    process: () => child,
  });
  assert.equal(killed, 1);
});

test('failed browser termination is bounded and never reported as successful cleanup', async () => {
  const started = performance.now();
  await assert.rejects(
    closeBrowser({
      close: async () => {
        throw Error('failed');
      },
      kill: () => new Promise(() => {}),
      process: () => ({ exitCode: null, signalCode: null }),
    }),
    /deadline/,
  );
  assert(performance.now() - started < 3000);
});

test('the real owned Chromium server confirms process exit during cleanup', async () => {
  const server = await chromium.launchServer({
    host: '127.0.0.1',
    headless: true,
    chromiumSandbox: true,
  });
  const child = server.process();
  assert(child.pid);
  try {
    await closeBrowser(server);
    assert(child.exitCode !== null || child.signalCode !== null);
  } finally {
    if (child.exitCode === null && child.signalCode === null) await server.kill();
  }
});

test('ambient proxy settings cannot receive the loopback search query', async () => {
  let proxyCalls = 0;
  let directCalls = 0;
  await service(
    (_, response) => {
      proxyCalls++;
      response.end('proxy');
    },
    async (proxy) => {
      await service(
        (_, response) => {
          directCalls++;
          response.writeHead(200, { 'Content-Type': 'application/json' });
          response.end(JSON.stringify({ results: [], unresponsive_engines: [] }));
        },
        async (baseUrl) => {
          const child = spawn(
            process.execPath,
            [
              '--use-env-proxy',
              'scripts/search-smoke.mjs',
              '--provider',
              'searxng',
              '--query',
              'private query',
              '--searxng-url',
              baseUrl,
              '--engine',
              'wikipedia',
            ],
            {
              env: {
                ...process.env,
                HTTP_PROXY: proxy,
                http_proxy: proxy,
                HTTPS_PROXY: proxy,
                https_proxy: proxy,
                NO_PROXY: '',
                no_proxy: '',
              },
              windowsHide: true,
            },
          );
          let stdout = '';
          child.stdout.on('data', (chunk) => {
            stdout += chunk;
          });
          let stderr = '';
          child.stderr.on('data', (chunk) => {
            stderr += chunk;
          });
          const code = await new Promise((resolve, reject) => {
            child.on('error', reject);
            child.on('close', resolve);
          });
          assert.equal(code, 0, stderr);
          assert.equal(JSON.parse(stdout).status, 'succeeded');
        },
      );
    },
  );
  assert.equal(proxyCalls, 0);
  assert.equal(directCalls, 1);
});

test('ambient Selenium launch steering is rejected before browser or query traffic', async () => {
  let calls = 0;
  await service(
    (_, response) => {
      calls++;
      response.end('{}');
    },
    async (baseUrl) => {
      const child = spawn(
        process.execPath,
        ['scripts/search-smoke.mjs', '--query', 'private query'],
        {
          env: {
            ...process.env,
            SELENIUM_REMOTE_URL: baseUrl,
            SELENIUM_REMOTE_HEADERS: JSON.stringify({ Authorization: 'synthetic secret' }),
          },
          windowsHide: true,
        },
      );
      let stdout = '';
      child.stdout.on('data', (chunk) => {
        stdout += chunk;
      });
      const code = await new Promise((resolve, reject) => {
        child.on('error', reject);
        child.on('close', resolve);
      });
      assert.equal(code, 1);
      assert.equal(JSON.parse(stdout).code, 'invalid_environment');
    },
  );
  assert.equal(calls, 0);
});

test('browser control rejects ambient proxy settings before launch', async () => {
  let calls = 0;
  await service(
    (_, response) => {
      calls++;
      response.end('{}');
    },
    async (proxy) => {
      const child = spawn(
        process.execPath,
        ['--use-env-proxy', 'scripts/search-smoke.mjs', '--query', 'private query'],
        {
          env: {
            ...process.env,
            HTTP_PROXY: proxy,
            HTTPS_PROXY: proxy,
            ALL_PROXY: proxy,
            NO_PROXY: '',
            no_proxy: '',
          },
          windowsHide: true,
        },
      );
      let stdout = '';
      child.stdout.on('data', (chunk) => {
        stdout += chunk;
      });
      const code = await new Promise((resolve, reject) => {
        child.on('error', reject);
        child.on('close', resolve);
      });
      assert.equal(code, 1);
      assert.equal(JSON.parse(stdout).code, 'invalid_environment');
    },
  );
  assert.equal(calls, 0);
});

let browser;
before(async () => {
  browser = await chromium.launch({ headless: true, chromiumSandbox: true });
});
after(async () => {
  await browser?.close();
});

test('real DOM distinguishes results, explicit empty, challenge and selector breakage', async () => {
  const context = await browser.newContext({ javaScriptEnabled: false, serviceWorkers: 'block' });
  await context.route('**/*', (route) => route.abort());
  const page = await context.newPage();
  try {
    const fixtures = [
      [
        '<div class="result"><a class="result__a" href="https://docs.rust-lang.org/">CAPTCHA examples</a><p class="result__snippet">What is a captcha?</p></div>',
        'results',
      ],
      ['<div class="no-results">No results found for this query</div>', 'empty'],
      [
        '<form id="challenge-form">Verify you are human</form><div class="no-results">No results found</div>',
        'challenge',
      ],
      ['<p>Unknown template</p>', 'unknown'],
    ];
    for (const [html, kind] of fixtures) {
      assert.equal((await page.evaluate(readDuckDuckGoDocument, html)).kind, kind);
    }
  } finally {
    await context.close();
  }
});

test('offline HTML parsing cannot navigate or fetch injected result resources', async () => {
  let fetched = 0;
  await service(
    (_, response) => {
      fetched++;
      response.end('unexpected');
    },
    async (base) => {
      const context = await browser.newContext({
        javaScriptEnabled: false,
        serviceWorkers: 'block',
      });
      await context.route('**/*', (route) => route.abort());
      const page = await context.newPage();
      try {
        const html = `<meta http-equiv="refresh" content="0;url=${base}/redirect"><script>window.injected=true;fetch('${base}/script')</script>
        <img src="${base}/image"><iframe src="${base}/frame"></iframe><link rel="stylesheet" href="${base}/style">
        <div class="result"><a class="result__a" href="https://docs.rust-lang.org/">Rust</a><p class="result__snippet">Text</p></div>`;
        const result = await page.evaluate(readDuckDuckGoDocument, html);
        assert.equal(result.kind, 'results');
        await delay(100);
        assert.equal(page.url(), 'about:blank');
        assert.equal(await page.evaluate(() => window.injected), undefined);
        assert.equal(fetched, 0);
      } finally {
        await context.close();
      }
    },
  );
});
