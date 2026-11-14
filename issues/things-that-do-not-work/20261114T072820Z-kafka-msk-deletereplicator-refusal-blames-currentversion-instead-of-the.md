Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-replicator-notfound-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-replicator-notfound-errors
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DeleteReplicator|4xx->4xx|$.invalidParameter
Issue-Commit: 5e63c3f3cfbb2d12f4a6b6c85ae0a2dcb02d6ef6

# MSK DeleteReplicator refusal blames currentVersion instead of the replicatorArn that is actually invalid

> _Note: this issue was identified against the code base at commit `5e63c3f3cfbb2d12f4a6b6c85ae0a2dcb02d6ef6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteReplicator

## Description
ONE net-new root cause in the kafka (MSK) service survives every ALREADY-FILED / OWNED filter in this replay of simparity-wire-aws-v1. All 24 divergence clusters in the run belong to kafka; 23 are bound to an existing open issue by the replay's own owned_by linkage, and this is the only unowned one.

=== Cause 1 of 1: DeleteReplicator's BadRequestException names `currentVersion` as the offending parameter when the parameter MSK blames is `replicatorArn` ===

AWS: Smithy com.amazonaws.kafka#Kafka, operation DeleteReplicator, `DELETE /replication/v1/replicators/{ReplicatorArn}`, success code 200. Its input has exactly two members: `ReplicatorArn` (`@httpLabel`, `required: true`, target `__string`) and `CurrentVersion` (`@httpQuery("currentVersion")`, target `__string`, NOT required). Modeled errors: BadRequestException 400, UnauthorizedException 401, ForbiddenException 403, NotFoundException 404, TooManyRequestsException 429, InternalServerErrorException 500, ServiceUnavailableException 503. `kafka#BadRequestException` declares member `InvalidParameter` with jsonName `invalidParameter`, documented as "The parameter that caused the error.", and member `Message` with jsonName `message` ("The description of the error."). The model does not state which member a given refusal blames, so the attribution rule is recorded behavior only: when DeleteReplicator is handed a replicator ARN the service refuses to resolve, MSK answers HTTP 400 with error code `BadRequestException` and sets `invalidParameter` to the camelCase wire name of the parameter that caused the error — here `replicatorArn`, the path-bound ARN. The accompanying `message` is the generic refusal sentence and the emulator already reproduces it exactly, so the message text is NOT part of this defect; the only requirement at issue is which parameter token the body names. `invalidParameter` is a fixed token (a parameter name from the API's wire vocabulary), not a value interpolated from the request — the replicator ARN, account id, region and the generated replicator name in the request are per-run values and are not requirements. The optional `currentVersion` query parameter cannot be the cause of the refusal: it is unconstrained in the model and is not even required, so an absent or stale value is not what MSK rejects on this path.

Emulator: status 400 and error code `BadRequestException` are correct and the `message` matches AWS's, but the body carries `"invalidParameter": "currentVersion"` where AWS carries `"invalidParameter": "replicatorArn"`. It therefore blames an entirely DIFFERENT input member — not merely a mis-cased one. Hypothesis (neither the Smithy model nor the recording states the emulator's internal ordering): the DeleteReplicator handler validates/compares the optional `currentVersion` query binding before it resolves the path-bound `ReplicatorArn`, so the first guard that fires attributes the refusal to the parameter it happened to be examining rather than to the ARN that could not be resolved.

Why: `invalidParameter` is the one machine-readable field in an MSK error that tells a client WHICH request input to fix. A client that dispatches on it to point an operator at the offending field, to retry with a corrected argument, or to classify the refusal is sent to `currentVersion` — an optional parameter that is fine — and will "fix" it forever while the real problem, the replicator ARN, is never surfaced. Because the status, the error code and the message are all correct, nothing else in the response signals the misattribution, so the client has no way to detect it.

Fix: the DeleteReplicator handler in the kafka service (its input validation / ARN-resolution ordering and the BadRequestException it constructs) — the refusal raised when the path-bound replicator ARN cannot be resolved must pass `replicatorArn` as the blamed parameter, which also means resolving `ReplicatorArn` before inspecting the optional `currentVersion` query binding. This is NOT fixed by the shared camelCase-ification of the error constructor: the token emitted here is already camelCase, it just names the wrong member.

