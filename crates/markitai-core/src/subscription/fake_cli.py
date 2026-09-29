#!/usr/bin/env python3
"""Local protocol fixture. It never contacts GitHub or reads a real auth store."""
import base64
import json
import os
import pathlib
import subprocess
import sys
import time

root = pathlib.Path(os.environ['COPILOT_HOME'])
config = json.loads((root / 'fixture.json').read_text())
mode = config['mode']
if 'cache_home' in config:
    assert os.environ.get('COPILOT_CACHE_HOME') == config['cache_home']
    cache = pathlib.Path(config['cache_home'])
    cache.mkdir(mode=0o700, exist_ok=True)
    (cache / 'fixture-cache-access').write_text('private cache preserved')
log = root / 'requests.jsonl'
assert os.environ.get('HOME') == config['home']
assert os.getcwd() != config['home']
assert pathlib.Path.cwd().stat().st_mode & 0o777 == 0o700
(root / 'cwd-mode').write_text(oct(pathlib.Path.cwd().stat().st_mode & 0o777))
assert '--headless' in sys.argv and '--stdio' in sys.argv
assert '--no-auto-update' in sys.argv
assert 'OPENAI_API_KEY' not in os.environ
assert 'COPILOT_PROVIDER_API_KEY' not in os.environ
(root / 'pid').write_text(str(os.getpid()))
(root / 'cwd').write_text(os.getcwd())
sys.stderr.write('credential-must-never-appear-in-error\n' * 256)
sys.stderr.flush()

def send(value):
    data = json.dumps(value, ensure_ascii=False).encode()
    packet = ('Content-Length: %d\r\n\r\n' % len(data)).encode() + data
    if mode == 'split':
        for offset in range(0,len(packet),3):
            sys.stdout.buffer.write(packet[offset:offset+3]);sys.stdout.buffer.flush()
    else:
        sys.stdout.buffer.write(packet);sys.stdout.buffer.flush()

def response(req,value): send({'jsonrpc':'2.0','id':req['id'],'result':value})
def event(kind,data,eventid=None,session='session-fixture',agent=None):
    item={'id':eventid or kind,'type':kind,'data':data,'parentId':None,'timestamp':'2026-09-29T00:00:00Z'}
    if agent is not None:item['agentId']=agent
    send({'jsonrpc':'2.0','method':'session.event','params':{'sessionId':session,'event':item}})

def usage(tokens=7, eid='u1',api='api-1',agent=None):
    payload={'apiCallId':api,'model':'wrong-model' if mode=='wrong_model' else 'fixture','inputTokens':tokens,'outputTokens':3,'cost':12.5,'finishReason':'length' if mode=='length' else 'stop'}
    if mode=='byok':payload['isByok']=True
    event('assistant.usage',payload,eid,agent=agent)

while True:
    headers={}
    while True:
        line=sys.stdin.buffer.readline()
        if not line:sys.exit(0)
        if line==b'\r\n':break
        k,v=line.decode().split(':',1);headers[k.lower()]=v.strip()
    body=sys.stdin.buffer.read(int(headers['content-length']))
    req=json.loads(body)
    with log.open('a') as out:out.write(json.dumps(req)+'\n')
    method=req.get('method')
    if method=='connect':
        p=req.get('params',{});client=p.get('clientInfo',{})
        if set(client)-{'editorName','editorVersion','extensionName','extensionVersion'}:
            send({'jsonrpc':'2.0','id':req['id'],'error':{'code':-32602,'message':'Invalid connect request: unknown clientInfo field'}})
            continue
        assert client.get('editorName')=='markitai' and isinstance(client.get('editorVersion'),str) and client['editorVersion']
        assert p.get('supportedTaskKinds')==[]
        response(req,{'protocolVersion':2 if mode=='wrong_protocol' else 3})
    elif method=='status.get':response(req,{'version':'1.0.90-2','protocolVersion':3})
    elif method=='auth.getStatus':response(req,{'isAuthenticated':mode!='unauth','login':'fixture-user','authType':'env','host':'https://github.com'})
    elif method=='models.list':response(req,{'models':[{'id':'z','name':'Zed','policy':{'state':'enabled'},'capabilities':{'supports':{'vision':True}}},{'id':'hidden','policy':{'state':'disabled'}},{'id':'a','name':'Alpha'}]})
    elif method=='session.create':
        p=req['params']
        assert p['availableTools']==[] and p['tools']==[] and p['mcpServers']=={}
        for key in ('enableConfigDiscovery','enableFileHooks','enableHostGitOperations','enableSessionStore','enableSkills'):
            assert p[key] is False
        assert p['systemMessage']=={'mode':'replace','content':'fixed system'}
        assert pathlib.Path(p['workingDirectory']).resolve()==pathlib.Path.cwd()
        event('session.start',{'context':{'cwd':os.getcwd()}})
        response(req,{'sessionId':'session-fixture'})
    elif method=='session.send':
        p=req['params'];assert p['sessionId']=='session-fixture'
        for i,image in enumerate(p['attachments']):
            assert image['type']=='blob' and image['mimeType']=='image/png'
            assert base64.b64decode(image['data'])==b'\x89PNG\r\n\x1a\nfixture'
        if mode=='hang':
            child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'])
            (root/'descendant').write_text(str(child.pid))
            time.sleep(60)
        if mode=='oversized':
            sys.stdout.buffer.write(b'Content-Length: 99999999999\r\n\r\n');sys.stdout.buffer.flush();time.sleep(60)
        if mode=='callback':
            send({'jsonrpc':'2.0','id':'permission-id','method':'permission.request','params':{'secret':'credential-must-never-appear-in-error'}})
            time.sleep(60)
        if mode=='permission':
            event('permission.requested',{'requestId':'blocked','permissionRequest':{'kind':'shell'}});time.sleep(60)
        if mode!='no_usage':usage(agent='unexpected-agent' if mode=='subagent' else None)
        if mode=='duplicate':usage();usage(eid='u2')
        if mode=='conflict':usage(tokens=999,eid='u2')
        if mode=='paid_error':
            event('session.error',{'errorType':'authentication','message':'credential-must-never-appear-in-error'})
            time.sleep(60)
        if mode=='eof':sys.exit(0)
        event('assistant.message',{'messageId':'unrelated','content':'foreign'},session='other-session')
        text=p['prompt']
        if mode=='split':
            midpoint=len(text)//2
            for i in [1,0]:
                event('assistant.message',{'messageId':'part'+str(i),'apiCallId':'api-1','chunkIndex':i,'chunkCount':2,'content':text[:midpoint] if i==0 else text[midpoint:],'originatingMessageId':'message-1'},'part'+str(i))
        elif mode!='empty':event('assistant.message',{'messageId':'answer','content':text,'originatingMessageId':'message-1'})
        event('session.idle',{'aborted':mode=='aborted'})
        response(req,{'messageId':'message-1'})
    else:
        send({'jsonrpc':'2.0','id':req.get('id'),'error':{'code':-32601,'message':'unknown fixture operation'}})
