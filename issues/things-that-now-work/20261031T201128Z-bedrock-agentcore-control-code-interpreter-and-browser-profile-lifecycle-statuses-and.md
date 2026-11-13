Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-code-interpreter-duplicate-and-errors
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-code-interpreter-duplicate-and-errors bedrock-agentcore-control-code-interpreter-lifecycle bedrock-agentcore-control-browser-profile-duplicate-and-notfound
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateCodeInterpreter|2xx->2xx|$.status
Issue-Key: bedrock-agentcore-control|GetCodeInterpreter|2xx->2xx|$.status
Issue-Key: bedrock-agentcore-control|DeleteCodeInterpreter|2xx->2xx|$.status
Issue-Key: bedrock-agentcore-control|CreateCodeInterpreter|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateBrowserProfile|4xx->4xx|$.message
Issue-Commit: c58e269113894f48dd3e032b4127d0ccc7e88163

# Code interpreter and browser profile lifecycle statuses and duplicate-name conflict messages diverge from AWS

> _Note: this issue was identified against the code base at commit `c58e269113894f48dd3e032b4127d0ccc7e88163`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCodeInterpreter

## Description
The emulator's sandbox-style tool resources (code interpreters, browser profiles) never leave their transitional lifecycle state, and their duplicate-name refusals carry emulator-authored prose instead of AWS's ConflictException sentence. 5 causes must all be fixed before this issue is closed. The open issue 20261030T221315Z-bedrock-agentcore-control-browser-lifecycle-statuses-post-delete-reads-and-conflict.md describes the same fault family for the BROWSER resource only; it does not cover code interpreters or browser profiles, which are the resources below.

Cause 1 of 5: CreateCodeInterpreter reports a transitional status instead of the ready state.
AWS: CreateCodeInterpreter (Smithy: PUT /code-interpreters, HTTP 202; output member `status` -> CodeInterpreterStatus, required, values CREATING|CREATE_FAILED|READY|DELETING|DELETE_FAILED|DELETED). The recording shows AWS answering 202 with `status` READY for a code interpreter created with a sandbox network configuration — the resource is usable as soon as the create returns, so no CREATING phase is observable to the caller.
Emulator: answers 202 but serializes `status` as 'CREATING'.
Why: a client that waits for READY before invoking the interpreter polls forever (the emulator never advances the value on the create response), and a client that asserts the create response is READY fails outright.
Fix: the CreateCodeInterpreter handler's state initialisation / status projection for the code-interpreter resource.
Traces: bedrock-agentcore-control-code-interpreter-duplicate-and-errors#setup#1 (also bedrock-agentcore-control-code-interpreter-lifecycle#setup#1).

Cause 2 of 5: GetCodeInterpreter reads back the transitional status.
AWS: GetCodeInterpreter (Smithy: GET /code-interpreters/{codeInterpreterId}, HTTP 200; output `status` -> CodeInterpreterStatus, required). Reading a created custom code interpreter returns READY.
Emulator: answers 200 with `status` 'CREATING'.
Why: the read path confirms the resource is stuck in the transitional state, so any readiness gate built on GetCodeInterpreter never opens.
Fix: the code-interpreter state's status field (set READY on successful create) and the GetCodeInterpreter serializer that renders it.
Traces: bedrock-agentcore-control-code-interpreter-lifecycle#task#2.

Cause 3 of 5: DeleteCodeInterpreter reports DELETING where AWS reports the terminal deleted state.
AWS: DeleteCodeInterpreter (Smithy: DELETE /code-interpreters/{codeInterpreterId}, HTTP 202; output `status` -> CodeInterpreterStatus, required). The recording shows AWS answering 202 with `status` DELETED — the delete is complete when it returns.
Emulator: answers 202 with `status` 'DELETING'.
Why: a client that treats DELETED as the completion signal never sees it and keeps polling or reports the teardown as incomplete.
Fix: the DeleteCodeInterpreter handler's state transition / status projection.
Traces: bedrock-agentcore-control-code-interpreter-duplicate-and-errors#teardown#5 (also bedrock-agentcore-control-code-interpreter-lifecycle#teardown#3).

