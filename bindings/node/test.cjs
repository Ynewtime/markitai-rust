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
