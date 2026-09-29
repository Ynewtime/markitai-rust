'use strict';

const native = require('./markitai.node');

class ConversionError extends Error {
  constructor(message, code, usage) {
    super(message);
    this.name = 'ConversionError';
    this.code = code;
    this.usage = usage ?? undefined;
  }
}

function request(source, options) {
  if (typeof source !== 'string') throw new TypeError('source must be a string');
  if (options === null || typeof options !== 'object' || Array.isArray(options)) {
    throw new TypeError('options must be an object');
  }
  return JSON.stringify({ source, options });
}

function result(response) {
  const envelope = JSON.parse(response);
  if (!envelope.ok) {
    throw new ConversionError(envelope.error.message, envelope.error.code, envelope.error.usage);
  }
  return envelope.result;
}

async function convert(source, options = {}) {
  return result(await native.convertJson(request(source, options)));
}

function convertSync(source, options = {}) {
  return result(native.convertJsonSync(request(source, options)));
}

module.exports = { convert, convertSync, ConversionError, version: native.version() };
