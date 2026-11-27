Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.logs-insights-semantics-unknown-log-group-rejected-at-start logs.logs-insights-semantics-both-log-group-selectors-rejected logs.logs-insights-semantics-semantic-error-rejected-at-start logs.deep-logs-1-logsql-sql-with-log-group-name-parameter-rejected logs.lookup-table-duplicate-name-rejected logs.cancel-import-task-invalid-state-fails logs.put-index-policy-limit-exceeded-fails logs.test-metric-filter-invalid-pattern-fails logs.put-destination-nonexistent-target-rejected logs.put-subscription-filter-bogus-destination-rejected logs.s3-table-integration-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.logs-insights-semantics-unknown-log-group-rejected-at-start|run#14|AssertionError|assert f"Log group '{missing}' does not exist" in msg, f"message must name the missing group {missing!r}: {msg!r}"
Issue-Key: logs.logs-insights-semantics-both-log-group-selectors-rejected|run#14|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: logs.logs-insights-semantics-semantic-error-rejected-at-start|_start_error#12|AssertionError|raise AssertionError(f"StartQuery accepted a query real AWS refuses: {kwargs.get('queryString')!r} -> {resp!r}")
Issue-Key: logs.deep-logs-1-logsql-sql-with-log-group-name-parameter-rejected|_start_error#13|AssertionError|raise AssertionError(f"StartQuery accepted a request real AWS refuses: {kwargs!r} -> {resp!r}")
Issue-Key: logs.lookup-table-duplicate-name-rejected|run#9|AssertionError|assert code_msg == expected, f"expected message {expected!r}, got {code_msg!r}"
Issue-Key: logs.cancel-import-task-invalid-state-fails|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: logs.put-index-policy-limit-exceeded-fails|run#15|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: logs.test-metric-filter-invalid-pattern-fails|run#4|AssertionError|assert control["matches"], f"expected the positive control to match, got {control['matches']}"
Issue-Key: logs.put-destination-nonexistent-target-rejected|_error#24|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: logs.put-subscription-filter-bogus-destination-rejected|_client_error#25|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: logs.s3-table-integration-lifecycle|_error_code#22|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# logs: StartQuery and eight other logs operations accept or misclassify calls real AWS refuses, and messages omit the resource (10 causes)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Ten independent root causes in the logs service. Cause count: 10. Each has its own failing check ids; all ten must be closed.

=== Cause 1 of 10: StartQuery's unknown-log-group refusal does not name the group ===
AWS: StartQuery naming a log group that does not exist is refused with ResourceNotFoundException whose message names the missing group (fixed text around one variable part: the logGroupName from the request).
Emulator: ResourceNotFoundException 'The specified log group does not exist.' — the identifier is dropped; replay: "message must name the missing group '<group>': 'The specified log group does not exist.'"
Why: a query over several selectors cannot be diagnosed — the caller cannot tell which group was missing.
Fix: the log-group resolution branch of the StartQuery handler.
Traces: logs.logs-insights-semantics-unknown-log-group-rejected-at-start.

=== Cause 2 of 10: the mutually-exclusive-selector refusal is worded by the emulator ===
AWS: StartQuery with both logGroupName and logGroupNames is refused with InvalidParameterException and the fixed text 'Both logGroupName and logGroupNames cannot be specified at the same time' (no variable parts).
Emulator: 'Exactly one of logGroupName, logGroupNames, logGroupIdentifiers must be specified.'
Why: the message names a different rule (exactly-one over three members) than the pair AWS complains about, so a client cannot repair the request from it.
Fix: the selector-validation branch of the StartQuery handler.
Traces: logs.logs-insights-semantics-both-log-group-selectors-rejected.

=== Cause 3 of 10: StartQuery accepts query strings real AWS refuses ===
AWS: StartQuery validates the query before returning a queryId: an unknown function in a CWLI stats clause ('fields ms | stats bogusfn(ms) as z') is refused with a MalformedQueryException/InvalidParameterException at start, and a queryLanguage=SQL request that also carries the logGroupName parameter is refused (SQL names its sources inside the statement). Recorded behavior only for the exact texts.
Emulator: both are accepted with HTTP 200 and a queryId ('00000000-0000-4000-8000-000000000001'), so the invalid query enters the system.
Why: a client gets a queryId for a query that can never produce results, and the validation contract that AWS enforces synchronously is skipped — errors surface (if at all) only later.
Fix: the query-string validation stage of the StartQuery handler (semantic validation for CWLI, and parameter-compatibility validation for queryLanguage=SQL). The open issue 20261025T192833Z-logs-cloudwatch-logs-7-net-new-root-causes-startquery.md covers other StartQuery acceptances; these two replay keys are not in it.
Traces: logs.logs-insights-semantics-semantic-error-rejected-at-start, logs.deep-logs-1-logsql-sql-with-log-group-name-parameter-rejected.

