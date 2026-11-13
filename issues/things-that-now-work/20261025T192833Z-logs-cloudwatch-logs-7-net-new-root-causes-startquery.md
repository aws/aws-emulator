Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-1-logsql-cwli-query-string-as-sql-rejected logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected logs.deep-logs-1-logsql-sql-ppl-across-standard-and-infrequent-access-rejected logs.logs-insights-semantics-syntax-error-rejected-at-start logs.e2e-export-task-produces-s3-object-with-log-content logs.export-task-time-range-window-respected logs.embedded-metric-format-event-extracts-only-the-declared-metric logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group logs.delivery-destination-delete-blocked-by-dependent-delivery-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-1-logsql-cwli-query-string-as-sql-rejected|_start_error#13|AssertionError|raise AssertionError(f"StartQuery accepted a request real AWS refuses: {kwargs!r} -> {resp!r}")
Issue-Key: logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected|_start_error#13|AssertionError|raise AssertionError(f"StartQuery accepted a request real AWS refuses: {kwargs!r} -> {resp!r}")
Issue-Key: logs.deep-logs-1-logsql-sql-ppl-across-standard-and-infrequent-access-rejected|_start_error#17|AssertionError|raise AssertionError(f"StartQuery accepted a request real AWS refuses: {kwargs!r} -> {resp!r}")
Issue-Key: logs.logs-insights-semantics-syntax-error-rejected-at-start|run#16|AssertionError|assert m1, f"unknown command: message must name the token 'frobnicate', got {msg1!r}"
Issue-Key: logs.e2e-export-task-produces-s3-object-with-log-content|_download_prefix_text#14|Timeout|time.sleep(3.0)
Issue-Key: logs.export-task-time-range-window-respected|_create_export_task_with_retry#28|Timeout|time.sleep(3.0)
Issue-Key: logs.embedded-metric-format-event-extracts-only-the-declared-metric|run#33|Timeout|time.sleep(10.0)
Issue-Key: logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group|run#31|Timeout|time.sleep(5.0)
Issue-Key: logs.delivery-destination-delete-blocked-by-dependent-delivery-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# CloudWatch Logs: 7 net-new root causes — StartQuery accepts three query classes AWS refuses and reports syntax errors with the model's own doc text, export tasks never run, EMF emits no metric, DeleteDeliveryDestination's conflict message is invented

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Seven distinct net-new root causes in the CloudWatch Logs surface, each grounded in failing behavioral checks whose check id leads with `logs`. None of these checks is marked ALREADY FILED or OWNED in the replay report.

----------------------------------------------------------------
Cause 1 of 7: StartQuery accepts a CWLI query string submitted with queryLanguage="SQL".
Failing check: logs.deep-logs-1-logsql-cwli-query-string-as-sql-rejected

AWS: Smithy: StartQuery.queryLanguage is enum QueryLanguage [CWLI, SQL, PPL] and queryString is an opaque @required string, so nothing in the model ties the two; the operation models InvalidParameterException (400, "A parameter is specified incorrectly."). Recorded behavior: a Logs-Insights (CWLI) pipeline string submitted with queryLanguage="SQL" is REFUSED 400 InvalidParameterException with the fixed message "Log group tag queries are not supported" (fixed text, no request-derived parts), and no query is recorded — DescribeQueries lists nothing for it.
Emulator: the call is ACCEPTED — 200 with {"queryId": "00000000-0000-4000-8000-000000000001"} — for queryString "fields @timestamp, @message | filter @message like /<marker>/ | sort @timestamp asc" under queryLanguage="SQL".
Why: a client that mislabels its dialect gets a running query instead of a refusal, so a bug that AWS surfaces immediately at StartQuery becomes a silently wrong (or empty) result set, and the refused query also leaves a visible query behind in DescribeQueries where AWS leaves none.
Fix: dialect validation in the StartQuery handler of the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs): the queryString must be compiled in the dialect named by queryLanguage, and a dialect mismatch refused before any query state is created.
Traces: logs.deep-logs-1-logsql-cwli-query-string-as-sql-rejected.

----------------------------------------------------------------
Cause 2 of 7: StartQuery accepts a SQL query whose HAVING clause filters on an aggregate function expression.
Failing check: logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected

