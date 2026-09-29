import {api, authenticatedURL, bootstrapToken, setToken, fileURL, element, button, errorText} from './api.js';
import {preview} from './preview.js';
import {initSettings, loadSettings} from './settings.js';
bootstrapToken();
const $ = id => document.getElementById(id);
let files = [], current = null, stream = null, poll = null, generation = 0, resultGeneration = 0;
let result = null, raw = '', documentPath = '', historyRows = [], page = 0;
let capabilities = null;
const notice = (message, bad = false) => { $('notice').textContent = message; $('notice').hidden = !message; $('notice').classList.toggle('error', bad); };
const run = async action => { try { return await action(); } catch (error) { notice(errorText(error), true); } };
const terminal = item => ['done','error'].includes(item.status);
// Request coverage is independent of a zero or rounded cost subtotal.
function attemptPricing(usage) {
  if (!usage) return null;
  let priced = 0, unpriced = 0;
  for (const row of Object.values(usage.by_model || {})) {
    const requests = Number.isSafeInteger(row.requests) && row.requests > 0 ? row.requests : 0;
    if (!requests) continue;
    const known = row.priced_requests, missing = row.unpriced_requests;
    const expected = known === 0 ? 'unknown' : missing > 0 ? 'partial' : 'complete';
    if (Number.isSafeInteger(known) && known >= 0 && Number.isSafeInteger(missing) && missing >= 0 && known + missing === requests && row.cost_status === expected) {
      priced += known; unpriced += missing;
    } else unpriced += requests;
  }
  if (Number.isSafeInteger(usage.requests)) unpriced += Math.max(0, usage.requests - priced - unpriced);
  return priced + unpriced > 0 ? {priced_requests:priced, unpriced_requests:unpriced, cost_status:priced === 0 ? 'unknown' : unpriced > 0 ? 'partial' : 'complete'} : null;
}
function priceText(cost, pricing) {
  if (typeof cost !== 'number' || !Number.isFinite(cost) || cost < 0) return '';
  const amount = `$${cost.toFixed(6)}`;
  if (pricing?.cost_status === 'complete') return `${amount} · all recorded requests priced`;
  if (pricing?.cost_status === 'partial') return `${amount} known subtotal · ${pricing.unpriced_requests} unpriced request(s)`;
  if (pricing?.cost_status === 'unknown') return `Price unknown · ${amount} known subtotal`;
  return cost > 0 ? `${amount} recorded subtotal · pricing completeness unavailable` : '';
}


function attemptNotice(item) {
  const attempt = item.diagnostics?.last_attempt;
  if (!attempt || (attempt.status !== 'error' && item.cost_usd != null)) return null;
  const cost = priceText(attempt.usage?.cost_usd, attemptPricing(attempt.usage));
  const failed = attempt.status === 'error';
  const error = failed && typeof attempt.error === 'string' && attempt.error !== item.error ? attempt.error : '';
  return {label: failed ? `Last attempt failed${cost ? `: ${cost}` : ''}` : cost ? `Last attempt: ${cost}` : '', error};
}