=== Cause 4 of 10: the duplicate lookup-table refusal does not name the table or account ===
AWS: CreateLookupTable under an existing name is refused with the fixed text "Lookup table '<name>' already exists in account '<accountId>'" — two variable parts, the requested name and the account id.
Emulator: 'The specified lookup table already exists'.
Why: the conflicting name is unrecoverable from the error, and cross-account diagnosis is lost.
Fix: the name-collision branch of the CreateLookupTable handler.
Traces: logs.lookup-table-duplicate-name-rejected.

=== Cause 5 of 10: CancelImportTask answers NotImplemented where AWS refuses with ResourceNotFoundException ===
AWS: CancelImportTask for an import task that does not exist (or is in a non-cancellable state) is refused with ResourceNotFoundException, HTTP 400.
Emulator: HTTP 501-class 'NotImplemented' — 'expected ResourceNotFoundException, got NotImplemented'.
Why: the operation is unusable and the refusal is unclassifiable: a client cannot distinguish a missing task from an unimplemented API.
Fix: implement the CancelImportTask handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/cancel_import_task.rs), including its not-found/invalid-state refusals.
Traces: logs.cancel-import-task-invalid-state-fails.

=== Cause 6 of 10: PutIndexPolicy over the per-policy field quota is refused with the wrong error class ===
AWS: a field-index policy naming more fields than the documented 20-per-policy quota is refused with InvalidParameterException (the request is malformed for the operation), not with LimitExceededException (which AWS reserves for account/resource counts).
Emulator: LimitExceededException — 'expected InvalidParameterException for a 300-field index policy (the documented quota is 20 fields per policy), got LimitExceededException'.
Why: LimitExceededException is throttling/quota-shaped and SDK retry policies treat the two classes differently, so a permanently malformed request can be retried forever.
Fix: the policyDocument field-count validation branch of the PutIndexPolicy handler — classify as InvalidParameterException.
Traces: logs.put-index-policy-limit-exceeded-fails.

=== Cause 7 of 10: TestMetricFilter reports no match for a pattern that does match ===
AWS: TestMetricFilter returns one matches entry per supplied log event the filterPattern matches (with eventNumber, eventMessage and extractedValues); the check's positive control is a pattern that must match its sample lines.
Emulator: the positive control returns an empty list — 'expected the positive control to match, got []'.
Why: the operation exists to let a client validate a pattern before creating a filter; always returning no match makes it worse than useless — it reports every valid pattern as non-matching.
Fix: the pattern-evaluation stage of the TestMetricFilter handler (the filter-pattern matcher it shares with metric-filter evaluation). Note the scaffold-stub issue 20261023T113357Z-logs-testmetricfilter-stub.md is marked fixed; the operation now answers, but its matcher returns nothing.
Traces: logs.test-metric-filter-invalid-pattern-fails.

=== Cause 8 of 10: PutDestination accepts a targetArn that does not exist ===
AWS: PutDestination validates the target synchronously — a Kinesis stream ARN with no such stream is refused with InvalidParameterException and no destination is created.
Emulator: HTTP 200 with the full destination object (destinationName, targetArn, roleArn, arn, creationTime) — 'expected a rejection, but the call SUCCEEDED'.
Why: a client gets a destination that can never deliver, and the cross-account subscription it anchors fails silently later instead of at configuration time.
Fix: the target-resolution stage of the PutDestination handler, before the destination is stored.
Traces: logs.put-destination-nonexistent-target-rejected.

=== Cause 9 of 10: PutSubscriptionFilter accepts a bogus destinationArn ===
AWS: PutSubscriptionFilter validates the destination synchronously (delivering a test record for a same-account Kinesis destination) and refuses an ARN that resolves to nothing with InvalidParameterException; no filter is created.
Emulator: HTTP 200 with an empty body and the filter is created — 'expected the call to be rejected, but it SUCCEEDED'.
Why: the filter looks configured and delivers nothing; the configuration-time validation AWS guarantees is gone.
Fix: the destination-validation stage of the PutSubscriptionFilter handler.
Traces: logs.put-subscription-filter-bogus-destination-rejected.

