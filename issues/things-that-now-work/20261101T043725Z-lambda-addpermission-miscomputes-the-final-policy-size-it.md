Issue-Source: trace-sweep-divergence
Issue-Trace: lam-err-policy-length-exceeded
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-err-policy-length-exceeded
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|AddPermission|4xx->4xx|$.message
Issue-Commit: 378c597fdd478fd1007bc4677beab7b05ad3e463

# Lambda AddPermission miscomputes the final policy size it reports in the PolicyLengthExceeded refusal

> _Note: this issue was identified against the code base at commit `378c597fdd478fd1007bc4677beab7b05ad3e463`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Single cause (Cause 1 of 1): AddPermission's resource-policy size accounting disagrees with AWS's, so the byte count it reports when the policy grows past the account limit is wrong.

AWS: AddPermission appends the requested statement to the function's resource policy and, when the resulting document exceeds the 20480-byte policy limit, refuses the request 400 PolicyLengthExceededException (Smithy: AddPermission errors include PolicyLengthExceededException, errorKind client, httpError 400; POST /2015-03-31/functions/{FunctionName}/policy). The refusal body's `message` is the fixed template `The final policy size (<N>) is bigger than the limit (<L>).` — the fixed text is `The final policy size (`, `) is bigger than the limit (`, `).`; `<N>` is the byte length of the FINAL policy document AWS would have stored (the complete compact JSON document: the `Version`/`Id` envelope plus every already-stored statement plus the statement being added, each rendered with its Sid, Effect, Principal, Action and Resource ARN), and `<L>` is the account's policy-size limit, 20480. The limit value is modeled/documented; the size value is whatever the document actually measures for that request sequence, so it is a function of the request and of state, not a per-run random value.

Emulator: status (400), error class and the limit are all correct, but the reported size is 25 bytes short of AWS's for the identical request sequence — the recorded refusal says `The final policy size (23993) is bigger than the limit (20480).` and the emulator answers `The final policy size (23968) is bigger than the limit (20480).`. The emulator therefore measures a document that is not the one AWS would store. Hypothesis: the size is computed over a partially-rendered document (statement list only, or without the full `{"Version":"2012-10-17","Id":"default","Statement":[...]}` envelope / without the trailing statement's full Resource ARN) rather than over the serialized final policy. Note this is the same policy-serialization area as the open issue 20261029T233239Z-lambda-resource-policy-documents-are-re-serialized-with.md (Lambda resource-policy documents are re-serialized with whitespace) — that issue is about the document returned by Put/GetResourcePolicy and does not own this AddPermission step or its size accounting.

Why: the number is not cosmetic — it is the emulator's own measurement of the policy, so the accept/reject boundary sits 25 bytes away from AWS's. A client that adds statements up to the limit (a deploy tool that grants one invoke permission per source) gets a different last-accepted statement than against AWS: a policy AWS refuses is accepted by the emulator, and callers that parse the size out of the message to compute remaining headroom, or that assert on the refusal text, compute/compare the wrong value.

Fix: AddPermission's policy-size validation in the lambda operation handler — it must measure the byte length of the fully serialized final policy document (the same bytes the resource-policy state would store and GetResourcePolicy would return) and interpolate that count into AWS's message template, rather than measuring a partial rendering.

Traces: fixture lam-err-policy-length-exceeded step task#6 (AddPermission, expected 400 / actual 400, `$.message` value_mismatch on the size).

## Evidence
Replay cluster service=lambda|AddPermission|status=400->400|keys=$.message, fixture lam-err-policy-length-exceeded step task#6, affected_count 1 (lam-err-policy-length-exceeded#task#6).

Expected (recorded AWS response), HTTP 400: {"message": "The final policy size (23993) is bigger than the limit (20480)."}
Actual (emulator), HTTP 400: {"message": "The final policy size (23968) is bigger than the limit (20480)."}

Status, error class and the limit token (20480) agree; only the emulator's computed final-policy byte count diverges, by 25 bytes short. Fixed text of the message: "The final policy size (" ... ") is bigger than the limit (" ... ").". Variable parts: the final policy size, derived from the request plus the function's stored resource policy, and the limit, the account policy-size quota (20480).

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Fixture lam-err-policy-length-exceeded builds up a function resource policy with repeated AddPermission calls until the document passes the 20480-byte policy limit.
3. At step task#6 the emulator correctly refuses with 400 PolicyLengthExceededException, but the body reports a final policy size of 23968 where the recorded AWS response reports 23993 for the identical request sequence.
4. Compare the reported size with the byte length of the serialized final policy document (what GetResourcePolicy would return after the statement was appended) — they must match.
