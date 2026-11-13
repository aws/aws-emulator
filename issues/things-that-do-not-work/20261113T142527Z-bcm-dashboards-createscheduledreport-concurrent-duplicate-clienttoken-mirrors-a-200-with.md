Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards.create-scheduled-report-idempotent-token-concurrent
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards.create-scheduled-report-idempotent-token-concurrent|run#12|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got {successes} and {errors!r}"
Issue-Commit: 8519584554abb240da771f66a4fe7c47c54f612d

# CreateScheduledReport: concurrent duplicate clientToken mirrors a 200 with the winner's ARN to every racer instead of refusing them with ConflictException

> _Note: this issue was identified against the code base at commit `8519584554abb240da771f66a4fe7c47c54f612d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduledReport

## Description
ONE root cause (Cause 1 of 1): the clientToken idempotency branch answers a duplicate token by REPLAYING the winner's success, so a fleet of concurrent duplicate-token callers all receive HTTP 200, and the modelled ConflictException is never produced for the racing losers.

=== Cause 1 of 1: a duplicate clientToken racing an in-flight create is answered 200 with the first ARN instead of ConflictException/409 ===
AWS: Smithy (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json) - CreateScheduledReport.clientToken targets ClientToken, @idempotencyToken, header X-Amzn-Client-Token, @length 1..128, @pattern ^[\u0021-\u007E]+$; the operation models ConflictException at HTTP 409 (documented for "attempting to create a resource that already exists or is being created"), and its output member `arn` targets ScheduledReportArn @pattern ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$. Recorded behavior for N concurrent CreateScheduledReport calls that share ONE clientToken and identical parameters: exactly ONE call succeeds (HTTP 200, one minted scheduled-report ARN, and exactly one report of that name listed by ListScheduledReports); every other call is REFUSED with error code ConflictException at HTTP 409. Which of the two ConflictException texts a loser gets depends on whether it overlapped the winner, and both are correct AWS behavior for this operation: while the winner is still in flight the fixed text is "A scheduled report creation is already in progress for the provided clientToken. Please retry after some time." (no variable parts); once the winner has completed the fixed text is "A scheduled report with the provided clientToken already exists (ARN: <arn>). Use a different clientToken to create a new scheduled report, or use the existing report's ARN to update it.", whose single variable part is the ARN this service stored under that token (taken from emulator state, never a literal from one run). A duplicate-token caller is NEVER given a 200.
Emulator: all five concurrent calls return HTTP 200, each carrying the SAME scheduled-report ARN (the winner's), and ZERO calls are refused - the replay reports `expected exactly one winner, got [<arn>, <arn>, <arn>, <arn>, <arn>] and []`, i.e. five successes and an empty error list. The handler is treating a matching-fingerprint duplicate token as a mirrored success rather than as a conflict, so the modelled ConflictException is unreachable in the concurrent case.
Why: the clientToken contract is what an idempotency-token client branches on. Against AWS a racing retry is told, by error code, either to back off and retry the same token (in-progress) or that the token is terminally spent (already-exists, naming the ARN to update instead); against this emulator every racer is told it created the report. A client developed here never exercises its 409 handling, treats a duplicate as its own create (and may then own/delete a resource another caller created), and any test that asserts single-winner semantics passes on the emulator while failing on AWS.
Why the existing open issue does not cover it: open issue 20261022T215638Z-bcm-dashboards-dashboardname-pattern-unenforced-constraint-not-found.md cause 4 describes the OPPOSITE emulator symptom for this check (zero successes, every concurrent call refused) and does not carry this check's current replay key, so it does not cover the mirrored-success divergence reported here. Fixed issue 20261024T030750Z-bcm-dashboards-createscheduledreport-wrong-behavior.md asked for a reused token to replay the original ARN at 200; that replay must be scoped so it never applies to a duplicate presented while the create is in flight (or after it completed - see fixed issue 20261030T033947Z), which is the behavior this check grades.
Fix: the CreateScheduledReport handler's idempotency/conflict branch and the creation record in this service's state that it consults - a clientToken must key an at-most-once create whose record distinguishes in-flight from completed, and a duplicate token must take the modelled ConflictException (HTTP 409) with the in-progress text while the winner is in flight and the already-exists text (naming the stored ARN) afterwards, instead of being served the winner's 200 response. No change belongs in the response serializer or in ListScheduledReports.
Traces: bcm-dashboards.create-scheduled-report-idempotent-token-concurrent

## Evidence
Suite: simparity-behavioral-aws-v1 - 24/29 passed; bcm-dashboards 24/26 (92%), 2 failed.

[mismatch] bcm-dashboards.create-scheduled-report-idempotent-token-concurrent
  Flow: create a dashboard + an execution role, then 5 parallel CreateScheduledReport calls released from one barrier, all with identical scheduledReport parameters and ONE shared clientToken.
  Expected: exactly 1 success whose `arn` matches ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/[-a-z0-9]+$, and 4 refusals with code ConflictException (HTTP 409) carrying either the in-progress text "A scheduled report creation is already in progress for the provided clientToken. Please retry after some time." or the already-exists text "A scheduled report with the provided clientToken already exists (ARN: <winner arn>). Use a different clientToken to create a new scheduled report, or use the existing report's ARN to update it."; then exactly one report of that name from ListScheduledReports.
  Actual: 5 successes, 0 errors. Assertion (check_bcm_dashboards_create_scheduled_report_idempotent_token_concurrent.py line 72):
    AssertionError: expected exactly one winner, got ['arn:aws:bcm-dashboards::000000000000:scheduled-report/00000000000000000000000000000001', 'arn:aws:bcm-dashboards::000000000000:scheduled-report/00000000000000000000000000000001', 'arn:aws:bcm-dashboards::000000000000:scheduled-report/00000000000000000000000000000001', 'arn:aws:bcm-dashboards::000000000000:scheduled-report/00000000000000000000000000000001', 'arn:aws:bcm-dashboards::000000000000:scheduled-report/00000000000000000000000000000001'] and []
  (The repeated ARN above is this run's generated id; the requirement is that four of the five calls are refused, not that any particular ARN value appears.)
  The check's later assertions - that the single winner's ARN is the only report listed under that name - were never reached and remain unverified until the refusal behavior is fixed.

Smithy consulted (smithy_query query="operation", operation="CreateScheduledReport"): input {scheduledReport required; resourceTags list @length 0..200; clientToken -> ClientToken @idempotencyToken, header X-Amzn-Client-Token, @length 1..128, @pattern ^[\u0021-\u007E]+$}, output {arn required, @length 20..2048, @pattern ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$}, errors AccessDeniedException 403 / ConflictException 409 / InternalServerException 500 / ServiceQuotaExceededException 402 / ThrottlingException 429 / ValidationException 400. The model fixes the error shape and status; the two message texts are recorded behavior.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore bcm-dashboards client at it (an IAM emulator is also used to mint the execution role).
2. create_dashboard(name=<valid>, widgets=[one cost-and-usage widget]) -> dashboard ARN D.
3. iam.create_role(...) with a trust policy naming bcm-dashboards.amazonaws.com -> role ARN R.
4. Pick one clientToken T and one request body: {scheduledReport: {name: N, dashboardArn: D, scheduledReportExecutionRoleArn: R, scheduleConfig: {scheduleExpression: "cron(0 8 ? * MON *)", scheduleExpressionTimeZone: "UTC"}}, resourceTags: [...], clientToken: T}.
5. Release 5 threads through a barrier, each issuing create_scheduled_report with that identical body.
6. Expect 1 success + 4 ConflictException/409 (in-progress or already-exists wording); observe 5 HTTP 200 responses all carrying the same arn and no error at all.
7. list_scheduled_reports and confirm exactly one report named N exists.
8. Re-replay check bcm-dashboards.create-scheduled-report-idempotent-token-concurrent to confirm the fix.