=== Cause 10 of 10: the S3-table-integration lifecycle accepts a call AWS refuses ===
AWS: the S3 table integration lifecycle refuses the invalid step the check exercises (HTTP 400, a client-error code) and creates no integration identifier.
Emulator: HTTP 200 with {'identifier': '00000000-0000-4000-8000-000000000002'} — 'expected this call to be rejected, but it SUCCEEDED'.
Why: an invalid integration is created and handed back an identifier, so subsequent lifecycle state diverges from AWS from that point on.
Fix: the validation stage of the S3-table-integration handler (PutIntegration/AssociateSourceToS3TableIntegration path) before the identifier is minted.
Traces: logs.s3-table-integration-lifecycle.

## Evidence
Replay quotes (per-run ids and names inside them are not part of the requirement):
Cause 1: "message must name the missing group 'sim2real-lisem-nosuchgrp-03c93c027ec2': 'The specified log group does not exist.'"
Cause 2: "expected message 'Both logGroupName and logGroupNames cannot be specified at the same time', got 'Exactly one of logGroupName, logGroupNames, logGroupIdentifiers must be specified.'"
Cause 3: "StartQuery accepted a query real AWS refuses: 'fields ms | stats bogusfn(ms) as z' -> {'queryId': '00000000-0000-4000-8000-000000000001', ... HTTPStatusCode: 200}" and "StartQuery accepted a request real AWS refuses: {'queryLanguage': 'SQL', 'queryString': \"SELECT svc, COUNT(*) ... FROM `<group>` ...\", 'logGroupName': '<group>', ...} -> {'queryId': ...}".
Cause 4: "expected message \"Lookup table '<name>' already exists in account '<account>'\", got 'The specified lookup table already exists'".
Cause 5: "expected ResourceNotFoundException, got NotImplemented".
Cause 6: "expected InvalidParameterException for a 300-field index policy (the documented quota is 20 fields per policy), got 'LimitExceededException'".
Cause 7: "expected the positive control to match, got []".
Cause 8: "expected a rejection, but the call SUCCEEDED and returned {'destination': {'destinationName': ..., 'targetArn': 'arn:aws:kinesis:...:stream/<never-created>', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}".
Cause 9: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'content-length': '2' ...}}".
Cause 10: "expected this call to be rejected, but it SUCCEEDED and returned {'identifier': '00000000-0000-4000-8000-000000000002', 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}".

## Steps to Reproduce
Cause 1: logs StartQuery logGroupName=<nonexistent> queryString='fields @timestamp' startTime/endTime valid. Compare the message with AWS's, which names the group.
Cause 2: StartQuery with BOTH logGroupName and logGroupNames set. Compare the message.
Cause 3: (a) StartQuery queryString='fields ms | stats bogusfn(ms) as z' over an existing group — expect a refusal at start, observe 200 + queryId; (b) StartQuery queryLanguage=SQL with a SELECT ... FROM `<group>` statement AND the logGroupName parameter — expect a refusal, observe 200 + queryId.
Cause 4: CreateLookupTable twice under the same name; compare the conflict message.
Cause 5: CancelImportTask taskId=<nonexistent>. Observe NotImplemented; expect ResourceNotFoundException.
Cause 6: PutIndexPolicy with a policyDocument naming 300 fields. Observe LimitExceededException; expect InvalidParameterException.
Cause 7: TestMetricFilter filterPattern=<a pattern that matches> logEventMessages=[<matching lines>]. Observe matches=[]; expect one entry per matching line.
Cause 8: PutDestination destinationName=<new> targetArn=arn:aws:kinesis:<region>:<account>:stream/<never created> roleArn=<role>. Observe 200; expect InvalidParameterException and no destination.
Cause 9: PutSubscriptionFilter with a destinationArn that resolves to nothing. Observe 200; expect InvalidParameterException and no filter.
Cause 10: drive the S3-table-integration lifecycle's invalid step (the create/associate call the check expects to be refused). Observe 200 with an identifier; expect a 400 client error and no identifier.