AWS: Smithy: StartQuery models MalformedQueryException (400); its model doc is "The query string is not valid. Details about this error are displayed in a QueryCompileError object." Recorded behavior: SQL of the form `SELECT svc, COUNT(*) AS n, SUM(ms) AS total FROM `<group>` WHERE ... GROUP BY svc HAVING COUNT(*) > 1` is REFUSED 400 MalformedQueryException whose message names the rule and the offending translated expression with its span — fixed text "No aggregation function expression allowed: ", then the translated predicate (e.g. `filtercount(*)>1`, `filtersum(ms)>50`) and its character span in the translated pipeline, wrapped in the AWS Java-SDK envelope "(Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <id>; Proxy: null)". The alias forms (`HAVING n > 1`, `HAVING total > 50`) ARE accepted and return rows — so the refusal is specific to the aggregate expression in the filter.
Emulator: both HAVING-with-aggregate variants are ACCEPTED — 200 with a queryId.
Why: the whole SQL-over-logs compile contract is unenforced. A client writing `HAVING COUNT(*) > 1` is told its query is fine and then gets results AWS would never have produced, with no way to learn that it must use the alias form.
Fix: SQL compilation/validation in the StartQuery handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs) — reject an aggregate function expression inside HAVING with MalformedQueryException and AWS's wording, the offending expression and span taken from the submitted query.
Traces: logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected.

----------------------------------------------------------------
Cause 3 of 7: StartQuery accepts a SQL or PPL query spanning a STANDARD and an INFREQUENT_ACCESS log group.
Failing check: logs.deep-logs-1-logsql-sql-ppl-across-standard-and-infrequent-access-rejected

AWS: Smithy: the log class is a property of the log group (CreateLogGroup.logGroupClass), invisible in StartQuery's input shape; StartQuery models InvalidParameterException (400). Recorded behavior: naming one STANDARD and one INFREQUENT_ACCESS group together — in SQL via FROM `logGroups(logGroupIdentifier: ['<std>','<ia>'])`, or in PPL via logGroupNames=[<std>,<ia>] — is REFUSED 400 InvalidParameterException with the fixed message "Querying is supported only in the same log class." in the Java-SDK envelope "(Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <id>; Proxy: null)", and nothing is recorded in DescribeQueries. Either group ALONE is accepted (including the INFREQUENT_ACCESS one), so the refusal is the class mix.
Emulator: both the SQL and the PPL mixed-class forms are ACCEPTED — 200 with a queryId.
Why: a client that mixes classes gets a query that runs instead of the refusal that tells it to split the query by log class; results computed across a class boundary cannot be reproduced on AWS.
Fix: cross-group admission in the StartQuery handler and the log-group state it resolves against (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs): resolve every named group's logGroupClass and refuse a heterogeneous set with InvalidParameterException and AWS's fixed text.
Traces: logs.deep-logs-1-logsql-sql-ppl-across-standard-and-infrequent-access-rejected.

----------------------------------------------------------------
Cause 4 of 7: StartQuery's MalformedQueryException message is the shape's documentation sentence instead of the parser's token-and-position wording.
Failing check: logs.logs-insights-semantics-syntax-error-rejected-at-start

AWS: Smithy: MalformedQueryException (400) carries `message` and `queryCompileError`. Recorded behavior: a syntactically invalid CWLI query is refused at StartQuery with a message naming the offending token and its position — fixed text "unexpected symbol found ", then the offending token taken from the request (`frobnicate` for an unknown command; `<EOF>` for a pipeline that ends where an argument is required, e.g. a trailing `| sort`), then " at line 1 and position ", then the position, which tracks the token's own offset in the submitted string (the two refusals' positions differ by exactly as many characters as the token moved). The error code already matches; only the message diverges.
Emulator: 400 MalformedQueryException with message "The query string is not valid." for every syntax fault — verbatim the first sentence of MalformedQueryException's model doc, naming neither token nor position, and identical for two different faults.
Why: the message is the only channel that tells a client WHERE its query broke. One constant sentence for every syntax error makes an invalid query undiagnosable and collapses two distinct faults into one indistinguishable response.
Fix: the MalformedQueryException message construction (and queryCompileError population) in the queryString parser behind the StartQuery handler, cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs). The token and the position must come from the submitted query string, not from a fixed string.
Traces: logs.logs-insights-semantics-syntax-error-rejected-at-start.

----------------------------------------------------------------
Cause 5 of 7: Export tasks never run — a created task never reaches COMPLETED and no object is ever written to the destination bucket.
Failing checks: logs.e2e-export-task-produces-s3-object-with-log-content, logs.export-task-time-range-window-respected

