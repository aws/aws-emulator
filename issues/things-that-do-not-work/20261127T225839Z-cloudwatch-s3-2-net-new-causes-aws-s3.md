Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__s3.browser-post-upload-counts-in-post-requests cloudwatch__s3.multi-object-delete-counts-in-delete-requests
Issue-Key: cloudwatch__s3.browser-post-upload-counts-in-post-requests|_poll#21|AssertionError|raise AssertionError(f"{what} not met within {timeout_s}s (last observed: {last!r})")
Issue-Key: cloudwatch__s3.multi-object-delete-counts-in-delete-requests|_poll#21|AssertionError|raise AssertionError(f"{what} not met within {timeout_s}s (last observed: {last!r})")
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudwatch__s3: 2 net-new causes — AWS/S3 request metrics never count browser-form POST uploads in PostRequests, nor DeleteObjects requests in DeleteRequests

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Two gaps in the AWS/S3 request-metrics counters published for a whole-bucket metrics configuration. In both checks the configuration itself works and the PutRequests counter is exact (the warm-up and the PUT control both pass), so the divergence is confined to the POST and DELETE verbs.

Cause 1 of 2: a browser-form POST upload is not counted in PostRequests (nor, consequently, in AllRequests).
  AWS: with a request-metrics filter on the bucket, each POST Object request counts once in AWS/S3 PostRequests for the (BucketName, FilterId) dimensions and once in AllRequests, in the minute of the request. Two presigned-POST uploads answered 204 therefore give PostRequests Sum 2, and AllRequests the total of 1 PUT + 2 POST + 1 LIST.
  Emulator: the two POST uploads are served (HTTP 204 each) and both objects are listed, and the same window's PutRequests counts the control PUT exactly, but PostRequests stays 0 for the whole poll — the POST path never increments the counter.
  Why: a client measuring browser/presigned-POST traffic (upload dashboards, per-filter request-count alarms, cost attribution) sees no POST activity at all, and AllRequests under-reports the bucket's real request volume.
  Fix: the S3 POST Object (browser form) request path must increment the bucket's request-metrics counters (PostRequests and AllRequests) the way PutObject already does.
  Traces: cloudwatch__s3.browser-post-upload-counts-in-post-requests ('PostRequests counting the 2 POST uploads not met within 120.0s (last observed: 0)').

Cause 2 of 2: a Multi-Object Delete (DeleteObjects) request is not counted in DeleteRequests (nor in AllRequests).
  AWS: each DeleteObjects request counts ONCE in DeleteRequests regardless of how many keys it names, and once in AllRequests. Two DeleteObjects calls (one naming two keys, one naming one) therefore give DeleteRequests Sum 2 and AllRequests 3 PUT + 2 DeleteObjects = 5.
  Emulator: the deletes succeed (no Errors in either response) and the same window's PutRequests counts the three PUTs exactly, but DeleteRequests stays 0 for the whole poll.
  Why: deletion traffic is invisible in request metrics, so lifecycle/cleanup dashboards and request-count alarms read zero while objects are being removed, and AllRequests again under-reports.
  Fix: the DeleteObjects (and the single DeleteObject) request path must increment DeleteRequests and AllRequests once per REQUEST, not per key.
  Traces: cloudwatch__s3.multi-object-delete-counts-in-delete-requests ('DeleteRequests counting the 2 DeleteObjects requests not met within 120.0s (last observed: 0)').

Both metrics are AWS/S3 request metrics (recorded behavior only; they are not in the Smithy model), published per bucket metrics configuration as documented in the S3 metrics-and-dimensions reference. No open issue carries either replay key.

## Evidence
Suite: service cloudwatch__s3 scored 27/29 (93%), 2 failed; neither is OWNED or ALREADY FILED.

[mismatch] cloudwatch__s3.browser-post-upload-counts-in-post-requests
  AssertionError: PostRequests counting the 2 POST uploads not met within 120.0s (last observed: 0)
  Controls that passed: PutBucketMetricsConfiguration accepted; the filter's PutRequests series appeared during warm-up; in the measured minute PutRequests == 1 for the control PUT; both presigned POSTs answered 204 and both objects were listed.
  Expected (AWS): PostRequests Sum == 2 and AllRequests == 1 PUT + 2 POST + 1 LIST for the measured window.
  Actual (emulator): PostRequests 0.

[mismatch] cloudwatch__s3.multi-object-delete-counts-in-delete-requests
  AssertionError: DeleteRequests counting the 2 DeleteObjects requests not met within 120.0s (last observed: 0)
  Controls that passed: warm-up series appeared; PutRequests == 3 in the measured window; both DeleteObjects responses carried no Errors.
  Expected (AWS): DeleteRequests Sum == 2 (one per request, not per key) and AllRequests == 5.
  Actual (emulator): DeleteRequests 0.

Bucket names, filter ids and keys of the recorded runs are per-run values; the requirements are the namespace AWS/S3, the metric names PostRequests/DeleteRequests/AllRequests, the per-filter dimensions, and the one-count-per-request rule.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateBucket and PutBucketMetricsConfiguration with a whole-bucket filter id; PutObject until the filter's PutRequests series appears, then wait out a quiet gap.
3. Cause 1: PutObject once (control), then upload twice with s3:GeneratePresignedPost forms (each answered 204) and ListObjectsV2. GetMetricStatistics(AWS/S3, PutRequests / PostRequests / AllRequests, per-filter dimensions, Period 60, Statistics [Sum]) over that window.
   -> AWS: PutRequests 1, PostRequests 2, AllRequests 4.
   -> Emulator: PutRequests 1, PostRequests 0.
4. Cause 2: PutObject three keys, then DeleteObjects naming two keys and DeleteObjects naming one key. Same GetMetricStatistics reads.
   -> AWS: PutRequests 3, DeleteRequests 2, AllRequests 5.
   -> Emulator: PutRequests 3, DeleteRequests 0.
5. Re-replay both checks in trace_ids to confirm the fix.
