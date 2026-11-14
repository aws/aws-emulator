Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.export-task-time-range-window-respected logs.export-task-honors-time-window-boundaries logs.logsquery-window-ending-before-group-creation-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.export-task-time-range-window-respected|run#33|AssertionError|assert f"INSIDE-ONE-{marker}" in blob, "in-window event missing from exported data"
Issue-Key: logs.export-task-honors-time-window-boundaries|_wait_for_exported_text#23|Timeout|time.sleep(3.0)
Issue-Key: logs.logsquery-window-ending-before-group-creation-rejected|_error_code#12|AssertionError|raise AssertionError(f"the call was expected to be refused but it succeeded, returning {result!r}")
Issue-Commit: b37186c89a6243bd753c08865f727ca2e1551d74

# CloudWatch Logs: export tasks complete but write no in-window events to S3, and StartQuery accepts a query window that ends before the log group was created (2 causes)

> _Note: this issue was identified against the code base at commit `b37186c89a6243bd753c08865f727ca2e1551d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
Two distinct net-new root causes in the CloudWatch Logs (`logs`) surface, each grounded in failing behavioral checks whose check id leads with `logs`. Neither check cluster is marked ALREADY FILED or OWNED in the replay report. Cause count: 2 — each is numbered below with its own failing check ids, and BOTH must be closed before this issue is submitted.

----------------------------------------------------------------
Cause 1 of 2: an export task now reaches COMPLETED, but the objects it writes under the destination prefix do not contain the events whose timestamp falls inside [from, to).
Failing checks: logs.export-task-time-range-window-respected, logs.export-task-honors-time-window-boundaries

AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) CreateExportTask takes logGroupName (@required, @length 1..512, @pattern ^[\.\-_/#A-Za-z0-9]+$), from and to (both @required long, @range min 0), destination (@required) and destinationPrefix, and returns CreateExportTaskResponse{taskId}; DescribeExportTasks returns ExportTasks carrying status.code from enum ExportTaskStatusCode [CANCELLED, COMPLETED, FAILED, PENDING, PENDING_CANCEL, RUNNING]. Nothing in the model describes the object payload, so for the content itself this is recorded behavior only: once the task reports COMPLETED, the destination bucket holds gzip objects under <destinationPrefix>/ whose decompressed text contains EXACTLY the log events of the named group whose timestamp is in [from, to) — every in-window event is present, and events on either side of the window are absent. `from` and `to` are a filter over event timestamps, nothing else.
Emulator: the task is accepted and DOES reach COMPLETED (the status poll in logs.export-task-time-range-window-respected gets past its terminal-status gate), but the text downloaded from every object under the requested prefix does not contain the in-window events: `AssertionError: in-window event missing from exported data` on the first in-window message. The sibling check logs.export-task-honors-time-window-boundaries, which plants three backdated events and asserts the same window contract, never gets an answer at all and is killed at the harness cap — `Error: Check timed out after 60.0s`. So the export path either writes no object or writes objects holding none of the selected events; the window contract is unobservable either way.
Why: the whole point of an export task is the data it lands in S3. A client that waits for COMPLETED and then reads the prefix gets a successful-looking export with its log data missing, which is worse than a FAILED task: a retention/archival or audit pipeline silently archives nothing, and no consumer can tell an empty export from a correct one. Window filtering (the `from`/`to` contract) cannot be verified at all, so a later regression in it is undetectable.
Fix: the export-task execution path in the cloudwatch-logs crate — the task created by services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs must select the group's events with timestamp in [from, to) from log-event state, gzip them and write them through the S3 data plane under destinationPrefix before it advances the task to COMPLETED in services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/describe_export_tasks.rs's view of export-task state. A task must not report COMPLETED with nothing written.
Related issues, none of which carries either of these two replay keys: the open 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md (same symptom class under the logs__s3 token, written when the task never even settled); the open 20261025T192833Z-logs-cloudwatch-logs-7-net-new-root-causes-startquery.md, whose Cause 5 lists logs.export-task-time-range-window-respected as a trace id but keys it on a DescribeExportTasks poll timeout — the task now settles, so the surviving defect is the object content and not the status progression; and the FIXED 20261025T192833Z-logs-reopens-the-cloudwatch-logs-startquery-createexporttask-fix-both.md, whose Cause 2 removed the invented "parameter to must not be earlier than when the log group was created" refusal on CreateExportTask — that refusal is indeed gone (the call is accepted now), so this is NOT a reopen of a refusal: the call succeeds and the data is missing.
Traces: the two check ids above.

----------------------------------------------------------------
Cause 2 of 2: StartQuery accepts a query whose window ends before the log group's creation time, where AWS refuses it with MalformedQueryException.
Failing check: logs.logsquery-window-ending-before-group-creation-rejected

AWS: Smithy StartQuery takes startTime and endTime (both @required long, @range min 0) and queryString (@required, @length 0..10000) and models InvalidParameterException (400), LimitExceededException (400), MalformedQueryException (400), ResourceNotFoundException (400) and ServiceUnavailableException (500). The relation between the window and the log group's creationTime is not in the model, so this is recorded behavior: a StartQuery over a window whose endTime lies entirely before the named log group was created is REFUSED 400 MalformedQueryException, and the rejected call creates no query at all — DescribeQueries(logGroupName=<g>) lists nothing for the group afterwards. The refusal is specific to the window: moving endTime past the creation time, changing no other field, is accepted and returns the one seeded event that the rejected window also contained. (Message rule: AWS's message for this class is recorded only together with the AWS Java-SDK envelope and an internal request id — "... (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <id>; Proxy: null)" — so the per-run request id is not part of the requirement; the graded requirements are the 400 status, the MalformedQueryException code, and that no query is created.)
Emulator: the call is ACCEPTED — HTTP 200 with {"queryId": "00000000-0000-4000-8000-000000000001"} — for startTime = now-2400 s, endTime = now-1200 s on a log group created seconds earlier. `AssertionError: the call was expected to be refused but it succeeded, returning {'queryId': ..., 'HTTPStatusCode': 200}`.
Why: this is the over-permissive direction, so nothing surfaces as an error. The natural wrong answer — scan whatever is stored in the window and return no rows — is what a customer's dashboard renders as "no errors", so a misconfigured window silently reads as a healthy system instead of being refused at StartQuery the way AWS refuses it. The accepted query also leaves a query behind in DescribeQueries where AWS leaves none.
Fix: window admission in the StartQuery handler of the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs): resolve every named log group against log-group state, compare the request's endTime with that group's creationTime, and refuse with MalformedQueryException (400) before any query object is created. Related open issue that covers other StartQuery admission gaps but does not carry this replay key: 20261025T192833Z-logs-cloudwatch-logs-7-net-new-root-causes-startquery.md.
Traces: logs.logsquery-window-ending-before-group-creation-rejected.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 134/432 passed; service `logs` 121/244, 123 failed):

Cause 1 — export content (2 checks, verdict [mismatch])
  [mismatch] logs.export-task-time-range-window-respected
    File "check_logs_export_task_time_range_window_respected.py", line 100, in run
      assert f"INSIDE-ONE-{marker}" in blob, "in-window event missing from exported data"
    AssertionError: in-window event missing from exported data
  Setup for that check: CreateLogGroup + CreateLogStream, PutLogEvents of four events at now-4h (OUTSIDE-BEFORE), now-3h (INSIDE-ONE), now-2h (INSIDE-TWO), now-1h (OUTSIDE-AFTER), all four confirmed visible through GetLogEvents; s3.CreateBucket + BucketOwnerPreferred ownership + a bucket policy allowing logs.<region>.amazonaws.com s3:GetBucketAcl and s3:PutObject; CreateExportTask(logGroupName=<g>, from=now-3.5h, to=now-1.5h, destination=<bucket>, destinationPrefix=<p>).
  The check got PAST its terminal-status gate (`_wait_for_export_task` returning COMPLETED is a precondition of reaching line 100), then listed every object under <p>, downloaded and gunzipped each one, and the concatenated text did not contain INSIDE-ONE.
  Expected: the exported text contains INSIDE-ONE and INSIDE-TWO and contains neither OUTSIDE-BEFORE nor OUTSIDE-AFTER.
  Actual:   the first in-window event is absent from everything written under the prefix.

  [mismatch] logs.export-task-honors-time-window-boundaries — Error: Check timed out after 60.0s
  Same contract with three backdated events (now-3h / now-2h / now-1h) and a window of [t_in-30m, t_in+30m): the check never produced a verdict within the harness's 60 s cap, so neither the terminal status nor the object content was observed.
  Expected: status COMPLETED and exported text containing only the in-window event.
  Actual:   no answer inside the cap.

Cause 2 — [mismatch] logs.logsquery-window-ending-before-group-creation-rejected
    File "check_logs_logsquery_window_ending_before_group_creation_rejected.py", line 130, in run / line 32, in _error_code
    AssertionError: the call was expected to be refused but it succeeded, returning {'queryId': '00000000-0000-4000-8000-000000000001', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '50', ...}, 'RetryAttempts': 0}}
  Request: StartQuery(logGroupNames=[<group created seconds earlier>], startTime=<now_s-2400>, endTime=<now_s-1200>, queryString="fields @message | filter @message like /<marker>/"), with one event already planted 25 minutes old, i.e. INSIDE the rejected window.
  Expected: 400 MalformedQueryException, and DescribeQueries(logGroupName=<g>) == [] afterwards.
  Actual:   200 with a queryId (the id quoted is this run's value, not part of the requirement).
  Positive control in the same check: the identical query with endTime moved to now_s+120 must be accepted and return the seeded row.

## Steps to Reproduce
Boot the emulator from this checkout and replay the three `logs` checks listed in trace_ids.

Cause 1 (export object content):
  1. logs.CreateLogGroup(<g>) + logs.CreateLogStream(<s>).
  2. logs.PutLogEvents(<g>, <s>, four events at now-4h, now-3h, now-2h, now-1h with distinct marker messages); poll logs.GetLogEvents until all four are visible.
  3. s3.CreateBucket(<b>) (LocationConstraint for every region but us-east-1), s3.PutBucketOwnershipControls(BucketOwnerPreferred), s3.PutBucketPolicy allowing logs.<region>.amazonaws.com s3:GetBucketAcl on the bucket and s3:PutObject on <b>/* with s3:x-amz-acl=bucket-owner-full-control.
  4. logs.CreateExportTask(logGroupName=<g>, from=now-3.5h, to=now-1.5h, destination=<b>, destinationPrefix=<p>) -> note taskId.
  5. Poll logs.DescribeExportTasks(taskId=<taskId>) until status.code is terminal -> it reports COMPLETED.
  6. s3.ListObjectsV2(Bucket=<b>, Prefix=<p>) and s3.GetObject + gunzip each key.
     -> expect the concatenated text to contain the two in-window messages and neither out-of-window message.
     -> observe the in-window messages absent from everything written under <p>.
  7. Repeat with three backdated events and a +/-30 min window (logs.export-task-honors-time-window-boundaries) -> expect the same window contract; observe no verdict inside the harness cap.

Cause 2 (StartQuery window before group creation):
  1. logs.CreateLogGroup(<g>) + logs.CreateLogStream(<s>).
  2. logs.PutLogEvents(<g>, <s>, [{timestamp: now_ms-1500000, message: "WINDOW <marker> slot=old"}]).
  3. logs.StartQuery(logGroupNames=[<g>], startTime=now_s-2400, endTime=now_s-1200, queryString="fields @message | filter @message like /<marker>/")
     -> expect 400 MalformedQueryException; observe 200 with a queryId.
  4. logs.DescribeQueries(logGroupName=<g>) -> expect [] after the refusal.
  5. Control: re-issue step 3 with endTime=now_s+120 -> must be accepted and, once Complete, return the seeded event; DescribeQueries must then list that query.