AWS: Smithy: CreateExportTask returns a taskId and DescribeExportTasks reports status.code from enum ExportTaskStatusCode [CANCELLED, COMPLETED, FAILED, PENDING, PENDING_CANCEL, RUNNING]. Recorded behavior: an accepted export task advances PENDING/RUNNING -> COMPLETED (the checks budget up to 600 s and 240 s respectively because real AWS can take minutes) and the gzipped objects holding the in-window events appear under the requested destinationPrefix in the destination bucket.
Emulator: both checks hang in their DescribeExportTasks poll and are killed at the harness cap — "Error: Check timed out after 60.0s". No terminal status is ever observed and no exported object appears, so the emulator never answers the question either way.
Why: the export data plane is unobservable and any client that waits for an export to finish hangs forever instead of getting COMPLETED (or FAILED). Nothing downstream of an export — object layout, prefix, per-stream content, window filtering — can be verified.
Fix: export-task execution and status transition in the cloudwatch-logs crate — the task created by services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs must be driven to a terminal ExportTaskStatusCode in export-task state and write its objects to the destination bucket, and services/.../src/ops/describe_export_tasks.rs must report that progression. Related open issue that covers the same symptom under a DIFFERENT service token (logs__s3) and does not carry these two `logs` replay keys: 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md.
Traces: the two check ids above.

----------------------------------------------------------------
Cause 6 of 7: Embedded-Metric-Format log events produce no CloudWatch metric datapoint.
Failing checks: logs.embedded-metric-format-event-extracts-only-the-declared-metric, logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group

AWS: recorded behavior only — the Smithy model types PutLogEvents' message as an opaque string and PutAccountPolicy.policyDocument as an opaque document, and says nothing about EMF. AWS documents that "no setup is required to use the embedded metric format": a PutLogEvents message that is an EMF document publishes the metrics named in _aws.CloudWatchMetrics[].Metrics into the declared namespace with no metric filter and no PutMetricData, with the declared Unit attached, readable through GetMetricData/GetMetricStatistics; a numeric field NOT named in Metrics produces nothing; and an account-level METRIC_EXTRACTION_POLICY {"EmbeddedMetricFormat":{"Status":"Disabled"}} scoped by `LogGroupName IN ["<g>"]` suppresses extraction for exactly that group while a sibling group keeps producing datapoints. The raw events stay readable verbatim.
Emulator: both checks hang in the post-ingest datapoint poll and are killed at the harness cap — "Error: Check timed out after 60.0s". No EMF-derived datapoint ever appears in the run's own namespace, so neither extraction, nor its selective suppression, nor its resumption after DeleteAccountPolicy can be observed.
Why: an emulator that extracts nothing is indistinguishable from one whose suppression policy is working, so EMF — the zero-configuration path most application code uses to emit metrics — silently produces no telemetry, and any consumer waiting on an EMF-derived metric hangs.
Fix: EMF parsing on the PutLogEvents ingest path and the metric emission it drives, plus METRIC_EXTRACTION_POLICY scope evaluation in the account-policy state (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_log_events.rs and the account-policy state module). Note: 20261025T070938Z-logs-cloudwatch-logs-6-net-new-root-causes-kinesis.md (status fixed) claimed this cause and carries logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group as a replay key, but that check still times out, so the behavior was not actually delivered; the open issues 20261025T020208Z-cloudwatch-logs-emf-log-events-and-metric-extraction-policy-produce.md and 20261025T174025Z-cloudwatch-logs-emf-documents-and-metric-extraction-policy.md cover the same symptom under the cloudwatch__logs token and do not carry these two `logs` replay keys.
Traces: the two check ids above.

----------------------------------------------------------------
Cause 7 of 7: DeleteDeliveryDestination's in-use refusal carries an emulator-authored message.
Failing check: logs.delivery-destination-delete-blocked-by-dependent-delivery-rejected

AWS: Smithy: DeleteDeliveryDestination.name is @required (@length 1..60, pattern ^[\w-]*$) and the operation models ConflictException (400), ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException, ValidationException. Recorded behavior: while a Delivery still references the destination, the delete is refused 400 ConflictException with the fixed message "Could not delete this Delivery Destination as it is currently in use. Please delete any Deliveries that reference this resource and try again." — entirely fixed text, naming neither the destination nor the delivery. The error code already matches; only the message diverges.
Emulator: 400 ConflictException with message "The delivery destination <name> cannot be deleted while delivery <id> is associated with it." — emulator-authored text that interpolates the destination name and the delivery id.
Why: the message is part of the error contract; a client matching or surfacing AWS's wording sees text that exists nowhere in AWS, and the invented sentence omits the corrective instruction AWS gives (delete the referencing Deliveries first).
Fix: the ConflictException message in the dependency check of the DeleteDeliveryDestination handler, cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_delivery_destination.rs) — emit AWS's fixed sentence, with no request- or state-derived interpolation.
Traces: logs.delivery-destination-delete-blocked-by-dependent-delivery-rejected.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 124/432 passed; service `logs` 113/244):

