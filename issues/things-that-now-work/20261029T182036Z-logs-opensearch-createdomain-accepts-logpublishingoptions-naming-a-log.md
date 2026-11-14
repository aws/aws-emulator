Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__opensearch.opensearch-log-publishing-to-a-group-without-an-es-grant-is-rejected logs__opensearch.opensearch-search-slow-logs-reach-the-log-group-only-for-an-index-over-threshold
Issue-Key: logs__opensearch.opensearch-log-publishing-to-a-group-without-an-es-grant-is-rejected|run#18|AssertionError|assert refused is not None, (
Issue-Key: logs__opensearch.opensearch-search-slow-logs-reach-the-log-group-only-for-an-index-over-threshold|_get#30|AssertionError|assert status == 200, f"expected HTTP 200 from GET {path}, got {status}: {body!r}"
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# logs__opensearch: CreateDomain accepts LogPublishingOptions naming a log group whose resource policy grants es.amazonaws.com nothing, and slow-log verification is blocked by the unserved domain data plane (2 causes)

> _Note: this issue was identified against the code base at commit `1a0d537a6f45d8cc06f79b3c3e7692a297108dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
This issue bundles 2 root causes spanning the cloudwatch-logs/opensearch pair; the leading token of both failing check ids is logs__opensearch. The open issue 20261025T024958Z-logs-opensearch-opensearch-updatedomainconfig-accepts-logpublishingoptions-naming-a-cloudwatch-log.md covers the UpdateDomainConfig/nonexistent-log-group variant; neither check below was marked owned by it. Each cause must be closed before submitting.

Cause 1 of 2: CreateDomain does not verify that the CloudWatch Logs resource policy of the named log group lets OpenSearch write to it.
AWS: Smithy: CreateDomain.LogPublishingOptions is a map of LogType (INDEX_SLOW_LOGS, SEARCH_SLOW_LOGS, ES_APPLICATION_LOGS, AUDIT_LOGS) to LogPublishingOption{CloudWatchLogsLogGroupArn, Enabled}, and CreateDomain models ValidationException (400). AWS resolves the named log group and checks that its CloudWatch Logs resource policy grants the es.amazonaws.com service principal logs:PutLogEvents/logs:CreateLogStream on it; if the policy grants es.amazonaws.com nothing, CreateDomain is REFUSED 400 ValidationException naming the log group ARN that cannot be written to, and no domain is created.
Emulator: CreateDomain is accepted (HTTP 200, domain created with LogPublishingOptions echoed) even though the target group's resource policy grants the service principal nothing.
Why: a client ships a domain configured to publish logs that can never be delivered; the misconfiguration AWS reports at submit time becomes a silent, permanent gap in observability, and the IAM/resource-policy precondition disappears from the test matrix.
Fix: CreateDomain's input validation for LogPublishingOptions must resolve each CloudWatchLogsLogGroupArn and evaluate the group's resource policy for the es.amazonaws.com principal before accepting the request (the same validator the UpdateDomainConfig path needs).
Traces: logs__opensearch.opensearch-log-publishing-to-a-group-without-an-es-grant-is-rejected.

Cause 2 of 2: slow-log publication cannot be observed because the domain REST data plane is not served.
AWS: a domain with SEARCH_SLOW_LOGS enabled and an index-level slow-log threshold set via the index settings REST API emits a log event to the configured log group only for a query over the threshold; reading the index settings with a signed `GET /<index>/_settings` on the published endpoint answers HTTP 200 with the settings document. Recorded behavior (data plane is outside the Smithy model).
Emulator: the signed `GET /<index>/_settings` on the published endpoint answers HTTP 404 with {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"} — the fixed text is `no emulated service is registered for '<signing name>'` with the signing name (`es`) taken from the request's credential scope. No index settings can be written or read, so no slow-log behaviour exists to verify.
Why: the whole slow-log feature — threshold configuration and the conditional emission that depends on it — is unreachable; a client cannot confirm that only over-threshold searches are logged.
Fix: the request router must bind the domain endpoint host to an OpenSearch REST data-plane handler and that handler must serve the index-settings API (same root cause as the opensearch data-plane issue filed in this sweep).
Traces: logs__opensearch.opensearch-search-slow-logs-reach-the-log-group-only-for-an-index-over-threshold.

## Evidence
Cause 1 — expected: 400 ValidationException from CreateDomain, no domain created. Actual: `AssertionError: expected CreateDomain publishing INDEX_SLOW_LOGS to arn:aws:logs:us-west-2:000000000000:log-group:sim2real-behav-osgrant-0600bca91aa3, whose resource policy grants es.amazonaws.com nothing, to be REJECTED, but it was accepted`.
Cause 2 — expected HTTP 200 from `GET /<index>/_settings` on the published endpoint. Actual: `AssertionError: expected HTTP 200 from GET /sim2real-osslow-logged/_settings, got 404: {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}`.

## Steps to Reproduce
1. Cause 1: CreateLogGroup in CloudWatch Logs, then PutResourcePolicy that grants nothing to es.amazonaws.com (or omit the grant entirely). Call opensearch CreateDomain with LogPublishingOptions={'INDEX_SLOW_LOGS': {'CloudWatchLogsLogGroupArn': <that group arn>, 'Enabled': true}} — observe HTTP 200 instead of 400 ValidationException, and DescribeDomain showing the domain exists.
2. Cause 2: create a domain with SEARCH_SLOW_LOGS publishing enabled, wait for the endpoint, then send a SigV4-signed (service `es`) `GET https://<Endpoint>/<index>/_settings` — observe HTTP 404 {"__type":"UnknownService",...} instead of the settings document.
