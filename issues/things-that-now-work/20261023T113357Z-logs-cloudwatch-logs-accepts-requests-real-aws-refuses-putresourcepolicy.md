Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists events__logs.xsvc-logs-1-bus-logging-include-detail-none-omits-the-event-detail events__logs.xsvc-logs-1-custom-bus-rule-writes-the-exact-event-envelope-to-a-log-group events__logs.xsvc-logs-1-error-level-bus-log-records-the-no-permissions-invocation-failure events__logs.xsvc-logs-1-input-template-without-timestamp-and-message-not-delivered-to-log-group events__logs.xsvc-logs-1-input-transformer-renders-message-and-event-time-into-the-log-record events__logs.xsvc-logs-1-log-group-without-events-policy-not-delivered-and-counts-failed-invocations events__logs.xsvc-logs-1-near-miss-detail-type-rule-not-delivered-to-its-log-group logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups logs__opensearch.opensearch-search-slow-logs-reach-the-log-group-only-for-an-index-over-threshold logs__opensearch.osdocstat-describe-domains-batch-matches-describe-domain logs__opensearch.osdocstat-four-log-types-round-trip-each-to-its-own-group logs.op-contract-delete-resource-policy-stale-revision-rejected logs.resource-policy-revision-conflict-rejected logs.associate-kms-key-invalid-key-arn-rejected logs.associate-kms-key-malformed-arn-rejected logs.associate-kms-key-without-service-permission-rejected logs.kms-grant-scoped-to-another-log-group-rejected logs.deep-logs-2-logcmk-disabled-key-associate-rejected kms__logs.associate-kms-key-alias-versus-key-arn kms__logs.xsvc-logs-2-associate-disabled-cmk-rejected-until-enabled kms__logs.xsvc-logs-2-cmk-scoped-to-group-a-rejects-associate-on-group-b kms__logs.xsvc-logs-2-create-log-group-with-out-of-scope-cmk-rejected logs.access-denied-enforcement logs.get-log-object-access-denied-enforced logs.op-contract-describeLogGroups logs.put-log-events-batch-contract-rejected logs.deep-cwlogs-semantics-put-log-events-surviving-span-over-24h-rejected cloudwatch__lambda__logs.logs-subscription-filter-missing-permission-negative lambda__logs.log-cwl-plumbing-lambda-filter-rejected-when-invoke-permission-names-another-group logs.embedded-metric-format-event-extracts-only-the-declared-metric logs.malformed-embedded-metric-format-event-counts-an-emf-parsing-error logs.put-log-events-emits-awslogs-incoming-metrics-for-the-log-group cloudwatch__logs.emf-log-to-metric-extraction-exactness logs.list-aggregate-log-group-summaries-reflects-groups logs.list-aggregate-log-group-summaries-cross-account logs.log-group-listing-and-aggregate-summaries-visible
Issue-Key: events__logs|PutResourcePolicy|InvalidParameterException
Issue-Key: logs__opensearch|PutResourcePolicy|InvalidParameterException
Issue-Key: cloudwatch-logs|PutResourcePolicy|InvalidParameterException
Issue-Key: logs.associate-kms-key-invalid-key-arn-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: logs.associate-kms-key-malformed-arn-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: logs.associate-kms-key-without-service-permission-rejected|_error_with_retry#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: logs.kms-grant-scoped-to-another-log-group-rejected|_error_code#21|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded and returned {result!r}")
Issue-Key: logs.deep-logs-2-logcmk-disabled-key-associate-rejected|_error#15|AssertionError|raise AssertionError("call unexpectedly SUCCEEDED; real AWS rejects it")
Issue-Key: kms__logs.associate-kms-key-alias-versus-key-arn|_error#10|AssertionError|raise AssertionError("the call was expected to be rejected but SUCCEEDED")
Issue-Key: kms__logs.xsvc-logs-2-associate-disabled-cmk-rejected-until-enabled|_error#12|AssertionError|raise AssertionError("the call was expected to be rejected but SUCCEEDED")
Issue-Key: kms__logs.xsvc-logs-2-cmk-scoped-to-group-a-rejects-associate-on-group-b|_error#12|AssertionError|raise AssertionError("the call was expected to be rejected but SUCCEEDED")
Issue-Key: kms__logs.xsvc-logs-2-create-log-group-with-out-of-scope-cmk-rejected|_error#12|AssertionError|raise AssertionError("the call was expected to be rejected but SUCCEEDED")
Issue-Key: logs.access-denied-enforcement|run#28|AssertionError|assert not failures, (
Issue-Key: logs.get-log-object-access-denied-enforced|_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: logs.op-contract-describeLogGroups|_error#12|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: logs.put-log-events-batch-contract-rejected|_client_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: logs.deep-cwlogs-semantics-put-log-events-surviving-span-over-24h-rejected|_error_code#9|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch__lambda__logs.logs-subscription-filter-missing-permission-negative|run#10|AssertionError|raise AssertionError(
Issue-Key: lambda__logs.log-cwl-plumbing-lambda-filter-rejected-when-invoke-permission-names-another-group|run#13|AssertionError|raise AssertionError(
Issue-Key: logs.embedded-metric-format-event-extracts-only-the-declared-metric|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: logs.malformed-embedded-metric-format-event-counts-an-emf-parsing-error|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: logs.put-log-events-emits-awslogs-incoming-metrics-for-the-log-group|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__logs.emf-log-to-metric-extraction-exactness|_poll_until_stable#30|Timeout|time.sleep(interval_s)
Issue-Key: logs.list-aggregate-log-group-summaries-reflects-groups|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: logs.list-aggregate-log-group-summaries-cross-account|_poll_until#7|Timeout|time.sleep(interval)
Issue-Key: logs.log-group-listing-and-aggregate-summaries-visible|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# CloudWatch Logs accepts requests real AWS refuses: PutResourcePolicy demands policyName, KMS keys and grants are never validated, no IAM authorization, and ingestion/metric semantics are missing (8 causes)

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Eight net-new root causes in the CloudWatch Logs emulator, all about requests being accepted (or wrongly refused) where real AWS decides differently. ALL EIGHT must be closed before submitting.

