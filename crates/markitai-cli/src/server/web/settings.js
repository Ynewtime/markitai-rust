import {api,element,button,errorText} from './api.js';
const $=id=>document.getElementById(id), base='/api/settings/llm';
let hooks, settings=null, providers=[], selected=null, draftRevision=null, discovered=[], edit=null, busy=false;
let loadGeneration=0;
function status(text,bad=false){$('settings-message').textContent=text;$('settings-message').classList.toggle('item-error',bad);}
function formBusy(value){busy=value;for(const node of $('connection-form').querySelectorAll('button'))node.disabled=value;}
async function guarded(action){if(busy)return;formBusy(true);try{await action();}catch(error){if(error.status===409){await conflict(error);}else{status(errorText(error),true);}}finally{formBusy(false);}}
async function conflict(error){
  const previous=draftRevision;await loadSettings();draftRevision=previous;
  $('conflict-message').hidden=false;
  $('conflict-message').replaceChildren(element('strong','', 'Settings changed. Your draft is still here.'),element('p','', 'Review the refreshed connection and model lists before using the new revision. Nothing was overwritten.'));
  $('conflict-message').append(button('Use current revision after review',()=>{draftRevision=settings.revision;$('conflict-message').hidden=true;status('Draft kept. Submit again when ready.');}));
  status(errorText(error),true);
}
function credentials(){const fields={};for(const [prefix,key]of[['key','api_key'],['base','api_base']]){const mode=$(prefix+'-mode').value;if(mode==='clear')fields[key]=null;if(mode==='replace'){const value=$('provider-'+prefix).value.trim();if(!value)throw new Error(`Enter ${key==='api_key'?'an API key':'a base URL'}, or select Clear.`);fields[key]=value;}}return fields;}
function currentModels(){const entries=[...$('discovered-models').querySelectorAll('input:checked')].map(node=>node.value);entries.push(...$('manual-models').value.split(/\r?\n/).map(line=>line.trim()).filter(Boolean));return [...new Set(entries)];}
function modelPayload(model){const value={model_name:$('routing-group').value.trim(),model,weight:Number($('model-weight').value),...credentials()};if(!value.model_name||!Number.isInteger(value.weight)||value.weight<0)throw new Error('Routing group and a nonnegative integer weight are required.');if(selected){if(selected.provider_id.startsWith('legacy:'))value.credential_deployment_id=selected.deployment_id||selected.provider_id.slice(7);else value.credential_provider_id=selected.provider_id;}else value.provider=$('provider-type').value;return value;}
function clearSecrets(){for(const key of ['key','base']){$('provider-'+key).value='';$(key+'-mode').value='keep';$('provider-'+key).disabled=true;}}
function reset(){selected=null;draftRevision=settings?.revision||null;discovered=[];clearSecrets();$('provider-type').disabled=false;$('provider-type').value='openai';$('connection-title').textContent='Add a connection';$('connection-hint').textContent='Discover available models or enter model identifiers manually.';$('manual-models').value='';$('discovered-models').replaceChildren();$('discovery-status').textContent='';$('routing-group').value='default';$('model-weight').value='1';$('save-connection').hidden=true;$('conflict-message').hidden=true;status('');}
async function editConnection(provider){
  await guarded(async()=>{
    const values=await api(`${base}/providers/${encodeURIComponent(provider.provider_id)}/credentials`);
    selected=provider;draftRevision=settings.revision;clearSecrets();
    $('provider-type').value=provider.provider;$('provider-type').disabled=true;
    $('provider-key').value=values.api_key||'';$('provider-base').value=values.api_base||'';
    $('provider-base').placeholder=values.api_base_placeholder||'https://api.example.com/v1';
    $('connection-title').textContent=provider.label||provider.provider;
    $('connection-hint').textContent='Existing credentials stay unchanged unless you choose Replace or Clear. You can also add models to this connection.';
    $('save-connection').hidden=false;$('conflict-message').hidden=true;$('manual-models').value='';$('discovered-models').replaceChildren();$('discovery-status').textContent='';status('Connection loaded for editing.');
  });
}
function render(){
  $('settings-context').textContent=settings?`${settings.config_origin} configuration · ${settings.config_path} · ${settings.routable?'Models available':'No routable model yet'}`:'';
  $('provider-list').replaceChildren();
  if(!providers.length)$('provider-list').append(element('p','empty compact','Add your first provider connection to enable model enhancement.'));
  for(const provider of providers){
    const row=element('article','connection-row');const info=element('div');info.append(element('strong','',provider.label||provider.provider),element('small','muted',`${provider.kind} · ${provider.status}${provider.model_count!==undefined?` · ${provider.model_count} models`:''}`));if(provider.api_base)info.append(element('small','mono muted',provider.api_base));
    const actions=element('div','row');
    if(provider.provider_id){actions.append(button('Edit',()=>editConnection(provider)),button('Delete',()=>guarded(async()=>{if(!await hooks.confirmDelete('Delete connection?',`Remove ${provider.label||provider.provider} and all linked models?`))return;await api(`${base}/providers/${encodeURIComponent(provider.provider_id)}?expected_revision=${encodeURIComponent(settings.revision)}`,{method:'DELETE'});if(selected?.provider_id===provider.provider_id)reset();await saved('Connection deleted.');}),'quiet small delete'));}
    else if(['openai','anthropic','gemini','deepseek','openrouter','azure','ollama','custom'].includes(provider.provider)){actions.append(button('Set up',()=>{reset();$('provider-type').value=provider.provider;}));}
    row.append(info,actions);$('provider-list').append(row);
  }
  $('deployment-list').replaceChildren();
  if(!settings?.deployments?.length&&!settings?.detected?.length)$('deployment-list').append(element('p','empty compact','No models selected yet. Discover or enter models on the right.'));
  for(const deployment of [...(settings?.deployments||[]),...(settings?.detected||[])]){
    const row=element('article','connection-row');const info=element('div');info.append(element('strong','filename',deployment.model),element('small','muted',`${deployment.routing_group} · weight ${deployment.weight} · ${deployment.persisted?'saved':'detected for this session'}`));const actions=element('div','row');
    if(deployment.persisted===false){actions.append(button('Use model',()=>{reset();$('provider-type').value=deployment.model.split('/')[0];$('manual-models').value=deployment.model;}),button('Test',()=>guarded(async()=>{status('Testing connection…');const response=await api(base+'/test',{method:'POST',body:{deployment_id:deployment.deployment_id}});status(response.detail,!response.ok);})));row.append(info,actions);$('deployment-list').append(row);continue;}
    actions.append(button('Test',()=>guarded(async()=>{status('Testing connection…');const result=await api(base+'/test',{method:'POST',body:{deployment_id:deployment.deployment_id}});status(result.detail,!result.ok);})),button('Edit',()=>{edit={...deployment,revision:settings.revision};$('edit-model').value=deployment.model;$('edit-group').value=deployment.routing_group;$('edit-weight').value=deployment.weight;$('edit-message').textContent='';$('deployment-dialog').showModal();}),button('Delete',()=>guarded(async()=>{if(!await hooks.confirmDelete('Delete model?',`Remove ${deployment.model}? Its connection remains saved.`))return;await api(`${base}/deployments/${encodeURIComponent(deployment.deployment_id)}?expected_revision=${encodeURIComponent(settings.revision)}`,{method:'DELETE'});await saved('Model deleted.');}),'quiet small delete'));
    row.append(info,actions);$('deployment-list').append(row);
  }
}
export async function loadSettings(){
  const expected=++loadGeneration;
  const [view,cardData]=await Promise.all([api(base),api(base+'/providers')]);
  if(expected!==loadGeneration)return;settings=view;providers=cardData.providers||[];if(draftRevision===null)draftRevision=view.revision;render();
}
async function saved(message){await loadSettings();draftRevision=settings.revision;await hooks.onSaved();status(message);$('conflict-message').hidden=true;}
export function initSettings(value){
  hooks=value;
  $('settings-refresh').addEventListener('click',()=>guarded(loadSettings));
  $('connection-new').addEventListener('click',reset);$('connection-reset').addEventListener('click',reset);
  for(const prefix of ['key','base'])$(prefix+'-mode').addEventListener('change',()=>{$('provider-'+prefix).disabled=$(prefix+'-mode').value!=='replace';});
  $('discover-models').addEventListener('click',()=>guarded(async()=>{
    const body={provider:$('provider-type').value,...credentials()};if(selected)body.provider_id=selected.provider_id;
    $('discovery-status').textContent='Discovering available models…';
    const result=await api(base+'/model-discovery',{method:'POST',body});discovered=result.models||[];
    $('discovery-status').textContent=`${result.status} · ${discovered.length} models${result.cached?' · cached':''}${result.stale?' · stale':''}${result.authoritative===false?' · verify model identifiers':''}${result.detail?' · '+result.detail:''}`;
    $('discovered-models').replaceChildren();
    if(discovered.length){$('discovered-models').append(button('Select all',()=>{for(const input of $('discovered-models').querySelectorAll('input'))input.checked=true;}),button('Clear',()=>{for(const input of $('discovered-models').querySelectorAll('input'))input.checked=false;}));}
    for(const model of discovered){const label=element('label','model-choice'),input=element('input');input.type='checkbox';input.value=model.model;label.append(input,element('span','',model.label||model.model));if(model.supports_vision)label.append(element('small','tag','VISION'));$('discovered-models').append(label);}
    if(!discovered.length)$('discovery-status').textContent+=' · You can still enter model identifiers below.';
  }));
  $('connection-form').addEventListener('submit',event=>{event.preventDefault();guarded(async()=>{
    if(!settings)throw new Error('Load settings before saving.');const models=currentModels();if(!models.length||models.length>50)throw new Error('Select or enter between 1 and 50 models.');
    const deployments=models.map(modelPayload);
    await api(base+'/deployments/batch',{method:'POST',body:{expected_revision:draftRevision,deployments}});
    reset();await saved(`${models.length} model(s) added.`);
  });});
  $('save-connection').addEventListener('click',()=>guarded(async()=>{
    if(!selected)return;const fields=credentials();if(!Object.keys(fields).length)throw new Error('Choose Replace or Clear for a credential field first.');
    await api(`${base}/providers/${encodeURIComponent(selected.provider_id)}`,{method:'PATCH',body:{expected_revision:draftRevision,...fields}});
    reset();await saved('Connection saved. Future jobs use the updated settings.');
  }));
  $('test-draft').addEventListener('click',()=>guarded(async()=>{
    const model=currentModels()[0];if(!model)throw new Error('Select or enter a model to test.');
    // Explicit edit loaded the saved reference, not its environment-expanded secret.
    const body={model,...credentials()};
    if(selected){if(!('api_key'in body)&&$('provider-key').value)body.api_key=$('provider-key').value;if(!('api_base'in body)&&$('provider-base').value)body.api_base=$('provider-base').value;}
    status('Testing model…');const response=await api(base+'/test',{method:'POST',body});status(response.detail,!response.ok);
  }));
  $('edit-cancel').addEventListener('click',()=>$('deployment-dialog').close());
  $('deployment-form').addEventListener('submit',async event=>{
    event.preventDefault();const weight=Number($('edit-weight').value);if(!Number.isInteger(weight)||weight<0)return;
    try{await api(`${base}/deployments/${encodeURIComponent(edit.deployment_id)}`,{method:'PATCH',body:{expected_revision:edit.revision,model:$('edit-model').value.trim(),model_name:$('edit-group').value.trim(),weight}});$('deployment-dialog').close();await saved('Model updated.');}
    catch(error){if(error.status===409){await loadSettings();$('edit-message').replaceChildren(element('p','', 'Settings changed; your edits remain here. Review the refreshed list before continuing.'),button('Use current revision after review',()=>{edit.revision=settings.revision;$('edit-message').textContent='Draft retained. Save again when ready.';}));}else $('edit-message').textContent=errorText(error);}
  });
}
