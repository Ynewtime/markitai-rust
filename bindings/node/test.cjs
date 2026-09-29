'use strict';

const { test, after } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'markitai-node-'));
process.env.MARKITAI_HOME = path.join(root, 'home');
const markitai = require('./index.cjs');
const source = path.join(root, '文档.md');
fs.writeFileSync(source, '# Node\n\n你好 🌍\n');
after(() => fs.rmSync(root, { recursive: true, force: true }));

test('sync and concurrent async calls preserve Unicode and result shape', async () => {
  assert.ok(markitai.version);
  const sync = markitai.convertSync(source, { config: {}, llm: false });
  assert.match(sync.markdown, /你好 🌍/);
  assert.equal(sync.output_path, null);
  const outputs = await Promise.all(Array.from({ length: 24 }, () => markitai.convert(source, { config: {}, llm: false })));
  for (const output of outputs) {
    assert.equal(output.markdown, sync.markdown);
    assert.equal(output.source, source);
    assert.equal(output.usage.requests, 0);
  }
});

test('native output files and structured errors', async () => {
  const output = await markitai.convert(source, { config: {}, output_dir: path.join(root, 'out'), llm: false });
  assert.ok(fs.existsSync(output.output_path));
  await assert.rejects(markitai.convert(path.join(root, 'missing.md'), { config: {}, llm: false }), (error) => {
    assert.ok(error instanceof markitai.ConversionError);
    assert.equal(typeof error.code, 'string');
    return true;
  });
  await assert.rejects(markitai.convert(source, { config: {}, unknown_option: true }), markitai.ConversionError);
  await assert.rejects(markitai.convert(null), TypeError);
  assert.throws(() => markitai.convertSync(source, null), TypeError);
});

test('async native fetch leaves the JavaScript event loop available', async () => {
  let requests = 0;
  const server = http.createServer((_request, response) => {
    requests++;
    response.writeHead(200, { 'Content-Type': 'text/html' });
    response.end('<html><article><h1>Native HTTP</h1><p>The Node server ran.</p></article></html>');
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    const url = `http://127.0.0.1:${server.address().port}/test`;
    const output = await markitai.convert(url, { config: {}, llm: false });
    assert.match(output.markdown, /The Node server ran\./);
    const cached = await markitai.convert(url, { config: {}, llm: false });
    assert.equal(cached.markdown, output.markdown);
    assert.equal(requests, 1);
    assert.equal(Object.hasOwn(cached, 'fetch_cache_hit'), false);
  } finally {
    await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});

test('older producer errors and constructor preserve optional usage', async () => {
  const vm = require('node:vm');
  const paid = { cost_usd: 0, requests: 1, input_tokens: 0, output_tokens: 0, by_model: { fixture: { requests: 1 } } };
  for (const usage of [undefined, paid]) {
    const error = { message: 'original message', code: 'conversion_error' };
    if (usage !== undefined) error.usage = usage;
    const encoded = JSON.stringify({ ok: false, error });
    const module = { exports: {} };
    vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'index.cjs'), 'utf8'), {
      module, require: (name) => {
        assert.equal(name, './markitai.node');
        return { convertJson: async () => encoded, convertJsonSync: () => encoded, version: () => 'fixture' };
      },
    });
    const wrapper = module.exports;
    const verify = (failure) => {
      assert.ok(failure instanceof wrapper.ConversionError);
      assert.equal(failure.message, 'original message');
      assert.equal(failure.code, 'conversion_error');
      assert.equal(JSON.stringify(failure.usage), JSON.stringify(usage));
      return true;
    };
    assert.throws(() => wrapper.convertSync('unused.md'), verify);
    await assert.rejects(wrapper.convert('unused.md'), verify);
  }
  const old = new markitai.ConversionError('old message', 'old_code');
  assert.equal(old.message, 'old message');
  assert.equal(old.code, 'old_code');
  assert.equal(old.usage, undefined);
});

test('paid native failures retain separate document accounting including zero tokens', async () => {
  const files = new Map(['TERMALPHA', 'TERMBETA', 'TERMZERO'].map((name) => {
    const file = path.join(root, `${name}.md`);
    fs.writeFileSync(file, `# ${name}\n\nComplete independent source document ${name}.\n`);
    return [name, file];
  }));
  const requests = [], errors = [];
  let entered = 0, release;
  const gate = new Promise((resolve) => { release = resolve; });
  const server = http.createServer(async (request, response) => {
    try {
      let raw = '';
      for await (const chunk of request) {
        raw += chunk;
        assert.ok(raw.length < 1024 * 1024);
      }
      const payload = JSON.parse(raw);
      const text = JSON.stringify(payload.messages);
      const name = [...files.keys()].find((key) => text.includes(key));
      assert.ok(name);
      requests.push(name);
      if (name !== 'TERMZERO') {
        entered++;
        if (entered === 2) release();
        await gate;
      }
      const [input, output] = { TERMALPHA: [11, 3], TERMBETA: [29, 7], TERMZERO: [0, 0] }[name];
      response.writeHead(401, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify({ error: { message: 'PRIVATE RESPONSE SECRET' }, model: name, usage: { prompt_tokens: input, completion_tokens: output } }));
    } catch (error) {
      errors.push(String(error));
      response.writeHead(500).end();
    }
  });
  server.requestTimeout = 15000;
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  let gateTimedOut = false;
  const timeout = setTimeout(() => { gateTimedOut = true; release(); }, 12000);
  const config = {
    cache: { enabled: false, global_dir: path.join(root, 'paid-cache') },
    prompts: { dir: path.join(root, 'prompts') }, history: { record: false },
    ocr: { enabled: false }, image: { alt_enabled: false, desc_enabled: false },
    llm: { enabled: true, on_failure: 'fail', max_requests_per_document: 1,
      router_settings: { num_retries: 0, timeout: 15 },
      model_list: [{ model_name: 'fixture', litellm_params: { model: 'openai/fixture', api_key: 'synthetic-key', api_base: `http://127.0.0.1:${server.address().port}/v1` } }],
    },
  };
  try {
    const failure = async (name) => {
      try { await markitai.convert(files.get(name), { config }); }
      catch (error) { return error; }
      assert.fail('paid native failure unexpectedly succeeded');
    };
    const [alpha, beta] = await Promise.all([failure('TERMALPHA'), failure('TERMBETA')]);
    const zero = await failure('TERMZERO');
    for (const [name, error, input, output] of [['TERMALPHA', alpha, 11, 3], ['TERMBETA', beta, 29, 7], ['TERMZERO', zero, 0, 0]]) {
      assert.ok(error instanceof markitai.ConversionError);
      assert.equal(error.code, 'conversion_error');
      assert.match(error.message, /HTTP 401/);
      assert.ok(!error.message.includes('PRIVATE RESPONSE SECRET'));
      assert.ok(error.usage);
      assert.deepEqual([error.usage.requests, error.usage.input_tokens, error.usage.output_tokens], [1, input, output]);
      assert.deepEqual(Object.keys(error.usage.by_model), [name]);
    }
    assert.equal(entered, 2);
    assert.equal(gateTimedOut, false, 'both document requests reached the server together');
    assert.deepEqual(errors, []);
    assert.deepEqual(requests.sort(), [...files.keys()].sort());
    await assert.rejects(markitai.convert(path.join(root, 'missing-paid.md'), { config: {}, llm: false }), (error) => {
      assert.ok(error instanceof markitai.ConversionError);
      assert.equal(error.usage, undefined);
      return true;
    });
  } finally {
    clearTimeout(timeout);
    release();
    await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});