export async function confirmDelete(title, text) {
  const dialog = $('confirm-dialog'); $('confirm-title').textContent = title; $('confirm-message').textContent = text;
  return new Promise(resolve => { dialog.addEventListener('close', () => resolve(dialog.returnValue === 'confirm'), {once:true}); dialog.showModal(); });
}
function view(name) {
  for (const section of document.querySelectorAll('.view')) section.hidden = section.id !== `${name}-view`;
  for (const link of document.querySelectorAll('[data-view]')) link.classList.toggle('active', link.dataset.view === name);
  if (name === 'history') run(loadHistory);
  if (name === 'settings') run(loadSettings);
}
for (const nav of document.querySelectorAll('[data-view]')) nav.addEventListener('click', () => view(nav.dataset.view));
for (const [key, label] of Object.entries({llm:'LLM enhancement',ocr:'Local OCR',alt:'Image alt text',desc:'Image descriptions',screenshot:'Screenshots',screenshot_only:'Screenshots only',pure:'Pure mode',no_cache:'Bypass cache',no_compress:'Keep image quality'})) {
  const field = element('label','field',label), select = element('select'); select.dataset.option = key;
  for (const [value, text] of [['','Default'],['true','On'],['false','Off']]) { const option = element('option','',text); option.value = value; select.append(option); }
  field.append(select); $('boolean-options').append(field);
}
function options() {
  const value = {};
  for (const key of ['preset','profile','strategy','backend']) if ($(key).value) value[key] = $(key).value;
  for (const select of document.querySelectorAll('[data-option]')) if (select.value) value[select.dataset.option] = select.value === 'true';
  return value;
}
function addFiles(values) {
  files.push(...values); showFiles();
}
function showFiles() {
  $('file-list').replaceChildren();
  for (const [index, file] of files.entries()) {
    const row = element('li'); row.append(element('span','filename',file.name),element('small','muted',size(file.size)),button('×',() => { files.splice(index,1); showFiles(); }));
    row.lastChild.setAttribute('aria-label',`Remove ${file.name}`); $('file-list').append(row);
  }
}
function size(bytes) { return bytes > 1024*1024 ? `${(bytes/1024/1024).toFixed(1)} MiB` : `${Math.ceil(bytes/1024)} KiB`; }
$('files').addEventListener('change',event => { addFiles(event.target.files); event.target.value = ''; });
for (const type of ['dragover','dragenter']) $('drop-zone').addEventListener(type,event => { event.preventDefault(); $('drop-zone').classList.add('drag'); });
for (const type of ['dragleave','drop']) $('drop-zone').addEventListener(type,event => { event.preventDefault(); $('drop-zone').classList.remove('drag'); });
$('drop-zone').addEventListener('drop',event => addFiles(event.dataTransfer.files));
$('convert-form').addEventListener('submit',async event => {
  event.preventDefault(); await run(async () => {
    const urls = $('urls').value.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
    if (!files.length && !urls.length) throw new Error('Choose at least one file or enter a URL.');
    if (files.length + urls.length > (capabilities?.limits?.max_job_items || 1000)) throw new Error('Too many items in one job.');
    if (files.some(file => file.size > 100*1024*1024)) throw new Error('Each file must be 100 MiB or smaller.');
    for (const value of urls) { const parsed = new URL(value); if (!['http:','https:'].includes(parsed.protocol)) throw new Error('URLs must use HTTP or HTTPS.'); }
    const body = new FormData(); for (const file of files) body.append('files',file,file.name);
    body.append('urls',JSON.stringify(urls)); body.append('options',JSON.stringify(options()));
    $('submit-job').disabled = true; $('submit-job').textContent = 'Uploading & creating job…'; notice('');
    try { const created = await api('/api/jobs',{method:'POST',body}); await openJob(created.job_id); files = []; showFiles(); $('urls').value = ''; }
    finally { $('submit-job').disabled = false; $('submit-job').textContent = 'Convert to Markdown ↗'; }
  });
});
function closeStream() { stream?.close(); stream = null; clearTimeout(poll); poll = null; }
async function openJob(id) {
  generation++; closeStream(); resultGeneration++; result = null; $('result-panel').hidden = true;
  const expected = generation;
  const snapshot = await api(`/api/jobs/${encodeURIComponent(id)}`);
  if (expected !== generation) return;
  current = snapshot;
  const url = new URL(location.href); url.searchParams.set('job',id); history.replaceState(null,'',url.pathname+url.search);
  view('convert'); renderJob(); if (current.status === 'running') subscribe(expected);
}
function renderJob() {
  if (!current) return;
  $('job-empty').hidden = true; $('job-progress').hidden = false;
  const completed = current.items.filter(terminal).length;
  $('progress').max = current.total || current.items.length || 1; $('progress').value = completed;
  $('job-progress-text').textContent = `${completed} of ${current.items.length} finished`;
  $('job-id').textContent = current.job_id;
  $('job-message').textContent = current.persistence_error || (current.status === 'running' ? 'Converting on your server…' : current.failed ? `${current.failed} item(s) need attention.` : 'Everything is ready.');
  $('job-archive').hidden = current.status === 'running';
  $('job-archive').href = authenticatedURL(`/api/jobs/${encodeURIComponent(current.job_id)}/archive`);
  $('job-items').replaceChildren();
  for (const item of current.items) {
    const row = element('article','job-item');
    const icon = element('span',`item-icon ${item.status}`,item.status === 'done' ? '✓' : item.status === 'error' ? '!' : item.status === 'running' ? '↻' : '·');
    const info = element('div','item-info'); info.append(element('strong','filename',item.name),element('small','muted',`${item.kind} · ${item.skip_reason === 'pending_batch' ? 'provider batch pending' : item.skipped ? 'skipped' : item.status}${item.duration_ms !== null ? ` · ${(item.duration_ms/1000).toFixed(1)}s` : ''}${item.llm_enhanced ? ' · enhanced' : ''}`));
    const outputCost = priceText(item.cost_usd, item.pricing);
    if (outputCost) info.append(element('small','muted',`Output cost: ${outputCost}`));
    const attempt = attemptNotice(item);
    if (attempt?.label) info.append(element('small','muted',attempt.label));
    if (attempt?.error) info.append(element('p','item-error',attempt.error));
    if (item.error) info.append(element('p','item-error',item.error));
    if (item.warnings?.length) { const details = element('details','warnings'); details.append(element('summary','',`${item.warnings.length} notice(s)`)); for (const warning of item.warnings) details.append(element('p','',warning)); info.append(details); }
    const actions = element('div','item-actions');
    if (item.status === 'done' && item.output) actions.append(button('View',() => run(() => openResult(item)), 'quiet small strong'));
    if (terminal(item) && item.retryable && item.skip_reason !== 'pending_batch') {
      actions.append(button('Retry',() => run(() => retry(item,false))));
      const enhance = button('Enhance',() => run(() => retry(item,true))); enhance.disabled = !capabilities?.llm?.routable; enhance.title = enhance.disabled ? 'Configure a working model connection first' : 'Reconvert with LLM enhancement'; actions.append(enhance);
    }
    if (current.status !== 'running') actions.append(button('Delete',() => run(async () => {
      if (!await confirmDelete('Delete this item?',`“${item.name}” and its owned files will be removed. Shared assets remain available to other items.`)) return;
      const id = current.job_id; await api(`/api/jobs/${encodeURIComponent(id)}/items/${encodeURIComponent(item.item_id)}`,{method:'DELETE'});
      if (current.items.length === 1) { current = null; closeStream(); $('job-items').replaceChildren(); $('job-empty').hidden = false; $('job-progress').hidden = true; $('job-archive').hidden = true; $('result-panel').hidden = true; const url=new URL(location.href);url.searchParams.delete('job');history.replaceState(null,'',url.pathname+url.search); }
      else await openJob(id);
    }), 'quiet small delete'));
    row.append(icon,info,actions); $('job-items').append(row);
  }
}
function subscribe(expected) {
  const id = current.job_id;
  stream = new EventSource(authenticatedURL(`/api/jobs/${encodeURIComponent(id)}/events`));
  for (const kind of ['snapshot','item','job']) stream.addEventListener(kind,event => {
    if (expected !== generation) return;
    try {
      const value = JSON.parse(event.data);
      if (kind === 'snapshot') current = value;
      else if (kind === 'item') { const index = current.items.findIndex(item => item.item_id === value.item_id); if (index >= 0) current.items[index] = value; }
      else Object.assign(current,value);
      renderJob();
      if (current.status !== 'running') { closeStream(); run(async () => { const value=await api(`/api/jobs/${encodeURIComponent(id)}`); if(expected===generation){current=value;renderJob();} }); }
    } catch { notice('Could not read a progress update. Refreshing the job.',true); }
  });
  stream.onerror = () => {
    if (expected !== generation) return;
    closeStream(); $('job-message').textContent = 'Progress connection interrupted. Reconnecting…';
    poll = setTimeout(() => run(async () => {
      const value = await api(`/api/jobs/${encodeURIComponent(id)}`);
      if (expected !== generation) return; current = value; renderJob(); if (current.status === 'running') subscribe(expected);
    }),2000);
  };
}
async function retry(item, enhance) {
  const id = current.job_id;
  const body = enhance ? {operation:'enhance',options:{...current.options,llm:true}} : {operation:'retry'};
  // History options can contain internal origin metadata; only public option keys travel back.
  if (enhance) for (const key of Object.keys(body.options)) if (!['preset','llm','ocr','profile','alt','desc','screenshot','screenshot_only','pure','no_cache','no_compress','strategy','backend'].includes(key)) delete body.options[key];
  await api(`/api/jobs/${encodeURIComponent(id)}/items/${encodeURIComponent(item.item_id)}/retry`,{method:'POST',body});
  await openJob(id);
}
async function openResult(item) {
  const expected = ++resultGeneration, job = current.job_id;
  const value = await api(`/api/jobs/${encodeURIComponent(job)}/items/${encodeURIComponent(item.item_id)}/result`);
  if (expected !== resultGeneration || job !== current?.job_id) return;
  result = {...value,job}; $('result-title').textContent = item.name; $('result-panel').hidden = false;
  const variants = value.artifacts.filter(asset => asset.relpath.endsWith('.md'));
  const preferred = variants.find(asset => value.variant === 'llm' ? asset.relpath.endsWith('.llm.md') : !asset.relpath.endsWith('.llm.md')) || variants[0];
  documentPath = preferred?.relpath || item.output;
  $('result-variant').replaceChildren();
  for (const asset of variants) { const option = element('option','',asset.relpath.endsWith('.llm.md') ? 'Enhanced' : 'Base'); option.value = asset.relpath; $('result-variant').append(option); }
  $('result-variant').value = documentPath;
  $('artifact-list').replaceChildren();
  for (const asset of value.artifacts) { const li = element('li'); const link = element('a','',asset.relpath); link.href = authenticatedURL(fileURL(job,asset.relpath)); link.download = ''; li.append(link,element('small','muted',size(asset.size))); $('artifact-list').append(li); }
  renderResult(value.markdown); $('result-panel').scrollIntoView({behavior:'smooth',block:'start'});
}
function renderResult(markdown) { raw = markdown; $('source').textContent = raw; $('download-source').href = authenticatedURL(fileURL(result.job,documentPath)); $('download-source').download = ''; preview(raw,result.job,documentPath,result.artifacts,$('rendered')); }
$('result-variant').addEventListener('change',() => run(async () => { const expected=++resultGeneration, path=$('result-variant').value, selected=result; const text=await api(fileURL(selected.job,path),{text:true}); if(expected===resultGeneration&&result===selected){documentPath=path;renderResult(text);} }));
for (const mode of ['preview','source']) $(`mode-${mode}`).addEventListener('click',() => { $('rendered').hidden=mode!=='preview';$('source').hidden=mode!=='source';$('mode-preview').classList.toggle('active',mode==='preview');$('mode-source').classList.toggle('active',mode==='source'); });
$('copy-source').addEventListener('click',() => run(async () => { await navigator.clipboard.writeText(raw); notice('Markdown copied.'); }));
async function loadHistory() { historyRows=await api('/api/history');page=0;renderHistory();$('history-archive').href=authenticatedURL('/api/history/archive'); }
function renderHistory() {
  const search=$('history-search').value.toLowerCase(); const rows=historyRows.filter(row=>(row.names_preview||[]).join(' ').toLowerCase().includes(search));
  const pages=Math.max(1,Math.ceil(rows.length/12));page=Math.min(page,pages-1);
  $('history-count').textContent=`${rows.length} saved jobs`; $('history-page').textContent=`${page+1} / ${pages}`; $('history-prev').disabled=page===0;$('history-next').disabled=page>=pages-1;
  $('history-list').replaceChildren();
  if(!rows.length)$('history-list').append(element('p','empty','No saved jobs yet.'));
  for(const row of rows.slice(page*12,page*12+12)){
    const item=element('article','history-item');const info=element('div');info.append(element('strong','filename',row.names_preview.join(', ')||row.job_id),element('small','muted',`${new Date(row.created_at).toLocaleString()} · ${row.total} items · ${row.done} done${row.failed?` · ${row.failed} failed`:''}`));
    const historyCost = priceText(row.cost_usd, row.pricing); if (historyCost) info.append(element('small','muted',historyCost));
    const actions=element('div','row');actions.append(button('Open',()=>run(()=>openJob(row.job_id))));const zip=element('a','quiet small','ZIP ↓');zip.href=authenticatedURL(`/api/jobs/${encodeURIComponent(row.job_id)}/archive`);actions.append(zip,button('Delete',()=>run(async()=>{if(await confirmDelete('Delete saved job?',`Remove ${row.names_preview.join(', ')} and all its saved files?`)){await api(`/api/history/${encodeURIComponent(row.job_id)}`,{method:'DELETE'});await loadHistory();}}),'quiet small delete'));item.append(info,actions);$('history-list').append(item);
  }
}
$('history-search').addEventListener('input',()=>{page=0;renderHistory();});$('history-prev').addEventListener('click',()=>{page--;renderHistory();});$('history-next').addEventListener('click',()=>{page++;renderHistory();});$('history-refresh').addEventListener('click',()=>run(loadHistory));
async function refreshCapabilities(){capabilities=await api('/api/capabilities');$('version').textContent=`v${capabilities.version}`;$('service-status').textContent='● Connected';const selected=$('preset').value;$('preset').replaceChildren();for(const name of ['',...capabilities.presets]){const option=element('option','',name?name[0].toUpperCase()+name.slice(1):'Server defaults');option.value=name;$('preset').append(option);}$('preset').value=selected;renderJob();}
initSettings({notice,confirmDelete,onSaved:refreshCapabilities});
$('auth-open').addEventListener('click',()=>$('auth-dialog').showModal());$('auth-cancel').addEventListener('click',()=>$('auth-dialog').close());
window.addEventListener('markitai:unauthorized',()=>{notice('This service requires an access token.',true);if(!$('auth-dialog').open)$('auth-dialog').showModal();});
$('auth-form').addEventListener('submit',event=>{event.preventDefault();setToken($('access-token').value);$('access-token').value='';$('auth-dialog').close();run(async()=>{await refreshCapabilities();if(current)await openJob(current.job_id);else{const id=new URL(location.href).searchParams.get('job');if(id)await openJob(id);}notice('Connected.');});});
window.addEventListener('beforeunload',closeStream);
run(async()=>{await refreshCapabilities();const id=new URL(location.href).searchParams.get('job');if(id)await openJob(id);});