Cause 1 — [mismatch] logs.deep-logs-1-logsql-cwli-query-string-as-sql-rejected
  AssertionError: StartQuery accepted a request real AWS refuses: {'queryLanguage': 'SQL', 'queryString': 'fields @timestamp, @message | filter @message like /<marker>/ | sort @timestamp asc', 'startTime': <t0>, 'endTime': <t1>} -> {'queryId': '00000000-0000-4000-8000-000000000001', HTTPStatusCode: 200}
  Expected: 400 InvalidParameterException, message "Log group tag queries are not supported", and no query recorded in DescribeQueries.
  Actual:   200 with a queryId.

Cause 2 — [mismatch] logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected
  AssertionError: StartQuery accepted a request real AWS refuses: {'queryLanguage': 'SQL', 'queryString': "SELECT svc, COUNT(*) AS n, SUM(ms) AS total FROM `<group>` WHERE run = '<marker>' GROUP BY svc HAVING COUNT(*) > 1", ...} -> {'queryId': '00000000-0000-4000-8000-000000000001', HTTPStatusCode: 200}
  Expected: 400 MalformedQueryException, message "No aggregation function expression allowed: <translated-predicate> ([<span>])" in the "(Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <id>; Proxy: null)" envelope — for the two variants the translated predicates are `filtercount(*)>1` and `filtersum(ms)>50`.
  Actual:   200 with a queryId for both variants.

Cause 3 — [mismatch] logs.deep-logs-1-logsql-sql-ppl-across-standard-and-infrequent-access-rejected
  AssertionError: StartQuery accepted a request real AWS refuses: {'queryLanguage': 'SQL', 'queryString': "SELECT COUNT(*) AS n FROM `logGroups(logGroupIdentifier: ['<std-group>', '<ia-group>'])` WHERE run = '<marker>'", ...} -> {'queryId': '00000000-0000-4000-8000-000000000001', HTTPStatusCode: 200}
  Same for the PPL form: StartQuery(queryLanguage="PPL", logGroupNames=[<std>, <ia>], queryString="where run = '<marker>' | stats count() as n").
  Expected: 400 InvalidParameterException, message "Querying is supported only in the same log class." (Java-SDK envelope appended).
  Actual:   200 with a queryId.

Cause 4 — [mismatch] logs.logs-insights-semantics-syntax-error-rejected-at-start
  AssertionError: unknown command: message must name the token 'frobnicate', got 'The query string is not valid.'
  Requests: StartQuery(logGroupNames=[<g>], queryString="fields @message | filter @message like '<marker>' | frobnicate x") and the same string ending "| sort".
  Expected: 400 MalformedQueryException, messages matching "unexpected symbol found frobnicate at line 1 and position <n>" and "unexpected symbol found <EOF> at line 1 and position <m>", where m - n equals the token's own displacement in the submitted strings.
  Actual:   400 MalformedQueryException, message "The query string is not valid." for both.

Cause 5 — export tasks never run (2 checks, verdict [mismatch])
  logs.e2e-export-task-produces-s3-object-with-log-content — Error: Check timed out after 60.0s
  logs.export-task-time-range-window-respected      — Error: Check timed out after 60.0s
  Both spin in a DescribeExportTasks(taskId=<id>) poll waiting for status.code in {COMPLETED, FAILED, CANCELLED}.
  Expected: the task reaches COMPLETED and the gzipped export objects appear under the requested prefix in the destination bucket.
  Actual:   no terminal status and no object within the harness's 60 s cap.

Cause 6 — EMF extraction emits no metric data (2 checks, verdict [mismatch])
  logs.embedded-metric-format-event-extracts-only-the-declared-metric — Error: Check timed out after 60.0s
  logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group — Error: Check timed out after 60.0s
  Both spin polling cloudwatch for a datapoint in the run's own namespace after PutLogEvents of an EMF document.
  Expected: a datapoint for the metric declared in _aws.CloudWatchMetrics (value as emitted, declared Unit), with no metric filter and no PutMetricData; the undeclared numeric field produces none; a METRIC_EXTRACTION_POLICY scoped to one group suppresses only that group.
  Actual:   no datapoint ever appears; both checks run to the cap.

