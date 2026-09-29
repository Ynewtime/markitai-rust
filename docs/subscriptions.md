# Subscription runtimes

R31 source verification passes the workspace gate and authored process fixtures;
optimized CLI and installed-package delivery are being verified. This page separates the native adapter contract from verification
with a real subscription. Normal document conversion remains a native binary.
Subscription features require a separately installed official provider runtime.

## Copilot

The adapter targets the official GitHub Copilot CLI `1.0.90-2`, protocol 3, pinned
against [SDK source f5d9685](https://github.com/github/copilot-sdk/tree/f5d9685f55286061e12763ed15a73c4469609881).
It uses the Node SDK's actual `--headless --stdio` startup and Content-Length JSON
RPC framing. An incompatible runtime fails explicitly before a model request.

```text
markitai auth
markitai auth copilot status --json
markitai auth copilot login
```

Status asks the official runtime for authentication, without creating a model
session. Its public fields are provider/authenticated/user/expires_at/error/details.
JSON status exits zero even when unauthenticated, matching the reference command;
human status returns nonzero. Login replaces the Unix CLI process with the official
`copilot login`, retaining terminal ownership, signals and the exact exit status.
The official runtime owns login, refresh and storage. Markitai reads no token file,
prints no credential and does not rewrite the configured model list.

Configure the existing `llm.model_list[].litellm_params.model` as `copilot/MODEL`.
`COPILOT_CLI_PATH` selects an executable; `COPILOT_HOME` selects its official auth
home when provided. Explicit token precedence is COPILOT_GITHUB_TOKEN, GH_TOKEN,
GITHUB_TOKEN. Otherwise the official runtime resolves existing authentication.
No HTTP api_key/api_base or strict max_tokens setting is accepted for this route;
the pinned subscription protocol cannot enforce those settings. Missing login is
an error during conversion; it never initiates an interactive login command.

Text and ordered in-memory image attachments use the shared native LLM pipeline:
concurrency permits, document request admission, cancellation, model groups and
semantic validation. Structured output uses JSON text. Session tools, skills,
plugins, MCP, hooks, instruction discovery and persistent session storage are
disabled in the requested configuration. Unexpected permission/tool callbacks
fail. These controls are protocol settings, not an OS sandbox for an arbitrary
replacement executable.

Successful output requires an authoritative terminal idle event. Partial text at
EOF, terminal failure, refusal or truncation cannot become a successful document.
Every observed API call is counted once, including observations before a failure;
duplicate usage events do not multiply the count. Unreported usage stays unknown,
and subscription multipliers are never treated as dollars. A positive dollar
budget refuses before starting inference because no verified subscription dollar
tariff exists. The official runtime may internally make multiple API calls per
one admitted native request; its internal calls cannot be capped individually.

Pools containing Copilot bypass persistent response caching and cross-call request
merging, including when an explicit token exists. An official stored account may
change without changing its home path; a path hash is not account identity. Model
discovery likewise asks the runtime each time. HTTP-only pools retain existing
cache behavior.

Headless process execution has bounded concurrency, frame/event/input sizes and
an overall deadline. Stderr is drained without forwarding; it has no cumulative
byte-limit claim. On Unix, process-group cleanup ends children before temporary
workspaces are removed. Windows execution and login remain unsupported until its
process-tree boundary is implemented and tested.

## Other subscriptions and evidence

The `auth claude status` and `auth chatgpt status` command shapes report explicit
unsupported status at this checkpoint; they do not claim authenticated readiness.
Their runtime adapters remain separate work. No private OAuth protocol or API-
equivalent subscription dollar estimate is imported from the Python reference.

The committed fixtures are authored executable processes using fake credentials,
private MARKITAI_HOME/COPILOT_HOME and inherited HOME. Eleven adapter groups, seven conversion groups and three CLI process tests pass,
also included in the full 1,369-execution source gate. Real official-runtime compatibility, a real login and actual
subscription inference are distinct evidence and have not been asserted here.
