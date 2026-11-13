Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# Health derivation ignores scheduledReportExecutionRoleArn, so a report with an unassumable role reports HEALTHY and triggers

_Filed: 2026-10-24T16:45:54Z_
_Operation: ExecuteScheduledReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ExecuteScheduledReport` derives the report's health from the dashboard store only (`crate::state::scheduled_report_health`): it reports `UNHEALTHY` with `DASHBOARD_NOT_FOUND` or `WIDGET_ID_NOT_FOUND` and otherwise declares `HEALTHY` and sets `executionTriggered: true`. The `scheduledReportExecutionRoleArn` — the role the service documents it will assume to run the report — is never examined at all.

Expected: `HEALTHY` means "the scheduled report is configured properly and has all required permissions to execute" (model doc on `HealthStatusCode`). A report whose execution role does not exist in IAM, or exists but does not trust the bcm-dashboards service principal, cannot be assumed, so the execution cannot run: the answer should be `UNHEALTHY` with `statusReasons` [`EXECUTION_ROLE_ASSUME_FAILED`] (and `EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS` when the role is assumable but its policies do not allow the report's work) and `executionTriggered: false`, with that refreshed health then visible to `GetScheduledReport`/`ListScheduledReports`.

Actual: with an execution role ARN that names no IAM role at all (the pattern-valid value every test fixture uses, which is never created in IAM), the call answers 200 `{healthStatus: {statusCode: "HEALTHY", lastRefreshedAt: now}, executionTriggered: true}` and records `lastExecutionAt` — a canned-healthy verdict for a configuration the real service reports as broken. Two of the seven modelled `StatusReason` values are therefore unreachable, and clients cannot develop the "fix your execution role" path.

## Evidence

Spec: `StatusReason` enumerates EXECUTION_ROLE_ASSUME_FAILED and EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS alongside DASHBOARD_NOT_FOUND/WIDGET_ID_NOT_FOUND (`smithy_query(query="shape",shape="StatusReason")`); `HealthStatus.statusCode` doc defines HEALTHY as "configured properly and has all required permissions to execute"; `ScheduledReportInput.scheduledReportExecutionRoleArn` doc: "The ARN of the IAM role that the scheduled report uses to execute. Amazon Web Services Billing and Cost Management Dashboards will assume this IAM role while executing the scheduled report."
Handler/state: `src/ops/execute_scheduled_report.rs` lines 286-302 computes `state.scheduled_report_health(&record, now)` and sets `execution_triggered = status_code == HEALTHY`; `src/state.rs` `scheduled_report_health` (lines 478-515) inspects only `self.dashboards` and the record's `widget_ids` — `execution_role_arn` is stored (src/state.rs:184) and never read by any code path, and the doc comment concedes the role-related reasons are "never claimed".
Facility available in-tree and unused: `service_common::iam_trust_policy::admits_service_principal` plus the IAM store, which eventbridge, firehose, scheduler and fis use to judge a caller-supplied execution role; this crate's Cargo.toml depends on neither.
Tests: `tests/all/scheduled_report_execution.rs:29-35` documents that only the two dashboard-side reasons are produced and uses a role ARN (`arn:aws:iam::123456789012:role/BCMDashboardsReportRole`) that is never created in any IAM emulator, so no test asserts a healthy verdict is correct for a missing role — the behavior is uncovered.

## Steps to Reproduce

1. Register this service beside the IAM emulator on one engine.
2. CreateDashboard with one widget and keep the dashboard ARN.
3. CreateScheduledReport on that dashboard with scheduledReportExecutionRoleArn = arn:aws:iam::123456789012:role/NoSuchReportRole (no such role is created in IAM) and scheduleConfig {scheduleExpression: "cron(0 8 1 * ? *)"}.
4. Call ExecuteScheduledReport with dryRun=true on the new report ARN — expect healthStatus UNHEALTHY with statusReasons [EXECUTION_ROLE_ASSUME_FAILED]; observe HEALTHY with no statusReasons.
5. Call ExecuteScheduledReport without dryRun — expect executionTriggered=false and the UNHEALTHY health persisted; observe executionTriggered=true, lastExecutionAt set, and GetScheduledReport reporting HEALTHY.
6. Create an IAM role whose trust policy does NOT name the bcm-dashboards service principal, point a report at it, and repeat — the answer is still HEALTHY/triggered.