Cause 4 of 5: the CreateCodeInterpreter duplicate-name ConflictException message is emulator-authored.
AWS: CreateCodeInterpreter errors include ConflictException (Smithy: ConflictException, errorKind client, httpError 409, member `message` -> NonBlankString); `name` -> SandboxName is @pattern ^[a-zA-Z][a-zA-Z0-9_]{0,47}$ and must be unique in the account. Creating a second code interpreter with an existing name is refused 409 ConflictException with the message "CodeInterpreter with name '<name>' already exists in this account" — the fixed text is "CodeInterpreter with name '" ... "' already exists in this account" and the only variable part is the `name` taken verbatim from the request.
Emulator: answers 409 but with its own sentence of the form "a code interpreter named <name> already exists (codeInterpreterId <generatedId>)" — different wording, different capitalisation, and it leaks the colliding resource's generated id, which AWS does not disclose.
Why: clients and tests that match on AWS's message text (or redact it for logs) see unrecognised prose, and the extra generated id makes the message unstable across runs.
Fix: the CreateCodeInterpreter conflict path's error construction (message template), not the status code, which is already 409.
Traces: bedrock-agentcore-control-code-interpreter-duplicate-and-errors#task#2.

Cause 5 of 5: the CreateBrowserProfile duplicate-name ConflictException message is emulator-authored.
AWS: CreateBrowserProfile (Smithy: PUT /browser-profiles; errors include ConflictException httpError 409; `name` -> BrowserProfileName @pattern ^[a-zA-Z][a-zA-Z0-9_]{0,47}$) refuses a name that already exists in the account with 409 ConflictException and the message "Browser profile with name '<name>' already exists in this account" — fixed text "Browser profile with name '" ... "' already exists in this account", variable part is the request's `name`.
Emulator: answers 409 with "a browser profile named <name> already exists (profileId <generatedId>)" — non-AWS wording that additionally exposes the existing profileId.
Why: same as cause 4 — message-based handling breaks, and the response carries a per-run generated id AWS never returns here.
Fix: the CreateBrowserProfile conflict path's error construction (message template).
Traces: bedrock-agentcore-control-browser-profile-duplicate-and-notfound#task#2.

## Evidence
Cause 1 — bedrock-agentcore-control-code-interpreter-duplicate-and-errors#setup#1, CreateCodeInterpreter, expected 202 / actual 202: $.status value_mismatch (expected 'READY', got 'CREATING'); same cluster member bedrock-agentcore-control-code-interpreter-lifecycle#setup#1.
Cause 2 — bedrock-agentcore-control-code-interpreter-lifecycle#task#2, GetCodeInterpreter, 200 vs 200: $.status value_mismatch (expected 'READY', got 'CREATING').
Cause 3 — bedrock-agentcore-control-code-interpreter-duplicate-and-errors#teardown#5, DeleteCodeInterpreter, 202 vs 202: $.status value_mismatch (expected 'DELETED', got 'DELETING'); same cluster member bedrock-agentcore-control-code-interpreter-lifecycle#teardown#3.
Cause 4 — bedrock-agentcore-control-code-interpreter-duplicate-and-errors#task#2, CreateCodeInterpreter, 409 vs 409: $.message value_mismatch — expected "CodeInterpreter with name '<name>' already exists in this account", got "a code interpreter named <name> already exists (codeInterpreterId <id>)". In the recorded run the name was sim2real_probe_cintdup_67de1e63 and the leaked id sim2real_probe_cintdup_67de1e63-u3fwwgoi7s; those are that run's values, the requirement is the message template with the request's name substituted and no id disclosed.
Cause 5 — bedrock-agentcore-control-browser-profile-duplicate-and-notfound#task#2, CreateBrowserProfile, 409 vs 409: $.message value_mismatch — expected "Browser profile with name '<name>' already exists in this account", got "a browser profile named <name> already exists (profileId <id>)" (recorded run: name sim2real_probe_bprofdup_29483264, leaked profileId sim2real_probe_bprofdup_29483264-3cr7ct1g3w).
Status codes (202/200/202/409/409) matched in every cause; the divergences are the status enum values and the error message text.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture bedrock-agentcore-control-code-interpreter-duplicate-and-errors, step setup#1 — CreateCodeInterpreter and compare $.status against READY.
3. Cause 2: fixture bedrock-agentcore-control-code-interpreter-lifecycle, step task#2 — GetCodeInterpreter on the created interpreter and compare $.status against READY.
4. Cause 3: fixture bedrock-agentcore-control-code-interpreter-duplicate-and-errors, step teardown#5 — DeleteCodeInterpreter and compare $.status against DELETED.
5. Cause 4: same fixture, step task#2 — CreateCodeInterpreter reusing the existing name and compare the 409 body's $.message against "CodeInterpreter with name '<name>' already exists in this account".
6. Cause 5: fixture bedrock-agentcore-control-browser-profile-duplicate-and-notfound, step task#2 — CreateBrowserProfile reusing the existing name and compare the 409 body's $.message against "Browser profile with name '<name>' already exists in this account".