Cause 7 — [mismatch] logs.delivery-destination-delete-blocked-by-dependent-delivery-rejected
  AssertionError: expected message 'Could not delete this Delivery Destination as it is currently in use. Please delete any Deliveries that reference this resource and try again.', got 'The delivery destination sim2real-ddst-e92fc7bb62fd cannot be deleted while delivery 0000000000000000 is associated with it.'
  Request: DeleteDeliveryDestination(name=<dest>) while CreateDelivery(deliverySourceName=<src>, deliveryDestinationArn=<dest-arn>) is still in place.
  Expected: 400 ConflictException with AWS's fixed sentence above (no interpolated names/ids).
  Actual:   400 ConflictException with an emulator-authored sentence naming the destination and the delivery id (the name and id quoted here are this run's values, not part of the requirement).

## Steps to Reproduce
Boot the emulator from this checkout and replay the nine `logs` checks in trace_ids.

Cause 1: logs.CreateLogGroup + CreateLogStream + PutLogEvents; then
  logs.StartQuery(queryLanguage="SQL", queryString="fields @timestamp, @message | filter @message like /<marker>/ | sort @timestamp asc", startTime=<t0>, endTime=<t1>)
  -> expect 400 InvalidParameterException "Log group tag queries are not supported"; observe 200 + queryId. Then DescribeQueries -> expect no query for that string.

Cause 2: same group with JSON events; then
  logs.StartQuery(queryLanguage="SQL", queryString="SELECT svc, COUNT(*) AS n, SUM(ms) AS total FROM `<g>` WHERE run = '<marker>' GROUP BY svc HAVING COUNT(*) > 1")
  -> expect 400 MalformedQueryException naming the aggregate-in-filter rule; observe 200 + queryId. Repeat with "HAVING SUM(ms) > 50". Control: "HAVING n > 1" and "HAVING total > 50" must be accepted and return rows.

Cause 3: logs.CreateLogGroup(<std>) and logs.CreateLogGroup(<ia>, logGroupClass="INFREQUENT_ACCESS"), plant events in both; then
  logs.StartQuery(queryLanguage="SQL", queryString="SELECT COUNT(*) AS n FROM `logGroups(logGroupIdentifier: ['<std>', '<ia>'])` WHERE run = '<marker>'")
  and logs.StartQuery(queryLanguage="PPL", logGroupNames=[<std>, <ia>], queryString="where run = '<marker>' | stats count() as n")
  -> expect 400 InvalidParameterException "Querying is supported only in the same log class." for both; observe 200 + queryId. Control: either group alone is accepted.

Cause 4: logs.CreateLogGroup + a planted event; then
  logs.StartQuery(logGroupNames=[<g>], queryString="fields @message | filter @message like '<marker>' | frobnicate x")
  -> expect 400 MalformedQueryException "unexpected symbol found frobnicate at line 1 and position <n>"; observe "The query string is not valid."
  Repeat with a trailing "| sort" -> expect "unexpected symbol found <EOF> at line 1 and position <m>".

Cause 5: logs.CreateLogGroup + CreateLogStream + PutLogEvents; s3.CreateBucket (+ policy for logs.<region>.amazonaws.com); logs.CreateExportTask(logGroupName=<g>, destination=<bucket>, destinationPrefix=<p>, from=<t0>, to=<t1>); then poll logs.DescribeExportTasks(taskId=<id>)
  -> expect status.code to reach COMPLETED and s3.ListObjectsV2(Bucket=<bucket>, Prefix=<p>) to return the gzipped export; observe the status never leaves its initial value and no object appears.

Cause 6: logs.CreateLogGroup + CreateLogStream; logs.PutLogEvents with one message that is an EMF document declaring one metric (namespace N, Unit "Milliseconds", one dimension) plus one numeric field NOT named in Metrics; then poll cloudwatch.GetMetricData for namespace N
  -> expect a datapoint for the declared metric with the emitted value and declared Unit, none for the undeclared field; observe no datapoint at all. Then logs.PutAccountPolicy(policyType="METRIC_EXTRACTION_POLICY", policyDocument='{"EmbeddedMetricFormat":{"Status":"Disabled"}}', selectionCriteria='LogGroupName IN ["<off-group>"]') and verify only that group stops producing datapoints.

Cause 7: s3.CreateBucket; logs.PutDeliverySource(name=<src>, resourceArn=<bucket-arn>, logType="S3_SERVER_ACCESS_LOGS"); logs.CreateLogGroup; logs.PutDeliveryDestination(name=<dest>, deliveryDestinationType="CWL", deliveryDestinationConfiguration={destinationResourceArn:<log-group-arn>}); logs.CreateDelivery(deliverySourceName=<src>, deliveryDestinationArn=<dest-arn>); then
  logs.DeleteDeliveryDestination(name=<dest>)
  -> expect 400 ConflictException "Could not delete this Delivery Destination as it is currently in use. Please delete any Deliveries that reference this resource and try again."; observe the emulator's invented sentence naming the destination and delivery id.