Cause 1 of 8: PutResourcePolicy rejects a request that omits policyName.
AWS: Smithy (cloudwatchlogs) PutResourcePolicy input = {policyName: PolicyName, policyDocument (length 1..51200), resourceArn: Arn, expectedRevisionId}; NO member is marked required, and the resourceArn form (used for a log-group-scoped delivery policy) legitimately omits policyName. Modeled errors: InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceNotFoundException, ServiceUnavailableException (all 400 except the last).
Emulator: answers 400 InvalidParameterException with message 'policyName is required' and stores nothing.
Why: every service that writes a log-delivery resource policy (EventBridge bus logging, OpenSearch log publishing, scheduled queries) fails at setup, so cross-service log delivery cannot be configured at all.
Fix: PutResourcePolicy's input validation in the logs service crate - accept the resourceArn-scoped form with no policyName and key the stored policy accordingly.
Traces: events__logs.xsvc-logs-1-* (8 checks), logs__opensearch (4 checks), logs.op-contract-delete-resource-policy-stale-revision-rejected, logs.resource-policy-revision-conflict-rejected.

Cause 2 of 8: AssociateKmsKey / CreateLogGroup accept any kmsKeyId - invalid ARN, malformed ARN, a disabled CMK, or a CMK whose key policy/grant does not cover this log group.
AWS: Smithy (cloudwatchlogs) AssociateKmsKey: kmsKeyId is required (length 0..256) and the operation declares InvalidParameterException and ResourceNotFoundException (both 400). Real AWS resolves the key, requires the key policy to allow logs.{region}.amazonaws.com with an encryption context scoped to the log group ARN, and refuses a disabled key or a key scoped to another group.
Emulator: returns 200 with an empty body for all of those cases; the association is recorded unconditionally.
Why: an encryption misconfiguration that AWS refuses up front looks healthy in the emulator, so a client never learns its CMK policy is wrong; conversely reads that AWS blocks on a disabled key keep working.
Fix: AssociateKmsKey/CreateLogGroup validation plus the KMS-key resolution path in the logs service crate (ARN shape, key existence/enabled state, key-policy/grant scope check against the log group ARN).
Traces: logs.associate-kms-key-invalid-key-arn-rejected, logs.associate-kms-key-malformed-arn-rejected, logs.associate-kms-key-without-service-permission-rejected, logs.kms-grant-scoped-to-another-log-group-rejected, logs.deep-logs-2-logcmk-disabled-key-associate-rejected, kms__logs.associate-kms-key-alias-versus-key-arn, kms__logs.xsvc-logs-2-associate-disabled-cmk-rejected-until-enabled, kms__logs.xsvc-logs-2-cmk-scoped-to-group-a-rejects-associate-on-group-b, kms__logs.xsvc-logs-2-create-log-group-with-out-of-scope-cmk-rejected.

