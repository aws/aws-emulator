Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-serverless-cache-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-serverless-cache-lifecycle
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|ExportServerlessCacheSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: d77a58d64fb70ca743013b1e12d44799c7a2b26f

# ElastiCache ExportServerlessCacheSnapshot not-found refusal carries emulator prose instead of AWS message text

> _Note: this issue was identified against the code base at commit `d77a58d64fb70ca743013b1e12d44799c7a2b26f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportServerlessCacheSnapshot

## Description
One net-new root cause in the elasticache emulator; the replay marked no owning issue and no stub for it.

Cause 1 of 1: ExportServerlessCacheSnapshot against a serverless cache snapshot name that does not exist refuses with the right status and the right error code, but with the emulator's own sentence rather than the message AWS returns for that error class.
  AWS: Smithy ExportServerlessCacheSnapshot binds error ServerlessCacheSnapshotNotFoundFault, errorKind client, httpError 404, awsQueryError code `ServerlessCacheSnapshotNotFoundFault` / httpResponseCode 404; its documentation reads "This serverless cache snapshot could not be found or does not exist." The recorded AWS response is HTTP 404 with Error.Code `ServerlessCacheSnapshotNotFoundFault` and the FIXED sentence `Serverless cache snapshot with specified name does not exists.` — the whole message is fixed text for this error class: it has NO variable part, and in particular it does NOT interpolate the requested ServerlessCacheSnapshotName (AWS's own wording, including the non-standard "does not exists", is the requirement).
  Emulator: same HTTP 404 and same Error.Code, but Error.Message is `Serverless cache snapshot with name <ServerlessCacheSnapshotName> not found.` — a different sentence that additionally interpolates the requested snapshot name (the name itself is a per-run request value, not part of the template; the defect is the template).
  Why: a client that branches on, logs, surfaces or asserts AWS's documented not-found text for a serverless-cache-snapshot export (idempotent teardown, retry/skip logic, console-style error display) never matches the emulator's prose, so an export of a missing snapshot is reported as an unknown failure rather than a recognized not-found.
  Fix: the not-found refusal path of the ExportServerlessCacheSnapshot handler in the elasticache service crate (its ServerlessCacheSnapshotNotFoundFault construction / message formatting), not the status or the error code, both of which already match.
  Traces: elasticache-serverless-cache-lifecycle#task#6.

Related open issues describe the same CLASS of defect (emulator prose instead of AWS message text) for OTHER elasticache paths and do not carry this operation or this replay key: 20261109T151249Z-elasticache-refusal-messages-are-the-emulator-s-own--r2.md (ten message-text causes, none of them ExportServerlessCacheSnapshot), 20261108T152921Z-elasticache-refusals-diverge-from-aws-on-error-code.md and 20261105T143748Z-elasticache-teardown-refusals-diverge-from-aws-a-valid.md.

## Evidence
Wire replay of fixture elasticache-serverless-cache-lifecycle, step task#6 (ExportServerlessCacheSnapshot), expected_status 404 / actual_status 404, verdict mismatch:
  $.ErrorResponse.Error.Message: value_mismatch
    expected: 'Serverless cache snapshot with specified name does not exists.'
    actual:   'Serverless cache snapshot with name sim2real-probe-elasticache-serverless-snapshot-absent-18f0a2a3 not found.'
Only the Error.Message key diverged — the HTTP status (404) and Error.Code (ServerlessCacheSnapshotNotFoundFault, per the Smithy awsQueryError binding) match. In the actual message, `sim2real-probe-elasticache-serverless-snapshot-absent-18f0a2a3` is the ServerlessCacheSnapshotName taken from the request (a per-run generated name); the expected message is entirely fixed text with no request- or state-derived part.

## Steps to Reproduce
1. Point an AWS SDK/CLI elasticache client at the emulator's data listener.
2. Call ExportServerlessCacheSnapshot with a ServerlessCacheSnapshotName that does not exist in the emulator's state and any S3BucketName, e.g. ExportServerlessCacheSnapshot(ServerlessCacheSnapshotName='<name-never-created>', S3BucketName='<bucket>').
3. Observe the Query-protocol error response: HTTP 404, ErrorResponse.Error.Code = ServerlessCacheSnapshotNotFoundFault.
4. Compare ErrorResponse.Error.Message to AWS's fixed text 'Serverless cache snapshot with specified name does not exists.' — the emulator instead returns 'Serverless cache snapshot with name <name> not found.'.
5. Equivalently, replay fixture elasticache-serverless-cache-lifecycle and inspect step task#6.
