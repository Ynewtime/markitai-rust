#!/usr/bin/env python3
"""Authored CLI contract fixture: no network or actual Codex configuration access."""
import json
import os
from pathlib import Path
import sys

root = Path(__file__).resolve().parent
state = json.loads((root / 'state.json').read_text())
assert os.environ.get('HOME') == state['home']
assert os.environ.get('CODEX_HOME') == state['codex_home']
assert not {'OPENAI_API_KEY', 'OPENAI_BASE_URL', 'CODEX_API_KEY', 'HTTP_PROXY', 'HTTPS_PROXY'} & set(os.environ)
args = sys.argv[1:]
with (root / 'calls.jsonl').open('a') as stream:
    stream.write(json.dumps({'args': args, 'pid': os.getpid(), 'home': os.environ.get('HOME'), 'codex_home': os.environ.get('CODEX_HOME')}) + '\n')
if args == ['--version']:
    print('codex-cli ' + ('0.158.0' if state['mode'] == 'wrong-version' else '0.159.0'))
elif args == ['login', 'status']:
    if state['mode'] == 'signed-out':
        print('Not logged in', file=sys.stderr)
        sys.exit(1)
    if state['mode'] == 'api-key':
        print('Logged in using an API key - sk-fixture-never-expose', file=sys.stderr)
    elif state['mode'] == 'malformed':
        print('sk-fixture-never-expose', file=sys.stderr)
        sys.exit(9)
    else:
        print('Logged in using ChatGPT', file=sys.stderr)
elif args == ['login']:
    sys.exit(state['login_exit'])
else:
    raise AssertionError('Status/login must not launch inference: ' + repr(args))
