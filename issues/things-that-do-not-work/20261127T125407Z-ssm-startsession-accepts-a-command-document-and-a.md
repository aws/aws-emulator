Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.start-session-command-document-rejected ssm.start-session-undeclared-parameter-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.start-session-command-document-rejected|_refused#9|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: ssm.start-session-undeclared-parameter-rejected|_refusal#6|AssertionError|raise AssertionError(f"expected StartSession to be refused, but it answered {result!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ssm: StartSession accepts a Command document and a parameter the session document does not declare

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSession

## Description
Cause 1 of 2: StartSession accepts a document whose DocumentType is Command.
AWS: StartSession's DocumentName must name a Session document; a Command document is refused (HTTP 400) and no session is created.
Emulator: returns 200 with a full session — SessionId, TokenValue and a StreamUrl `wss://<host>/v1/data-channel/<session id>?stream=input`.
Why: a client gets a session (and a token/stream URL) for a document AWS refuses, so the refusal path cannot be exercised and the session leaks.
Fix: the DocumentType check in the StartSession handler (services/service-com-amazonaws-ssm-amazonssm/src/ops/start_session.rs).
Traces: ssm.start-session-command-document-rejected.

Cause 2 of 2: StartSession accepts a Parameters entry the session document does not declare.
AWS: parameters are validated against the session document's declared parameters; an undeclared key is refused (HTTP 400) and no session starts.
Emulator: returns 200 with SessionId/TokenValue/StreamUrl.
Why: a typo'd or unsupported session parameter is silently ignored instead of refused, so the session runs with settings AWS would not accept.
Fix: the parameter validation in the same handler, against the resolved session document.
Traces: ssm.start-session-undeclared-parameter-rejected.

## Evidence
Cause 1: "the call was expected to be refused but returned {'SessionId': 'emulator-2f604388-…', 'TokenValue': 'tF+obCagC6…', 'StreamUrl': 'wss://127.0.0.1:37835/v1/data-channel/emulator-2f604388-…?stream=input'}" (HTTP 200).
Cause 2: "expected StartSession to be refused, but it answered {'SessionId': 'emulator-a8171ff9-…', 'TokenValue': '8KJ3iH4SREF/…', 'StreamUrl': 'wss://127.0.0.1:43171/v1/data-channel/…'}" (HTTP 200).

## Steps to Reproduce
1. CreateDocument with DocumentType Command, then StartSession(DocumentName=<that document>, Target=<managed node>) — AWS refuses, the emulator returns a session.
2. CreateDocument with DocumentType Session declaring no parameters, then StartSession with Parameters={'undeclared': ['x']} — AWS refuses, the emulator returns a session.
