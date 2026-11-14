Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.health-check-str-match-searchstring-governs-the-reported-health
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.health-check-str-match-searchstring-governs-the-reported-health|_poll#19|Timeout|time.sleep(interval_s)
Issue-Commit: a04a6917251164fb3627e673f5d6322225de49e4

# Route 53 HTTP_STR_MATCH SearchString is never evaluated: GetHealthCheckStatus reports "Success: HTTP Status Code 200, OK" from every checker unconditionally, so a string-match health check can never report Failure

> _Note: this issue was identified against the code base at commit `a04a6917251164fb3627e673f5d6322225de49e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHealthCheckStatus

## Description
One net-new root cause (Cause 1 of 1), grounded in failing check route53.health-check-str-match-searchstring-governs-the-reported-health.

Cause 1 of 1: an endpoint-type health check's reported status is a CONSTANT, not an assessment of the monitored endpoint, so `SearchString` has no effect on anything observable.

AWS: Smithy `HealthCheckConfig$SearchString` — "If the value of Type is HTTP_STR_MATCH or HTTPS_STR_MATCH, the string that you want Amazon Route 53 to search for in the response body from the specified resource. If the string appears in the response body, Route 53 considers the resource healthy. Route 53 considers case when searching for SearchString in the response body." `GetHealthCheckStatus` returns `HealthCheckObservations`, "one HealthCheckObservation element for each Amazon Route 53 health checker that is reporting a status about the health check endpoint", each carrying `StatusReport$Status`, "a description of the status of the health check endpoint as reported by one of the Amazon Route 53 health checkers" (free text, unconstrained by the model). So `SearchString` is a content PREDICATE re-evaluated on every probe: while the fetched body contains the string every checker reports a status whose fixed leading text is `Success: `; once the body no longer contains it — with nothing about the health check itself changed — every checker reports a status whose fixed leading text is `Failure: ` and whose reason names string matching rather than a transport problem. Recorded behavior (the check's own real-AWS replay note, 2026-09-18, us-west-2): the failure wording measured against real Route 53 was `Failure: STRING_MATCH_FAILURE` — `Failure: STRING_MATCH_FAILURE` is the fixed text; nothing in that prefix comes from the request, and only the trailing detail sentence varies with what the checker observed. The model documents no wording for it, so the exact trailing detail is "recorded behavior only"; what is a requirement is the `Failure: ` opening and that the reason names string matching, since a connect/DNS/timeout failure is a different fact about the endpoint.

Emulator: the status is hard-coded. `StoredHealthCheck::reported_status()` returns `STATUS_TCP_CONNECTION_ESTABLISHED` ("Success: TCP connection established") for Type=TCP and otherwise always `STATUS_HTTP_OK` ("Success: HTTP Status Code 200, OK"), and `to_health_check_observations()` stamps that one `StatusReport` onto every checker region. Neither `SearchString` nor the body of the monitored endpoint is read anywhere on this path — the only things the stored config decides are WHICH regions report, WHETHER any report at all, and WHEN they last did. Consequently the replayed check passed its first phase (every checker Success while the monitored body contained the marker, checker IPs inside published `GetCheckerIpRanges` CIDRs, 3+ regions), then rewrote the monitored endpoint's body so the marker was gone and polled for every checker to report Failure — which can never happen — and the check was killed by the 60s harness limit while still polling. The same emulator also contradicts itself: the health-check EVALUATOR behind the CloudWatch `HealthCheckStatus` series classifies EVERY endpoint-type probe as a failure (`observe()` returns `Some(false)` for every type in `ENDPOINT_HEALTH_CHECK_TYPES`, on the stated grounds that the emulator makes no outbound connection), so the CloudWatch series for the very same health check publishes 0/unhealthy while `GetHealthCheckStatus` reports `Success: HTTP Status Code 200, OK` to the same caller. One of the two reporters must be wrong for any given endpoint check, and today they are wired to disagree on all of them.

Why: a client gets the one fact a health check exists to supply inverted or frozen. `SearchString` is accepted and stored verbatim, so configuration round-trips look perfect, but content-based monitoring is inert: an endpoint that stops serving the required string is still reported healthy by every checker forever, a DNS-failover runbook or test that waits for a string-match health check to go unhealthy hangs rather than failing, and `GetHealthCheckLastFailureReason`/`StatusReport` never produce the string-match reason a client branches on. Conversely, a client that watches the CloudWatch series instead sees the check unhealthy from the start, so the two supported ways of observing one health check give opposite answers. Any emulator target that merely stores `SearchString` is currently indistinguishable from Route 53, which is exactly the gap this check was written to close.

Fix: the health-check state/evaluator, not the serializer. `StoredHealthCheck::reported_status()` must stop returning a constant and instead derive each probe's report from an actual assessment of the configured endpoint, and that assessment must be shared with the CloudWatch `HealthCheckStatus` publisher so one health check has one status. For the endpoint types, the assessment the emulator CAN make is against endpoints its own world serves (the monitored host in this check is an S3 static-website bucket in the same emulator, reachable in-process): fetch/behave as the configured `Type`, `FullyQualifiedDomainName`/`IPAddress`, `Port` and `ResourcePath` describe, and for `HTTP_STR_MATCH`/`HTTPS_STR_MATCH` apply the documented case-sensitive substring test of `SearchString` against the response body, reporting a `Success: `-prefixed status when it matches and a `Failure: `-prefixed status naming string matching when it does not, re-evaluated per probe so a later body rewrite flips the report with `HealthCheckVersion` and the stored config untouched. An endpoint the emulator genuinely cannot reach should report a transport failure, not a fabricated success.

No other open issue covers this: the open route53 health-check issues (20261025T035714Z-route53-route-53-health-check-references-are-unusable-changeresourcerecordsets.md, 20261025T115222Z-route53-route-53-health-check-refusals-use-the-error.md, 20261027T004357Z-route53-route-53-nosuchhealthcheck-from-gethealthcheckstatus-gethealthchecklastfailurereason-carries-the.md) are about HealthCheckId references and about refusal/not-found MESSAGE wording, not about whether a probe is assessed at all.

## Evidence
Failing check: route53.health-check-str-match-searchstring-governs-the-reported-health — verdict mismatch, "Check timed out after 60.0s" (killed while polling for the Failure phase; the poll's own budget is 240s).

Expected (AWS):
  Phase 1, monitored body CONTAINS SearchString -> every HealthCheckObservation[].StatusReport.Status starts with the fixed text "Success: ".
  Phase 2, monitored body REWRITTEN so it no longer contains SearchString, health check untouched -> every HealthCheckObservation[].StatusReport.Status starts with the fixed text "Failure: " and its reason names string matching; real-AWS replay measured the wording "Failure: STRING_MATCH_FAILURE" (fixed text; no part of that prefix is taken from the request). HealthCheckVersion and HealthCheckConfig (SearchString, Inverted=false) unchanged across the flip.
  Phase 3, body restored -> every checker back to a "Success: "-prefixed status.

Actual (emulator): all three phases report the identical constant status from every checker, "Success: HTTP Status Code 200, OK" (and "Success: TCP connection established" for Type=TCP). Phase 2 therefore never arrives and the check is cut off by the harness limit. No status containing "string"/STRING_MATCH is producible by any input.

Source of the constant (citations, not a patch):
  services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs:1113-1118 — reported_status(): `if type == TCP { STATUS_TCP_CONNECTION_ESTABLISHED } STATUS_HTTP_OK`; SearchString and the endpoint body are not read.
  services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs:1124-1142 — to_health_check_observations(): one cloned StatusReport applied to every checker region.
  services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs:1157,1161 — the two constant status strings.
  services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs:1104-1112 — the handler's own comment conceding "a checker of an endpoint this service is checking has no failure to report".
  services/service-com-amazonaws-route53-awsdnsv20130401/src/state/health_check_status.rs:302-311 — observe(): every ENDPOINT_HEALTH_CHECK_TYPES probe yields Some(false), i.e. the CloudWatch series calls the same check unhealthy.
  services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/get_health_check_status.rs:149-169 — the handler is a pure read of the derived constant.

Controls that did hold (so the divergence is isolated to the content predicate): CreateHealthCheck stored SearchString verbatim; GetCheckerIpRanges published CIDRs and every reported checker IPAddress fell inside one; observations came from 3+ regions.

## Steps to Reproduce
1. Stand up a publicly readable HTTP origin the emulator itself serves whose body contains a marker string, e.g. an S3 static-website bucket in the same emulator with index.html = "MARKER" (the replayed check uses CreateBucket + PutPublicAccessBlock + PutBucketPolicy(Principal "*") + PutBucketWebsite + PutObject, host <bucket>.s3-website-<region>.amazonaws.com).
2. route53 CreateHealthCheck with HealthCheckConfig {Type: HTTP_STR_MATCH, FullyQualifiedDomainName: <that host>, Port: 80, ResourcePath: "/", SearchString: "MARKER", RequestInterval: 10, FailureThreshold: 1, MeasureLatency: false, Inverted: false}.
3. Poll route53 GetHealthCheckStatus(HealthCheckId) until every HealthCheckObservations[].StatusReport.Status starts with "Success" — observed immediately.
4. PutObject the same key with a body that does NOT contain "MARKER"; confirm via GetObject. Change nothing about the health check.
5. Poll GetHealthCheckStatus again. AWS: within a few RequestIntervals every checker reports a "Failure: "-prefixed status naming string matching (measured: "Failure: STRING_MATCH_FAILURE"). Emulator: every checker keeps reporting "Success: HTTP Status Code 200, OK" indefinitely; the poll never terminates.
6. Restore the marker in the body — the emulator's answer is unchanged throughout, i.e. the three phases are indistinguishable.
7. Side-by-side inconsistency: for the same HealthCheckId, read the CloudWatch AWS/Route53 HealthCheckStatus series — it publishes 0 (unhealthy) while GetHealthCheckStatus reports Success.