Related OPEN issue that does NOT cover this cause: 20261029T202809Z-kafka-msk-error-payloads-name-the-smithy-member-instead.md bundles nine `invalidParameter` causes of the same FAMILY (and two of its nine — DescribeClusterOperation and DescribeVpcConnection — are likewise wrong-parameter rather than wrong-case), but it carries no DeleteReplicator cause and none of its Issue-Key / Issue-Trace-Ids entries name the fixture step below, so a fixer closing it would never exercise this path. Also neighbouring but distinct: 20261029T202809Z-kafka-msk-not-found-already-exists-and-tagging-refusals.md owns the DescribeReplicator message divergence in this same fixture (task#1), and 20261112T094430Z-kafka-4-net-new-causes-createconfiguration-s-per.md touches DeleteReplicator only as part of a never-settling lifecycle, not its refusal payload.

Traces: kafka-replicator-notfound-errors#task#3 (DeleteReplicator). The cluster has exactly one member, so this fixture step is the whole cause.

## Evidence
Corpus: simparity-wire-aws-v1 — 50 fixtures replayed, 163 steps, 131 passed, 32 mismatched, 0 errored, 0 blocked, score 80.4%. emulator_up = true ("/__admin/health answered 200 and the :40511 data listener accepts"), bootstrap_signal = false, not_implemented = 0. 24 divergence clusters, every one on service `kafka`.

This is the ONLY cluster in the run with an EMPTY `owned_by` and an empty `stub_path`; the other 23 are reported as owned by existing open issues and are skipped per the replay's own linkage.

Cause 1 of 1 — divergence_key `service=kafka|DeleteReplicator|status=400->400|keys=$.invalidParameter`:
  fixture_id: kafka-replicator-notfound-errors
  step_key:   task#3
  operation:  DeleteReplicator
  verdict:    mismatch
  expected_status 400, actual_status 400 (status AGREES — the HTTP status and the error code are not at issue)
  detail (complete, verbatim): "$.invalidParameter: value_mismatch (expected 'replicatorArn', got 'currentVersion')"
  affected: ["kafka-replicator-notfound-errors#task#3"], affected_count 1
  owned_by: [] , stub_path: "" , stub_service: ""

Expected (AWS): HTTP 400, x-amzn-errortype `BadRequestException`, body `{"invalidParameter": "replicatorArn", "message": "<MSK's refusal sentence>"}`.
Actual (emulator): HTTP 400, x-amzn-errortype `BadRequestException`, body `{"invalidParameter": "currentVersion", "message": "<same sentence>"}`.
$.invalidParameter is the ONLY diverging key in the whole cluster — `status`, `$.message` and `x-amzn-errortype` were all compared and all matched, so the message's fixed text and its variable parts are already correct and only the blamed-parameter token is wrong. The token itself is fixed text (a wire parameter name), not a value drawn from the request or from emulator state; the replicator ARN, region, account and replicator name in the step's request are per-run values and are not asserted.

Smithy cited (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json): DeleteReplicator http {method: DELETE, uri: /replication/v1/replicators/{ReplicatorArn}, code: 200}; input ReplicatorArn {httpLabel, required: true}, CurrentVersion {httpQuery: "currentVersion", NOT required}; errors BadRequestException 400 / UnauthorizedException 401 / ForbiddenException 403 / NotFoundException 404 / TooManyRequestsException 429 / InternalServerErrorException 500 / ServiceUnavailableException 503; BadRequestException members InvalidParameter -> jsonName "invalidParameter" ("The parameter that caused the error.") and Message -> jsonName "message" ("The description of the error.").

## Steps to Reproduce
1. Boot the emulator from this checkout (the kafka service must be serving; /__admin/health answers 200 and the data listener accepts).
2. Replay wire fixture `kafka-replicator-notfound-errors` and stop at step `task#3`, the DeleteReplicator call.
3. Assert the refusal envelope: HTTP status 400 and x-amzn-errortype `BadRequestException` (the emulator already gets both right).
4. Assert the blamed parameter: the response body's `invalidParameter` MUST equal the string `replicatorArn`. The emulator returns `currentVersion` — this is the failure.
5. Assert `message` is unchanged by the fix: it must keep matching AWS's refusal sentence byte-for-byte, exactly as it already does in this replay. Do not alter the message while re-attributing the parameter.
6. Confirm the attribution is ordering-correct, not hard-coded: re-issue DeleteReplicator for the same unresolvable replicator ARN (a) with `currentVersion` omitted entirely and (b) with an arbitrary `currentVersion` value. Both must still answer 400 `BadRequestException` with `invalidParameter == "replicatorArn"` — the optional query parameter must never become the blamed parameter when the path-bound ARN is what could not be resolved.
7. Positive control on the neighbouring path: DescribeReplicator in the same fixture (`task#1`) must keep answering 404 `NotFoundException`; this fix must not change which error CLASS DeleteReplicator returns on this input (it stays 400 BadRequestException).
8. Re-replay `kafka-replicator-notfound-errors#task#3`; the step must pass with the status and message unchanged.
