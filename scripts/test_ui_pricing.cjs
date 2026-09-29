// Exercise the shipped pure cost labels; DOM rendering has separate validation.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../crates/markitai-cli/src/server/web/app.js'), 'utf8');
const start = source.indexOf('function attemptPricing(');
const end = source.indexOf('export async function confirmDelete(', start);
assert.ok(start >= 0 && end > start);
const helpers = vm.runInNewContext(`${source.slice(start, end)}
({attemptPricing, priceText, attemptNotice})`);
const {attemptPricing, priceText, attemptNotice} = helpers;
let checks = 0;
function eq(actual, expected) { assert.deepEqual(JSON.parse(JSON.stringify(actual)), expected); checks++; }
const old = {cost_usd:0.5, error:null, pricing:{cost_status:'complete'}};
const usage = {requests:1,cost_usd:0,by_model:{legacy:{requests:1,cost_usd:0}}};
const failed = {...old,diagnostics:{last_attempt:{status:'error',error:'Provider refused this attempt',usage}}};
eq(attemptNotice(failed), {label:'Last attempt failed: Price unknown · $0.000000 known subtotal',error:'Provider refused this attempt'});
eq(priceText(failed.cost_usd, failed.pricing),'$0.500000 · all recorded requests priced');
eq(attemptNotice({...failed,error:'Provider refused this attempt'}).error,'');
eq(attemptNotice({...failed,error:'Prior unrelated failure'}).error,'Provider refused this attempt');
eq(attemptNotice({...old,diagnostics:{last_attempt:{status:'done',error:null,usage}}}),null);
eq(attemptNotice(old),null);
eq(attemptNotice({cost_usd:null,error:null,diagnostics:{last_attempt:{status:'done',error:null,usage}}}),{label:'Last attempt: Price unknown · $0.000000 known subtotal',error:''});
eq(attemptNotice({...old,diagnostics:{last_attempt:{status:'error',error:'<script>literal message</script>',usage:{requests:1,by_model:{}}}}}),{label:'Last attempt failed',error:'<script>literal message</script>'});
console.log(JSON.stringify({checks,status:'passed'}));
