Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-maintenance-window-lifecycle
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-maintenance-window-lifecycle ssm-parameter-unlabel-partial-semantics
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DescribeMaintenanceWindowTasks|2xx->2xx|$.Tasks[*]
Issue-Key: ssm|DescribeParameters|2xx->2xx|$.Parameters[*]
Issue-Commit: b61d91764c5b64113b88eda248364bbad90afe06

# SSM read projections drop modeled members and rewrite the parameter Name

> _Note: this issue was identified against the code base at commit `b61d91764c5b64113b88eda248364bbad90afe06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeMaintenanceWindowTasks

## Description
Two net-new causes under one heading: a 200 read answers with a record whose modeled members do not match what the write persisted — one maintenance-window read omits two members and invents a third, and one Parameter Store read rewrites the parameter's name. Each cause is a separate recorded step a fixer must re-replay.

Cause 1 of 2: DescribeMaintenanceWindowTasks omits ServiceRoleArn and TaskParameters and adds a CutoffBehavior the registration never set.
AWS: Smithy (ssm): DescribeMaintenanceWindowTasks input {WindowId @pattern ^mw-[0-9a-f]{17}$ (required), Filters, MaxResults @range 10..100, NextToken}; output {Tasks: MaintenanceWindowTaskList, NextToken}. The `MaintenanceWindowTask` member `ServiceRoleArn` is documented "The Amazon Resource Name (ARN) of the IAM service role for Amazon Web Services Systems Manager to assume when running a maintenance window task. If you do not specify a service role ARN, Systems Manager uses a service-linked role in your account. If no appropriate service-linked role for Systems Manager exists in your account, it is created when you run RegisterTaskWithMaintenanceWindow" — i.e. the member is DEFAULTED on registration and is therefore always present on the read, holding the SSM service-linked role ARN when the caller supplied none. `TaskParameters` is a (deprecated, sensitive) map that the read reports as the empty map when the task registered none. `CutoffBehavior` is an optional enum: the recording shows it ABSENT for a task registered without it, so the read does not synthesize a value for it (recorded behavior only for the absence).
Emulator: HTTP 200 with the task record carrying the right WindowId, WindowTaskId, TaskArn, Type, Targets, Priority, MaxConcurrency and MaxErrors, but with NO `ServiceRoleArn` and NO `TaskParameters` member, and with an extra `CutoffBehavior: 'CONTINUE_TASK'` the registration never supplied.
Why: a client reading `task['ServiceRoleArn']` to discover which role the window task will assume — the normal pre-flight permission check, and what IaC drift detection compares against — gets a KeyError/None for every task registered without an explicit role, and a client reading `task['TaskParameters']` likewise. The fabricated CutoffBehavior makes an unset field look explicitly configured, so a round-trip through describe-then-update writes a value the caller never chose.
Fix: the DescribeMaintenanceWindowTasks projection (and the RegisterTaskWithMaintenanceWindow state it reads) — default ServiceRoleArn to the SSM service-linked role ARN at registration and project it, project an absent TaskParameters as the empty map, and project CutoffBehavior only when the registration set it.
Related open issue, which does NOT carry this fixture step's replay key: 20261119T230444Z-ssm-getmaintenancewindowtask-omits-the-defaulted-servicerolearn-and-taskparameters.md states the same two omissions for the sibling single-task read `GetMaintenanceWindowTask`; the list read and the extra CutoffBehavior are not named there, and the two reads should be fixed from one projection.
Traces: ssm-maintenance-window-lifecycle#task#8.

Cause 2 of 2: DescribeParameters echoes the parameter Name with a leading '/' the caller never supplied.
AWS: Smithy (ssm): DescribeParameters output {Parameters: ParameterMetadataList, NextToken}; `ParameterMetadata$Name` targets PSParameterName (@length 1..2048) and is documented simply "The parameter name." Recorded behavior: the name reported back is the name as the caller created it — a parameter created with an unqualified (non-hierarchical) name is listed with that exact name and with an ARN of the form arn:aws:ssm:<region>:<account>:parameter/<name>. AWS does not normalize an unqualified name into a hierarchical one.
Emulator: HTTP 200 whose single `Parameters` element carries `Name` with a '/' prepended to the created name, while the sibling `ARN` member is built from the un-prefixed name — so the two members of the same record disagree about what the parameter is called. (The `LastModifiedDate` difference in the same diff is a per-run clock value and is NOT part of this finding.)
Why: a client that feeds the listed `Name` straight back into GetParameter/PutParameter/DeleteParameter, or that joins the listing against names it created or against the ARN's last segment, misses or mis-targets the parameter — the leading slash makes it a different name. It also makes the name inconsistent with the ARN in the very same record.
Fix: the DescribeParameters projection, or the parameter store's name normalization that feeds it — persist and report the name exactly as supplied, and derive the ARN from that same stored name so the two members cannot disagree.
Related open issue, which does NOT carry this fixture step's replay key: 20261116T133835Z-ssm-parameter-history-name-echo-and-patch-filter.md reports a name-echo divergence on `GetParameterHistory`; the same normalization is likely shared, but that issue does not replay this step.
Traces: ssm-parameter-unlabel-partial-semantics#task#7.

## Evidence
Cause 1 — fixture `ssm-maintenance-window-lifecycle`, step `task#8`, DescribeMaintenanceWindowTasks [mismatch, status 200 -> 200]: `$.Tasks[0]` expected `{'WindowId': 'mw-<id>', 'WindowTaskId': '<uuid>', 'TaskArn': 'AWS-RunShellScript', 'Type': 'RUN_COMMAND', 'Targets': [{'Key': 'WindowTargetIds', 'Values': ['<target id>']}], 'Priority': 1, 'MaxConcurrency': '1', 'MaxErrors': '1', 'ServiceRoleArn': 'arn:aws:iam::<account>:role/aws-service-role/ssm.amazonaws.com/AWSServiceRoleForAmazonSSM', 'TaskParameters': {}}`, actual the same record MINUS `ServiceRoleArn` and `TaskParameters` and PLUS `'CutoffBehavior': 'CONTINUE_TASK'`. (The window id, task id and target id are per-run values; the two missing members and the one fabricated member are the finding.)

