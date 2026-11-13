Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-cluster-lifecycle-op-preconditions
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-cluster-lifecycle-op-preconditions redshift-usage-limit-endpoint-and-scheduled-action-preconditions
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|PauseCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateScheduledAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifyScheduledAction|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|ModifyScheduledAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteScheduledAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: b037d5392f1efc291497c72edeaad9a6f7e9925d

# Redshift scheduled-action and PauseCluster refusals use emulator-authored wording and validate request syntax before resource existence

> _Note: this issue was identified against the code base at commit `b037d5392f1efc291497c72edeaad9a6f7e9925d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyScheduledAction

## Description
Four net-new refusal divergences in the redshift service. They share one root cause family: the emulator writes its own not-found message text instead of AWS's, and it runs request-syntax validation (Schedule expression parsing) BEFORE the precondition checks AWS performs first (IAM-role validity, scheduled-action existence), so the wrong fault is reported.

Cause 1 of 4 — PauseCluster ClusterNotFound message wording.
AWS: Smithy: PauseCluster input PauseClusterMessage {ClusterIdentifier (required)}; modeled error ClusterNotFoundFault, awsQueryError code 'ClusterNotFound', HTTP 404, doc 'The ClusterIdentifier parameter does not refer to an existing cluster.' Recorded behavior: HTTP 404, Error.Code 'ClusterNotFound', Error.Type 'Sender', Error.Message "Cluster <clusterIdentifier> can't be found." The fixed text is 'Cluster ' + " can't be found."; the only variable part is the ClusterIdentifier taken verbatim from the request.
Emulator: same 404 and code, but Message 'Cluster <clusterIdentifier> not found.' — the emulator's own template, not AWS's.
Why: clients and test suites match on the refusal sentence to tell 'no such cluster' apart from other 404s; a reworded message breaks that matching, and it also makes AWS's per-operation wording differences invisible (FailoverPrimaryCompute words the same fault differently, 'Cluster not found: <id>'), so an emulator that emits one uniform sentence cannot be right for both.
Fix: the PauseCluster handler's cluster-identifier lookup/refusal path in the redshift service crate — the ClusterNotFoundFault message template for this operation.

Cause 2 of 4 — CreateScheduledAction validates the Schedule expression before the IamRole.
AWS: Smithy: CreateScheduledAction input requires ScheduledActionName, TargetAction, Schedule and IamRole; modeled errors include InvalidScheduleFault (InvalidSchedule, 400) and InvalidScheduledActionFault (InvalidScheduledAction, 400). Recorded behavior for a request that carries BOTH an unparseable Schedule and an IamRole naming a role that does not exist: HTTP 400 with Error.Message 'The IAM role <iamRoleArn> is not valid. The IAM role must delegate access to Amazon Redshift scheduler.' The error code matched the emulator's and was not flagged. AWS therefore validates the scheduler IAM role first and reports that; the fixed text is 'The IAM role ' + ' is not valid. The IAM role must delegate access to Amazon Redshift scheduler.' with the role ARN taken from the request.
Emulator: HTTP 400 whose Message is its Schedule-syntax complaint, "Invalid value for Schedule: '<schedule>'. Schedule must be an at(yyyy-mm-ddThh:mm:ss) or cron(Minutes Hours Day-of-month Month Day-of-week Year) expression." The IamRole is never checked.
Why: a client fixing the reported problem (the schedule) still cannot create the action, because the real blocker — an IAM role that does not delegate to the Redshift scheduler — is never reported. The emulator also never refuses an undelegated role at all, so the role precondition is unenforced.
Fix: CreateScheduledAction's validation ordering in the redshift service crate — resolve and validate IamRole against IAM state (role exists and trusts the Redshift scheduler) before parsing Schedule, and emit AWS's role-refusal message.

Cause 3 of 4 — ModifyScheduledAction reports a Schedule-syntax refusal instead of ScheduledActionNotFound.
AWS: Smithy: ModifyScheduledAction requires ScheduledActionName; modeled errors include ScheduledActionNotFoundFault (awsQueryError code 'ScheduledActionNotFound', HTTP 400, doc 'The scheduled action cannot be found.') and InvalidScheduleFault. Recorded behavior for a modify naming a scheduled action that does not exist (and also carrying an unsupported rate() Schedule): HTTP 400, Error.Code 'ScheduledActionNotFound', Error.Message 'The scheduled action <scheduledActionName> was not found.' Fixed text 'The scheduled action ' + ' was not found.'; the name comes from the request. Existence of the named action is resolved before the Schedule value is parsed.
Emulator: HTTP 400 with Error.Code 'InvalidParameterValue' and Message "Invalid value for Schedule: '<schedule>'. Schedule must be an at(yyyy-mm-ddThh:mm:ss) or cron(Minutes Hours Day-of-month Month Day-of-week Year) expression." — the wrong error code AND the wrong message; InvalidParameterValue is not even a modeled error of this operation.
Why: the error code is the part SDKs branch on. A client that catches ScheduledActionNotFoundFault to fall back to CreateScheduledAction never sees it and instead gets an unmodeled parameter error, so upsert logic and retry/ignore-if-absent paths break.
Fix: ModifyScheduledAction in the redshift service crate — look the ScheduledActionName up in scheduled-action state first and raise ScheduledActionNotFoundFault with AWS's wording before validating Schedule/TargetAction.

Cause 4 of 4 — DeleteScheduledAction ScheduledActionNotFound message wording.
AWS: Smithy: DeleteScheduledAction input requires ScheduledActionName; modeled errors ScheduledActionNotFoundFault (ScheduledActionNotFound, 400) and UnauthorizedOperation. Recorded behavior: HTTP 400 with Error.Message 'The scheduled action <scheduledActionName> was not found.' — the same fixed template as ModifyScheduledAction, with the name from the request.
Emulator: correct status and code, but Message 'ScheduledAction not found: <scheduledActionName>' — the emulator's generic '<Type> not found: <id>' template.
Why: callers that assert on the refusal sentence, or that parse it to recover the name, diverge; and the generic template leaks the emulator's internal naming ('ScheduledAction') rather than AWS's prose.
Fix: the ScheduledActionNotFoundFault message template used by DeleteScheduledAction (and shared with ModifyScheduledAction) in the redshift service crate's scheduled-action error construction.

Related open issues that do NOT carry these fixture steps' replay keys: '20261108T233629Z-redshift-cluster-identifier-refusals-diverge-no-identifier-syntax.md' (ClusterNotFound wording, owns FailoverPrimaryCompute only) and '20261108T233629Z-redshift-snapshot-schedule-lifecycle-diverges-missing-invalidschedule-floor.md' (snapshot schedules, a different resource). The replay reports all four clusters here as unowned.

Traces: redshift-cluster-lifecycle-op-preconditions#task#4; redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#9, #task#10, #task#11.

## Evidence
Cause 1 — PauseCluster (redshift-cluster-lifecycle-op-preconditions#task#4), status 404 expected and actual:
  expected $.ErrorResponse.Error.Message = "Cluster sim2real-probe-cluster-missing can't be found."
  actual   $.ErrorResponse.Error.Message = 'Cluster sim2real-probe-cluster-missing not found.'
  (the cluster identifier is the request value; the rest is fixed text)

Cause 2 — CreateScheduledAction (redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#9), status 400 expected and actual:
  expected $.ErrorResponse.Error.Message = 'The IAM role arn:aws:iam::000000000000:role/sim2real-probe-role-missing is not valid. The IAM role must delegate access to Amazon Redshift scheduler.'
  actual   $.ErrorResponse.Error.Message = "Invalid value for Schedule: 'not-a-valid-schedule'. Schedule must be an at(yyyy-mm-ddThh:mm:ss) or cron(Minutes Hours Day-of-month Month Day-of-week Year) expression."
  (role ARN and schedule string are request values; Error.Code was not flagged, so only the message/precedence diverges)

Cause 3 — ModifyScheduledAction (redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#10), status 400 expected and actual:
  expected $.ErrorResponse.Error.Code = 'ScheduledActionNotFound'; Message = 'The scheduled action sim2real-probe-sa-missing was not found.'
  actual   $.ErrorResponse.Error.Code = 'InvalidParameterValue'; Message = "Invalid value for Schedule: 'rate(24 hours)'. Schedule must be an at(yyyy-mm-ddThh:mm:ss) or cron(Minutes Hours Day-of-month Month Day-of-week Year) expression."

Cause 4 — DeleteScheduledAction (redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#11), status 400 expected and actual:
  expected $.ErrorResponse.Error.Message = 'The scheduled action sim2real-probe-sa-missing was not found.'
  actual   $.ErrorResponse.Error.Message = 'ScheduledAction not found: sim2real-probe-sa-missing'
  (the scheduled-action name is the request value; 'The scheduled action ' / ' was not found.' is fixed text)

## Steps to Reproduce
1. Replay fixture redshift-cluster-lifecycle-op-preconditions and inspect step task#4: POST Action=PauseCluster with a ClusterIdentifier that does not exist -> 404 ClusterNotFound whose message must read "Cluster <id> can't be found."
2. Replay fixture redshift-usage-limit-endpoint-and-scheduled-action-preconditions:
   a. step task#9: POST Action=CreateScheduledAction with an unparseable Schedule AND an IamRole ARN naming a role that does not exist -> AWS refuses on the IAM role, not the schedule.
   b. step task#10: POST Action=ModifyScheduledAction naming a ScheduledActionName that does not exist, with Schedule='rate(24 hours)' -> AWS refuses with code ScheduledActionNotFound.
   c. step task#11: POST Action=DeleteScheduledAction naming a ScheduledActionName that does not exist -> 400 ScheduledActionNotFound with message 'The scheduled action <name> was not found.'
3. Compare Error.Code and Error.Message against the expectations above.