Cause 3 of 8: no IAM authorization on logs operations - a caller with zero permissions is served.
AWS: a sigv4 caller whose identity policy grants nothing is refused with 400 AccessDeniedException (code AccessDeniedException) on logs control and read operations; DescribeLogGroups does not return results to an unauthorized principal.
Emulator: DescribeLogGroups returns 200 {'logGroups': []} for a zero-permission role, and the sweep over the whole surface shows no operation answering AccessDeniedException (operations that are implemented answer success, stubs answer NotImplemented).
Why: an application's least-privilege policy cannot be validated; a deny that AWS enforces is invisible, so broken permissions ship.
Fix: the request authorization layer for the logs service crate (evaluate the caller's identity policy before dispatching the op).
Traces: logs.access-denied-enforcement, logs.get-log-object-access-denied-enforced.

Cause 4 of 8: DescribeLogGroups accepts logGroupIdentifiers together with logGroupNamePrefix.
AWS: those members are mutually exclusive and the call is refused with 400 InvalidParameterException. Smithy (cloudwatchlogs) DescribeLogGroups declares logGroupIdentifiers, logGroupNamePrefix and logGroupNamePattern with InvalidParameterException as its only client error.
Emulator: returns 200 with the matching log group.
Why: a client building a query from both filters gets silently different semantics from AWS instead of an error it can fix.
Fix: DescribeLogGroups input validation in the logs service crate.
Traces: logs.op-contract-describeLogGroups.

Cause 5 of 8: PutLogEvents does not enforce the batch contract.
AWS: PutLogEvents refuses a batch whose events span more than 24 hours (and the other batch-contract violations: out-of-order timestamps within a batch, >10000 events, >1 MiB) with 400 InvalidParameterException; it does not accept the batch and report the rejection in rejectedLogEventsInfo.
Emulator: returns 200 with nextSequenceToken and rejectedLogEventsInfo {'tooOldLogEventEndIndex': 1} for a >24h span, and 200 for the other refused batch shapes.
Why: an ingestion client that relies on AWS refusing a malformed batch silently loses events here, and the 200 masks a bug in the producer.
Fix: PutLogEvents request validation in the logs service crate, ahead of the append to stream state.
Traces: logs.put-log-events-batch-contract-rejected, logs.deep-cwlogs-semantics-put-log-events-surviving-span-over-24h-rejected.

Cause 6 of 8: PutSubscriptionFilter accepts a Lambda destination that has not granted logs.amazonaws.com permission to invoke it, or whose permission names a different log group.
AWS: PutSubscriptionFilter resolves the destinationArn and verifies the invoke grant; without a lambda:InvokeFunction permission for logs.amazonaws.com scoped to that log group it is refused with 400 InvalidParameterException.
Emulator: returns 200 and registers the filter.
Why: a subscription that can never deliver looks configured; the missing permission is only discovered as silent non-delivery.
Fix: PutSubscriptionFilter destination validation in the logs service crate (cross-check the destination's resource policy / Lambda permission and its source-ARN scope).
Traces: cloudwatch__lambda__logs.logs-subscription-filter-missing-permission-negative, lambda__logs.log-cwl-plumbing-lambda-filter-rejected-when-invoke-permission-names-another-group.

Cause 7 of 8: an Embedded Metric Format log event never materializes a CloudWatch metric.
AWS: a log event whose message is a valid EMF document causes CloudWatch Logs to extract the declared metrics and publish them to CloudWatch (and to count an EMF parsing error for a malformed document); PutLogEvents also publishes the AWS/Logs IncomingLogEvents/IncomingBytes metrics for the log group.
Emulator: recorded behavior only - the checks poll GetMetricStatistics/ListMetrics for the extracted datapoint and hit the 60s deadline with nothing published.
Why: the whole EMF-based metric path (the documented way to emit custom metrics from logs) produces no metric, so dashboards and alarms built on it cannot be exercised.
Fix: the PutLogEvents ingestion path in the logs service crate (EMF parse + metric emission into CloudWatch state), including the AWS/Logs ingestion metrics.
Traces: logs.embedded-metric-format-event-extracts-only-the-declared-metric, logs.malformed-embedded-metric-format-event-counts-an-emf-parsing-error, logs.put-log-events-emits-awslogs-incoming-metrics-for-the-log-group, cloudwatch__logs.emf-log-to-metric-extraction-exactness.

Cause 8 of 8: the aggregate log-group summary surface never reflects existing log groups.
AWS: after CreateLogGroup the group appears in the aggregate log-group summaries (ListLogGroups/aggregate summaries, including the linked-account view), bounded by normal propagation.
Emulator: recorded behavior only - the checks poll for the newly created group in the summaries until the 60s deadline and never see it.
Why: a console-style listing path returns an empty/stale view, so a client cannot enumerate its own log groups through that surface.
Fix: the aggregate log-group summary read path in the logs service crate, served from the same state CreateLogGroup writes.
Traces: logs.list-aggregate-log-group-summaries-reflects-groups, logs.list-aggregate-log-group-summaries-cross-account, logs.log-group-listing-and-aggregate-summaries-visible.

## Evidence
Cause 1: botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the PutResourcePolicy operation: policyName is required. Expected: 200 + resourcePolicy/revisionId (policyName is optional in the model).
Cause 2: AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (invalid/malformed key ARN, no service permission); 'the call was expected to be rejected but SUCCEEDED' (disabled CMK, out-of-scope CMK, alias vs key ARN). Expected: 400 InvalidParameterException (or ResourceNotFoundException for an unknown key).
Cause 3: AssertionError: operations that did NOT return an acceptable rejection code for a zero-permission caller: {'CreateImportTask': "wrong error code: 'NotImplemented' (expected one of ('AccessDeniedException', ...))", ... 'PutBearerTokenAuthentication': "wrong error code: 'ResourceNotFoundException'" ...}; and DescribeLogGroups returned 200 {'logGroups': []} where a ClientError was expected. Expected: 400 AccessDeniedException.
Cause 4: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'logGroups': [...]} for describe_log_groups(logGroupIdentifiers=[arn], logGroupNamePrefix=name). Expected: 400 InvalidParameterException.
Cause 5: AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'nextSequenceToken': '...', 'rejectedLogEventsInfo': {'tooOldLogEventEndIndex': 1}, 'HTTPStatusCode': 200}. Expected: 400 InvalidParameterException.
Cause 6: AssertionError: expected PutSubscriptionFilter to be rejected without a logs.amazonaws.com invoke permission grant on the destination Lambda, but it succeeded; AssertionError: expected PutSubscriptionFilter on <group-b> to be refused: the function's invoke permission names only <group-a>. Expected: 400 InvalidParameterException.
Cause 7 and 8: Error: Check timed out after 60.0s while polling for the extracted metric datapoint / for the created group in the aggregate summaries.

## Steps to Reproduce
Cause 1: logs.put_resource_policy(policyDocument=<doc>, resourceArn='arn:aws:logs:...:log-group:<g>') with no policyName -> InvalidParameterException 'policyName is required'.
Cause 2: CreateLogGroup, then AssociateKmsKey with kmsKeyId='not-an-arn', with a disabled CMK, and with a CMK whose key policy grants nothing to logs -> all return 200.
Cause 3: assume a role with an empty/deny-all identity policy and call DescribeLogGroups -> 200 with an empty list instead of AccessDeniedException.
Cause 4: describe_log_groups(logGroupIdentifiers=[<arn>], logGroupNamePrefix=<name>) -> 200.
Cause 5: PutLogEvents with two events whose timestamps are >24h apart -> 200.
Cause 6: create a Lambda with no logs.amazonaws.com invoke permission, then PutSubscriptionFilter to it -> 200.
Cause 7: PutLogEvents with a valid EMF JSON message, then poll GetMetricStatistics for the declared metric -> no datapoint.
Cause 8: CreateLogGroup, then poll the aggregate log-group summaries -> group absent.