Cause 2 — fixture `ssm-parameter-unlabel-partial-semantics`, step `task#7`, DescribeParameters [mismatch, 200 -> 200]: `$.Parameters[0]` expected `Name` = `<created name>` with `ARN` = `arn:aws:ssm:<region>:<account>:parameter/<created name>`; actual `Name` = `/<created name>` with the SAME `ARN` = `arn:aws:ssm:<region>:<account>:parameter/<created name>` — the leading '/' appears on Name only. Version (2), Type, Tier, Policies and DataType all match; the LastModifiedDate difference is a per-run clock value and is excluded.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1 — fixture `ssm-maintenance-window-lifecycle`, step `task#8`: CreateMaintenanceWindow, RegisterTargetWithMaintenanceWindow, RegisterTaskWithMaintenanceWindow(TaskArn='AWS-RunShellScript', TaskType='RUN_COMMAND', Targets=[{Key:'WindowTargetIds',Values:[<target id>]}], Priority=1, MaxConcurrency='1', MaxErrors='1') with NO ServiceRoleArn, NO TaskParameters and NO CutoffBehavior; then DescribeMaintenanceWindowTasks(WindowId=<mw id>). Observe the returned task has no ServiceRoleArn and no TaskParameters, and carries CutoffBehavior='CONTINUE_TASK'.
3. Cause 2 — fixture `ssm-parameter-unlabel-partial-semantics`, step `task#7`: PutParameter with an unqualified name (no leading '/'), overwrite it to version 2, label/unlabel a version, then DescribeParameters filtered to that parameter. Observe the listed `Name` carries a leading '/' while the listed `ARN` does not.
