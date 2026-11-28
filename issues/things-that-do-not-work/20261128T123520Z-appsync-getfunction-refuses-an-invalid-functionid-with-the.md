Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-get-function-id-grammar
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-get-function-id-grammar
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|GetFunction|4xx->4xx|$.message
Issue-Commit: b880698c4f2cb4e70545429d30696ac71b7bccc0

# AppSync GetFunction refuses an invalid functionId with the generic constraint text instead of AWS's "The Function Id is invalid" message

> _Note: this issue was identified against the code base at commit `b880698c4f2cb4e70545429d30696ac71b7bccc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunction

## Description
One net-new root cause in the appsync emulator; it must be closed before this issue is submitted.

=== Cause 1 of 1: GetFunction's functionId refusal carries the generic validation-framework message instead of AppSync's function-id refusal text ===
AWS: Smithy (appsync) GetFunction — GET /v1/apis/{apiId}/functions/{functionId}, functionId: ResourceName @httpLabel @required @length 1-65536 @pattern ^[_A-Za-z][_0-9A-Za-z]*$; modeled errors ConcurrentModificationException|409, NotFoundException|404, UnauthorizedException|401 (service-common BadRequestException|400 for refusals). Recorded behavior for a functionId that does not satisfy that grammar (for example an id beginning with a digit) is HTTP 400 with the message 'The Function Id is invalid. Please refer to the documentation for more details.' — that text is entirely FIXED: it interpolates no request value, does not name the member, and does not publish the pattern. AppSync words this refusal itself rather than emitting its generic constraint report for this member.
Emulator: HTTP 400 (the status and the x-amzn-errortype header both match the recording) but the body message is the generic framework report "1 validation error detected: Value '<the submitted functionId>' at 'functionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$" — i.e. it echoes the rejected value and publishes an anchored form of the member pattern where AWS returns its own fixed sentence.
Why: the error message is part of the contract. A client, test suite or operator alert that recognises AWS's 'The Function Id is invalid. Please refer to the documentation for more details.' to tell a malformed function id apart from other 400 refusals sees an unrecognised string, and a client that logs or surfaces the message shows the caller a constraint report AWS never emits (including the submitted id, which AWS deliberately does not echo here).
Fix: the GetFunction handler's request validation / refusal path — the functionId label refusal must produce AppSync's fixed message for an invalid function id instead of falling through to the shared constraint-report formatter. NOTE: the open issues 20261127T093942Z-appsync-generic-validation-messages-interpolate-the-rejected-value.md (generic constraint text interpolating the value and anchoring the pattern) and 20261127T093942Z-appsync-identifier-grammar-and-query-parse-refusals-return.md (identifier-grammar refusals on lookup paths) describe neighbouring faults and may be closed by the same change, but neither carries this cluster's replay key, so this cause is not covered by them.
Traces: appsync-get-function-id-grammar#task#4, appsync-get-function-id-grammar#task#5.

## Evidence
Cause 1 — appsync-get-function-id-grammar#task#4 GetFunction (same cluster also covers #task#5): status 400 expected and 400 observed, x-amzn-errortype matched; $.message value_mismatch — expected 'The Function Id is invalid. Please refer to the documentation for more details.' (fixed text, no variable parts), got "1 validation error detected: Value '7l25rax6unuvhaa7gf3xxkgbkgh' at 'functionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$" (the quoted id is the request's functionId value, not part of the required template; the required message has no variable parts at all).

## Steps to Reproduce
1. CreateGraphqlApi(name=<any>, authenticationType='API_KEY') and note apiId.
2. GET /v1/apis/{apiId}/functions/{functionId} with a functionId that violates ^[_A-Za-z][_0-9A-Za-z]*$ — e.g. one beginning with a digit (appsync get-function --api-id <apiId> --function-id 7l25rax6unuvhaa7gf3xxkgbkgh).
3. Expected: HTTP 400 whose message is exactly 'The Function Id is invalid. Please refer to the documentation for more details.'
   Actual: HTTP 400 whose message is "1 validation error detected: Value '<submitted id>' at 'functionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$".
4. Re-replay fixture appsync-get-function-id-grammar (steps task#4 and task#5) to confirm.
