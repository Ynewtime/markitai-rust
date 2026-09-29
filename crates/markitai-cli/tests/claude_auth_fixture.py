#!/usr/bin/env python3
"""Private CLI status/login substitute; no credentials or service access."""
import json, os, pathlib, sys
root=pathlib.Path(os.environ['CLAUDE_CONFIG_DIR'])
state=json.loads((root/'fixture.json').read_text())
assert os.environ.get('HOME')==state['home']
assert not any(key in os.environ for key in ['OPENAI_API_KEY','ANTHROPIC_API_KEY','CLAUDE_CODE_OAUTH_TOKEN','COPILOT_GITHUB_TOKEN'])
args=sys.argv[1:]
with (root/'claude-calls.jsonl').open('a') as f:f.write(json.dumps({'args':args,'pid':os.getpid()})+'\n')
if args==['--version']:
 print('2.1.284 (Claude Code)')
elif args==['auth','status']:
 logged=state['mode']!='signed-out'
 print(json.dumps({'loggedIn':logged,'authMethod':'apiKey' if state['mode']=='byok' else 'claude.ai','apiProvider':'firstParty','email':'fixture@example.invalid','subscriptionType':'pro','token':'fixture-secret-never-exposed'}))
 sys.exit(0 if logged else 1)
elif args==['auth','login']:
 (root/'claude-login.json').write_text(json.dumps({'pid':os.getpid(),'home':os.environ.get('HOME')}))
 sys.exit(state['login_exit'])
else:sys.exit(90)
