Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__bedrock-agentcore-control__lambda.harness-before-invocation-lambda-deny-stops-with-hook-stopped bedrock-agentcore__bedrock-agentcore-control__lambda.harness-duplicate-lambda-hook-names-rejected
Issue-Key: bedrock-agentcore__bedrock-agentcore-control__lambda.harness-before-invocation-lambda-deny-stops-with-hook-stopped|_role_usable#22|AssertionError|raise AssertionError("a harness with duplicate hook names was created, then deleted")
Issue-Key: bedrock-agentcore__bedrock-agentcore-control__lambda.harness-duplicate-lambda-hook-names-rejected|_rejection#19|AssertionError|raise AssertionError("CreateHarness accepted duplicate hook names; the harness was deleted")
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agentcore__bedrock-agentcore-control__lambda: CreateHarness accepts two hooks sharing one name instead of refusing the duplicate

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHarness

## Description
Cause 1 of 1: CreateHarness does not reject a hook list in which two hooks carry the same name.

AWS: recorded behavior only (the control-plane model constrains each hook's name but states no uniqueness rule). The recording shows CreateHarness refusing a request whose hook list repeats a hook name — the harness is not created — so the before-invocation/before-tool-call hook that a name identifies is unambiguous.

Emulator: the harness is created with both duplicate-named hooks ('CreateHarness accepted duplicate hook names; the harness was deleted'), and the dependent deny-hook scenario therefore runs against a harness AWS would never have created.

Why: a client that mistakenly registers two hooks under one name gets no refusal, and which of the two hooks a later decision came from is undefined — the hook identity a harness invocation reports cannot be trusted.

Fix: duplicate-name validation over the hook list in the CreateHarness handler's input validation.

Traces: the two checks below both assert the duplicate-name refusal before continuing.

## Evidence
Expected: CreateHarness with two Lambda hooks sharing one name is refused and no harness exists.
Actual: `AssertionError: CreateHarness accepted duplicate hook names; the harness was deleted` and `AssertionError: a harness with duplicate hook names was created, then deleted`.

## Steps to Reproduce
1. Create two Lambda functions to serve as harness hooks.
2. CreateHarness with a hook list whose two entries carry the same hook name (same event, e.g. before_invocation).
3. Observe HTTP 2xx and a created harness instead of a refusal.
