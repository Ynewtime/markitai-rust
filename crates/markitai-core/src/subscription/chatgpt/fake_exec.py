#!/usr/bin/env python3
"""Authored protocol fixture. No network, login or real configuration reads."""
import hashlib,json,os,stat,subprocess,sys,time
from pathlib import Path
base=Path(__file__).resolve().parent
case=json.loads((base/'scenario.json').read_text())
args=sys.argv[1:]
with (base/'calls.jsonl').open('a') as f:f.write(json.dumps({'args':args,'cwd':str(Path.cwd()),'home':os.environ.get('HOME'),'env_names':sorted(os.environ)})+'\n')
if args==['--version']:
 print('codex-cli '+('0.158.0' if case['name']=='version' else '0.159.0'));sys.exit(0)
if args==['login','status']:
 if case['name']=='auth-none':print('Not logged in',file=sys.stderr);sys.exit(1)
 if case['name']=='auth-api':print('Logged in using an API key - sk-fake-never-expose',file=sys.stderr);sys.exit(0)
 print('Logged in using ChatGPT',file=sys.stderr);sys.exit(0)
assert 'exec' in args
for flag in ['--ignore-user-config','--ignore-rules','--ephemeral','--skip-git-repo-check','--json']:assert flag in args
assert args[args.index('--sandbox')+1]=='read-only'
assert args[args.index('--model')+1]=='gpt-5.5'
assert not {'OPENAI_API_KEY','OPENAI_BASE_URL','CODEX_API_KEY','HTTP_PROXY','HTTPS_PROXY'}&set(os.environ)
settings={}
for i,arg in enumerate(args):
 if arg=='-c':
  k,v=args[i+1].split('=',1);settings[k]=json.loads(v)
assert 'forced_login_method' not in settings
assert settings['model_provider']=='openai'
assert settings['project_doc_max_bytes']==0
assert settings['web_search']=='disabled'
assert settings['features.shell_tool'] is False
catalog=json.loads(Path(settings['model_catalog_json']).read_text())
assert len(catalog['models'])==1 and catalog['models'][0]['slug']=='gpt-5.5'
assert catalog['models'][0]['apply_patch_tool_type'] is None
assert catalog['models'][0]['tool_mode'] is None
assert catalog['models'][0]['experimental_supported_tools']==[]
paths=[Path(args[i+1]) for i,arg in enumerate(args) if arg=='--image']
request={'system':Path(settings['model_instructions_file']).read_text(),'user':sys.stdin.read(),'image_hashes':[hashlib.sha256(p.read_bytes()).hexdigest() for p in paths], 'workspace_mode':stat.S_IMODE(Path.cwd().stat().st_mode),'file_modes':[stat.S_IMODE(p.stat().st_mode) for p in [Path(settings['model_instructions_file']),Path(settings['model_catalog_json']),*paths]],'workspace':str(Path.cwd())}
(base/'request.json').write_text(json.dumps(request))
with (base/'requests.jsonl').open('a') as f:f.write(json.dumps(request)+'\n')
def emit(value):print(json.dumps(value),flush=True)
emit({'type':'thread.started','thread_id':'authored-thread'})
emit({'type':'item.completed','item':{'id':'warning','type':'error','message':'`[features].codex_hooks` is deprecated. Use hooks instead.'}})
emit({'type':'turn.started'})
if case['name']=='stderr':sys.stderr.write('x'*(1024*1024+1));sys.stderr.flush()
if case['name']=='sleep':
 child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']);(base/'grandchild.pid').write_text(str(child.pid));time.sleep(60)
if case['name']=='tool':
 emit({'type':'item.started','item':{'id':'tool','type':'command_execution','command':'must never run'}});time.sleep(60)
if case['name']=='failed':
 emit({'type':'turn.failed','error':{'message':'sk-fake-never-expose'}});sys.exit(1)
emit({'type':'item.completed','item':{'id':'comment','type':'agent_message','text':'An intermediate commentary.'}})
result=case.get('text','Complete authored document.')
if case['name']=='echo':
 result=request['user']
 if 'MARKITAI_DOCUMENT_JSON_V1' in request['system'] or 'MARKITAI_VISION_JSON_V1' in request['system']:
  result=json.dumps({'cleaned_markdown':result,'frontmatter':{'description':'Authored Codex fixture.','tags':['fixture']}})
emit({'type':'item.completed','item':{'id':'answer','type':'agent_message','text':result}})
if case['name']=='truncated':sys.exit(0)
usage={'input_tokens':11,'cached_input_tokens':3,'cache_write_input_tokens':0,'output_tokens':5,'reasoning_output_tokens':2}
if case['name']=='zero':usage={k:0 for k in usage}
if case['name']=='bad-count':usage['input_tokens']=-1
emit({'type':'turn.completed','usage':usage})
if case['name']=='after-terminal':emit({'type':'turn.failed','error':{'message':'sk-fake-never-expose'}})
if case['name']=='nonzero':sys.exit(7)
