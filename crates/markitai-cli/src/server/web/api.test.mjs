import test from 'node:test';
import assert from 'node:assert/strict';
import {bootstrapToken,authenticatedURL,serviceURL,artifactPath,api} from './api.js';

const origin='http://127.0.0.1:9876';
globalThis.location={origin};
const storage=()=>{const map=new Map();return{getItem:key=>map.get(key),setItem:(key,value)=>map.set(key,value),map};};
test('fragment token wins, both URL credentials disappear, and job selection survives',()=>{
  const saved=storage();let clean;
  bootstrapToken({href:origin+'/?token=query-secret&job=abc#token=fragment-secret'}, {replaceState:(_a,_b,url)=>clean=url},saved);
  assert.equal(clean,'/?job=abc');
  assert.equal(saved.map.get('markitai.service-token'),'fragment-secret');
  assert.equal(new URL(authenticatedURL('/api/jobs/abc/events')).searchParams.get('token'),'fragment-secret');
});
test('blocked storage still permits memory-only access and strips token first',()=>{
  let clean;Object.defineProperty(globalThis,'sessionStorage',{configurable:true,get(){throw new Error('blocked');}});
  bootstrapToken({href:origin+'/#token=memory-only'},{replaceState:(_a,_b,url)=>clean=url});
  assert.equal(clean,'/');assert.equal(new URL(authenticatedURL('/api/history')).searchParams.get('token'),'memory-only');
  delete globalThis.sessionStorage;
});
test('tokens cannot be added to foreign, credentialed or non-API URLs',()=>{
  for(const path of ['https://evil.test/api/jobs','//evil.test/api/jobs','javascript:alert(1)','http://user:pass@127.0.0.1:9876/api/jobs','/ui/app.js'])assert.throws(()=>authenticatedURL(path));
  assert.equal(serviceURL('/api/jobs?x=1',origin).origin,origin);
});
test('artifact references require exact manifest membership and resolve encoded filenames once',()=>{
  const allowed=new Set(['assets/a b%?.png','base.md','assets/same.png']);
  assert.equal(artifactPath('assets/a%20b%25%3F.png?download=1#view','base.md',allowed),'assets/a b%?.png');
  assert.equal(artifactPath('../assets/same.png','folder/note.md',allowed),'assets/same.png');
  for(const path of ['https://evil.test/assets/same.png','//evil.test/a','/assets/same.png','javascript:alert(1)','assets/%ZZ','assets/no.png','..\\assets\\same.png'])assert.equal(artifactPath(path,'base.md',allowed),null);
});
test('authenticated fetch rejects redirects and never dispatches an external request',async()=>{
  const previous=globalThis.fetch;const calls=[];globalThis.fetch=async(url,options)=>{calls.push({url,options});return new Response(JSON.stringify({ok:true}),{status:200,headers:{'content-type':'application/json'}});};
  try{assert.deepEqual(await api('/api/capabilities'),{ok:true});assert.equal(calls.length,1);assert.equal(calls[0].options.redirect,'error');assert.equal(calls[0].options.cache,'no-store');assert.equal(calls[0].options.headers.get('Authorization'),'Bearer memory-only');await assert.rejects(api('https://evil.test/api/jobs'));assert.equal(calls.length,1);}finally{globalThis.fetch=previous;}
});
