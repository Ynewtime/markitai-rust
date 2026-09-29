#!/usr/bin/env python3
"""Authored local Copilot protocol substitute; no account or network access."""
import base64, hashlib, json, os, pathlib, sys
root=pathlib.Path(os.environ['COPILOT_HOME'])
config=json.loads((root/'fixture.json').read_text())
assert pathlib.Path.cwd().stat().st_mode & 0o777 == 0o700
assert os.environ.get('HOME')==config['home']
assert os.environ.get('MARKITAI_COPILOT_TOKEN')=='fixture-token'
assert 'OPENAI_API_KEY' not in os.environ
for flag in ['--headless','--stdio','--no-auto-update','--no-auto-login']:assert flag in sys.argv
mode=config['mode']
def send(value):
 data=json.dumps(value,ensure_ascii=False).encode();sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(data)).encode()+data);sys.stdout.buffer.flush()
def reply(request,result):send({'jsonrpc':'2.0','id':request['id'],'result':result})
def event(kind,data,identifier=None):send({'jsonrpc':'2.0','method':'session.event','params':{'sessionId':'fixture-session','event':{'id':identifier or kind,'type':kind,'data':data}}})
system=''
while True:
 headers={}
 while True:
  line=sys.stdin.buffer.readline()
  if not line:sys.exit(0)
  if line==b'\r\n':break
  name,value=line.decode().split(':',1);headers[name.lower()]=value.strip()
 request=json.loads(sys.stdin.buffer.read(int(headers['content-length'])))
 with (root/'requests.jsonl').open('a') as log:log.write(json.dumps(request,ensure_ascii=False)+'\n')
 method=request['method'];p=request.get('params',{})
 if method=='connect':reply(request,{'protocolVersion':3})
 elif method=='status.get':reply(request,{'version':'1.0.90-2','protocolVersion':3})
 elif method=='auth.getStatus':reply(request,{'isAuthenticated':True,'login':'private-fixture','authType':'env'})
 elif method=='models.list':reply(request,{'models':[{'id':'fixture','name':'Fixture','capabilities':{'supports':{'vision':True}}}]})
 elif method=='session.create':
  assert p['availableTools']==[] and p['tools']==[] and p['mcpServers']=={}
  assert p['systemMessage']['mode']=='replace' and p['enableConfigDiscovery'] is False
  system=p['systemMessage']['content'];reply(request,{'sessionId':'fixture-session'})
 elif method=='session.send':
  for image in p['attachments']:
   assert image['type']=='blob' and image['mimeType']=='image/png'
   decoded=base64.b64decode(image['data']);assert hashlib.sha256(decoded).hexdigest()==config['image_sha256']
  if mode!='no_usage':event('assistant.usage',{'model':'fixture','apiCallId':'api-fixture','inputTokens':7,'outputTokens':3,'cost':987.5,'finishReason':'stop'})
  if mode=='paid_error':
   event('session.error',{'errorType':'authentication','message':'do-not-disclose-fixture-secret'});reply(request,{'messageId':'prompt-fixture'});continue
  text=p['prompt']
  if mode=='invalid':text='not JSON required by the actual caller'
  elif 'MARKITAI_DOCUMENT_JSON_V1' in system or 'MARKITAI_VISION_JSON_V1' in system:
   text=json.dumps({'cleaned_markdown':text,'frontmatter':{'description':'Authored subscription fixture.','tags':['fixture'] }},ensure_ascii=False)
  elif 'Reply with exactly OK.'==text:text='OK'
  event('assistant.message',{'messageId':'answer-fixture','originatingMessageId':'prompt-fixture','content':text})
  event('session.idle',{});reply(request,{'messageId':'prompt-fixture'})
 else:send({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32601,'message':'unexpected fixture request'}})
