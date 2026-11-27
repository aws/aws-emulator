Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.maintenance-window-execution-failed-when-its-automation-task-fails ssm.maintenance-window-execution-listed-cancelled-after-cancel-mid-run ssm.maintenance-window-execution-with-a-failing-task-ends-failed ssm.maintenance-window-executions-list-newest-first ssm.e2e-maintenance-window-execution-history-flow ssm.maintenance-window-get-returns-create-members-and-offset-next-run ssm.describe-maintenance-windows-item-agrees-with-get-after-update
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.maintenance-window-execution-failed-when-its-automation-task-fails|_ended_execution#10|AssertionError|raise AssertionError(f"window never listed an ended execution; listed {listed!r}")
Issue-Key: ssm.maintenance-window-execution-listed-cancelled-after-cancel-mid-run|_wait_listed#10|AssertionError|raise AssertionError(f"window never listed {what}; listed {listed!r}")
Issue-Key: ssm.maintenance-window-execution-with-a-failing-task-ends-failed|_first_finished_execution#14|AssertionError|raise AssertionError(f"window executions at the deadline: {rows!r}")
Issue-Key: ssm.maintenance-window-executions-list-newest-first|run#13|AssertionError|raise AssertionError(f"after {budget:.0f}s the window had executions {rows!r}")
Issue-Key: ssm.maintenance-window-get-returns-create-members-and-offset-next-run|run#18|AssertionError|assert "NextExecutionTime" in got, f"no NextExecutionTime in {sorted(got)!r}"
Issue-Key: ssm.describe-maintenance-windows-item-agrees-with-get-after-update|run#15|KeyError|nxt = _instant(full["NextExecutionTime"])
Issue-Related: 20261122T161603Z-ssm-2-net-new-causes-a-maintenance-window.md
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ssm: 2 causes — a maintenance window's Schedule never produces a WindowExecution, and GetMaintenanceWindow/DescribeMaintenanceWindows omit NextExecutionTime

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMaintenanceWindow

## Description
Two net-new causes in SSM Maintenance Windows. (The open issue 20261122T161603Z-ssm-2-net-new-causes-a-maintenance-window owns a different set of window checks; the checks listed here are not carried by it.)

Cause 1 of 2: a window with a due Schedule never produces an execution, so no execution history exists.
AWS: a window whose cron/rate Schedule is due inside its StartDate/EndDate and whose Enabled is true runs its registered tasks; DescribeMaintenanceWindowExecutions then lists a WindowExecution (newest first) whose Status follows its tasks — Failed when its automation or run-command task fails, Cancelled after CancelMaintenanceWindowExecution mid-run.
Emulator: DescribeMaintenanceWindowExecutions keeps answering an empty list for the whole poll window (330 s in one check), so no execution, no task, no invocation and no ordering can be read.
Why: every Maintenance Window outcome — task success/failure, cancellation, execution history, newest-first ordering — is unobservable; a client waiting on a window hangs.
Fix: the maintenance-window scheduler that evaluates Schedule/ScheduleTimezone/ScheduleOffset against the clock and records WindowExecutions (services/service-com-amazonaws-ssm-amazonssm/src/maintenance_window/…).
Traces: ssm.maintenance-window-execution-failed-when-its-automation-task-fails, ssm.maintenance-window-execution-listed-cancelled-after-cancel-mid-run, ssm.maintenance-window-execution-with-a-failing-task-ends-failed, ssm.maintenance-window-executions-list-newest-first, ssm.e2e-maintenance-window-execution-history-flow.

Cause 2 of 2: NextExecutionTime is never returned for a window.
AWS: Smithy GetMaintenanceWindowResult declares NextExecutionTime (MaintenanceWindowStringDateTime), and MaintenanceWindowIdentity in DescribeMaintenanceWindows carries it too; for an enabled window with a Schedule (including one with a ScheduleOffset) AWS returns the next due time.
Emulator: the GetMaintenanceWindow response holds only [AllowUnassociatedTargets, CreatedDate, Cutoff, Description, Duration, Enabled, EndDate, ModifiedDate, Name, Schedule, ScheduleOffset, ScheduleTimezone, StartDate, WindowId] — the member is absent, and a client that reads it raises KeyError: 'NextExecutionTime'.
Why: a caller cannot tell when a window will next run, and code that reads the modelled member crashes rather than seeing a value.
Fix: the GetMaintenanceWindow / DescribeMaintenanceWindows serializers and the next-run computation behind them.
Traces: ssm.maintenance-window-get-returns-create-members-and-offset-next-run, ssm.describe-maintenance-windows-item-agrees-with-get-after-update.

## Evidence
Cause 1: 'window never listed an ended execution; listed []', 'window never listed an execution running its first task; listed []', 'window executions at the deadline: []', 'after 330s the window had executions []', 'condition never satisfied, last value=[]'.
Cause 2: 'no NextExecutionTime in [AllowUnassociatedTargets, CreatedDate, Cutoff, Description, Duration, Enabled, EndDate, ModifiedDate, Name, ResponseMetadata, Schedule, ScheduleOffset, ScheduleTimezone, StartDate, WindowId]'; and KeyError: 'NextExecutionTime' when the describe/get pair is compared after an update.

## Steps to Reproduce
1. CreateMaintenanceWindow (Enabled, Schedule 'cron(...)' due within a few minutes, Duration/Cutoff set), register a target and a run-command or automation task, then poll DescribeMaintenanceWindowExecutions for a WindowExecution.
2. Repeat with a task that fails, and with CancelMaintenanceWindowExecution mid-run; read the execution Status.
3. Call GetMaintenanceWindow and DescribeMaintenanceWindows on the window (with and without ScheduleOffset) and look for NextExecutionTime.
