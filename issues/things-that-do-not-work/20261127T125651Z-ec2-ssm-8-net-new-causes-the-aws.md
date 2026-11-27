Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.automation-create-tags-step-without-resource-type-tags-a-vpc-and-its-security-group ec2__ssm.ssm-automation-run-command-step-runs-on-a-managed-host ec2__ssm.create-tags-runbook-step-with-ec2-dry-run-input-rejected ec2__ssm.ssm-own-command-document-runs-on-a-managed-host ec2__ssm.ssm-run-command-resolves-a-parameter-store-reference-before-the-host-runs-it ec2__ssm.ssm-describe-parameters-in-a-not-opted-in-region-rejected ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer ec2__ssm.ssm-send-command-refuses-values-outside-the-document-parameter-constraints
Issue-Key: ec2__ssm.automation-create-tags-step-without-resource-type-tags-a-vpc-and-its-security-group|run#15|AssertionError|assert ex["AutomationExecutionStatus"] == "Success", (
Issue-Key: ec2__ssm.ssm-automation-run-command-step-runs-on-a-managed-host|run#11|AssertionError|assert ex["AutomationExecutionStatus"] == "Success", (
Issue-Key: ec2__ssm.create-tags-runbook-step-with-ec2-dry-run-input-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: ec2__ssm.ssm-own-command-document-runs-on-a-managed-host|_poll#15|AssertionError|raise AssertionError(
Issue-Key: ec2__ssm.ssm-run-command-resolves-a-parameter-store-reference-before-the-host-runs-it|run#12|AssertionError|assert inv["StandardOutputContent"].strip() == value, (
Issue-Key: ec2__ssm.ssm-describe-parameters-in-a-not-opted-in-region-rejected|run#12|AssertionError|raise AssertionError(f"DescribeParameters in the not-opted-in Region {region!r} answered {body!r}")
Issue-Key: ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal|_legs#5|AssertionError|assert l1[_LISTEN_PORT]["outcome"] == "timeout", (
Issue-Key: ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides|_legs#5|AssertionError|assert l1[_LISTEN_PORT]["outcome"] == "timeout", (
Issue-Key: ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.ssm-send-command-refuses-values-outside-the-document-parameter-constraints|_poll#12|Timeout|time.sleep(interval_s)
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2__ssm: 8 net-new causes - the aws:createTags/aws:runCommand automation actions are unimplemented, CreateDocument skips input validation, Run Command never settles or resolves {{ssm:}} references, SSM answers a not-opted-in Region, and blocked packets surface as ENETUNREACH

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationExecution

## Description
Eight independent net-new causes across SSM automation, Run Command and the EC2/SSM data plane. Each must be closed before submitting.

Cause 1 of 8: the `aws:createTags` and `aws:runCommand` automation actions are not implemented.
AWS: recorded behavior only for the runtime. An Automation document whose step is `aws:createTags` tags the named resources (ResourceIds, optional ResourceType, Tags) and the execution ends `Success`; a step that is `aws:runCommand` sends the named document to the targets, waits for the invocation and ends `Success` carrying its output.
Emulator: both executions end `Failed` with an emulator-internal message no AWS service emits: `The aws:createTags action of step labelNetwork runs where this emulator does not reach, so nothing ran for this step` and `The aws:runCommand action of step rc runs where this emulator does not reach, so nothing ran for this step` (the step names come from the document).
Why: these are two of the most used Automation actions; a runbook using either fails outright, and the failure reason is not an AWS error a client can branch on - it names an emulator limitation.
Fix: implement both actions in the SSM Automation step executor, dispatching through the EC2 CreateTags and SSM SendCommand surfaces the emulator already serves.
Traces: ec2__ssm.automation-create-tags-step-without-resource-type-tags-a-vpc-and-its-security-group, ec2__ssm.ssm-automation-run-command-step-runs-on-a-managed-host.

Cause 2 of 8: CreateDocument accepts an `aws:createTags` step carrying an input the action does not define.
AWS: `aws:createTags` accepts only ResourceIds, ResourceType and Tags. CreateDocument refuses a document whose step passes any other input - here EC2 CreateTags' `DryRun` - with error code `InvalidDocumentContent` and the message `Unknown input names provided. Known=[ [ResourceIds, ResourceType, Tags] ], Provided=[ [ResourceIds, Tags, DryRun] ].` The fixed text is `Unknown input names provided. Known=[ [` / `] ], Provided=[ [` / `] ].`; the two lists are the action's known inputs and the inputs the request actually supplied, in the order supplied.
Emulator: CreateDocument answers HTTP 200 and returns a DocumentDescription with Status `Creating`, leaving an invalid document registered.
Why: the document's error is caught at author time on AWS; accepted here, it only fails later at execution (or silently ignores DryRun), and a client validating runbooks in CI gets a green result for a document AWS rejects.
Fix: CreateDocument's content validation - validate each step's inputs against the action's input schema.
Traces: ec2__ssm.create-tags-runbook-step-with-ec2-dry-run-input-rejected.

Cause 3 of 8: SendCommand against a customer-created Command document never leaves Pending.
AWS: recorded behavior only. SendCommand naming a document the account created (DocumentType Command) runs on the managed node and the per-instance invocation reaches a terminal status (Success/Failed) with ResponseCode, ExecutionStartDateTime/EndDateTime and StandardOutputContent populated.
Emulator: the invocation stays `{'Status': 'Pending', 'StatusDetails': 'Pending', 'ResponseCode': -1, 'ExecutionStartDateTime': '', 'ExecutionElapsedTime': '', 'ExecutionEndDateTime': '', 'StandardOutputContent': ''}` until the check's budget expires (~48 s).
Why: a client that sends a command and waits for a terminal status waits forever; own-document Run Command is unusable and nothing distinguishes "still running" from "never scheduled".
Fix: the SendCommand handler and invocation state machine for customer-owned Command documents - drive the invocation to a terminal status.
Traces: ec2__ssm.ssm-own-command-document-runs-on-a-managed-host.

Cause 4 of 8: Run Command does not resolve `{{ssm:<name>}}` Parameter Store references before the host runs the command.
AWS: recorded behavior only. Systems Manager substitutes an `{{ssm:<parameter name>}}` reference in a document's parameter values with the parameter's current value before the command is dispatched, so the host prints the resolved value.
Emulator: the host receives and prints the reference literally - stdout is `{{ssm:/sim2real-behav-rcref-599907fa073d}}` where the resolved value was expected.
Why: the reference form is the documented way to keep secrets and environment values out of SendCommand calls; unresolved, the command operates on the literal text `{{ssm:...}}` and every script built on it mis-runs.
Fix: the SendCommand parameter-resolution step - expand `{{ssm:<name>}}` against Parameter Store before dispatching the invocation.
Traces: ec2__ssm.ssm-run-command-resolves-a-parameter-store-reference-before-the-host-runs-it.

Cause 5 of 8: SSM answers a request addressed to a Region the account has not opted into.
AWS: recorded behavior only (an account/credential rule, not a modelled SSM error). A request signed for a Region whose opt-in-status is `not-opted-in` is refused with HTTP 403, code `UnrecognizedClientException`, message `The security token included in the request is invalid.`
Emulator: DescribeParameters against `ap-east-1` answers HTTP 200 with `{'Parameters': []}`.
Why: a client sweeping Regions for parameters, or probing which Regions it may call, reads every not-opted-in Region as enabled-and-empty and reports clean for Regions it cannot actually call.
Fix: the SSM request path's Region admission - refuse a not-opted-in signed Region with 403 UnrecognizedClientException before dispatch. (The same defect is filed for ECS as 20261127T105620Z-ec2-ecs-ecs-answers-requests-sent-to-a.md; that issue does not cover SSM.)
Traces: ec2__ssm.ssm-describe-parameters-in-a-not-opted-in-region-rejected.

Cause 6 of 8: a packet a security group or a missing route should silently drop is answered with ICMP unreachable, so the guest sees ENETUNREACH instead of a connect timeout.
AWS: recorded behavior only. A TCP connect to a destination whose security group has no matching ingress rule, or whose traffic leaves by the internet gateway because no peering route exists, is dropped with no reply - the client's connect TIMES OUT. The distinction matters because a refusal and a drop mean different diagnoses.
Emulator: both cases return `{'outcome': 'oserror', 'errno': 101, 'msg': '[Errno 101] Network is unreachable'}` - the network layer answers unreachable rather than dropping.
Why: every network-truth diagnosis a client makes inverts: ENETUNREACH reads as "no route configured on my side" while a drop reads as "blocked by a security group / missing peering route", so operators chase the wrong resource, and application retry logic that distinguishes timeout from unreachable takes the wrong branch.
Fix: the VPC data-plane forwarding/filtering path - drop a packet with no reply when a security group does not admit it or no route matches, instead of emitting an unreachable.
Traces: ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal, ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides.

Cause 7 of 8: the security-group-plus-prefix-list resolution flow read back through the Reachability Analyzer never settles.
AWS: recorded behavior only. A security group rule referencing a managed prefix list admits the prefix list's members, and a Reachability Analyzer path analysis over the same pair reaches a terminal status whose verdict agrees with the data plane.
Emulator: the check is killed at its budget - `Check timed out after 60.0s` - with no verdict from either side.
Why: a client that gates a change on an analysis result, or on a prefix-list-backed rule taking effect, never gets an answer.
Fix: the managed-prefix-list resolution in security-group evaluation and the Reachability Analyzer analysis state machine - both must settle.
Traces: ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer.

Cause 8 of 8: SendCommand's refusal of values outside a document's parameter constraints never settles.
AWS: recorded behavior only. A document declaring parameter constraints (allowedValues/allowedPattern) causes SendCommand to refuse a value outside them with HTTP 400 `InvalidParameters`, while an in-constraint value runs to a terminal invocation.
Emulator: the check is killed at its budget - `Check timed out after 60.0s` - so neither the refusal nor the control run is observable.
Why: a client cannot rely on SendCommand to validate its document's own parameter constraints, and the call it expects to fail fast instead hangs.
Fix: SendCommand's document-parameter validation, and the invocation state machine the control leg waits on.
Traces: ec2__ssm.ssm-send-command-refuses-values-outside-the-document-parameter-constraints.

## Evidence
Cause 1 - expected AutomationExecutionStatus 'Success'; actual 'Failed' with 'The aws:createTags action of step labelNetwork runs where this emulator does not reach, so nothing ran for this step' and 'The aws:runCommand action of step rc runs where this emulator does not reach, so nothing ran for this step'.
Cause 2 - expected ('InvalidDocumentContent', 'Unknown input names provided. Known=[ [ResourceIds, ResourceType, Tags] ], Provided=[ [ResourceIds, Tags, DryRun] ].'); actual HTTP 200 with DocumentDescription Status 'Creating'.
Cause 3 - expected a terminal invocation; actual {'Status': 'Pending', 'StatusDetails': 'Pending', 'ResponseCode': -1, 'ExecutionStartDateTime': '', 'ExecutionEndDateTime': '', 'StandardOutputContent': ''} until the budget expired (~48 s).
Cause 4 - expected stdout 'resolved-65970231947b'; actual stdout '{{ssm:/sim2real-behav-rcref-599907fa073d}}\n'.
Cause 5 - expected ('UnrecognizedClientException', 'The security token included in the request is invalid.') HTTP 403; actual "DescribeParameters in the not-opted-in Region 'ap-east-1' answered {'Parameters': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".
Cause 6 - expected outcome 'timeout'; actual {'port': 8080, 'outcome': 'oserror', 'errno': 101, 'msg': '[Errno 101] Network is unreachable'} for both the no-ingress-rule leg and the no-peering leg.
Cause 7 - expected a settled prefix-list/analyzer verdict; actual 'Check timed out after 60.0s'.
Cause 8 - expected InvalidParameters for the out-of-constraint value and a terminal invocation for the in-constraint control; actual 'Check timed out after 60.0s'.

## Steps to Reproduce
Cause 1: CreateDocument (Automation, schemaVersion 0.3) with an aws:createTags step over a VPC and its security group, and another document with an aws:runCommand step targeting a managed instance; StartAutomationExecution and poll to terminal -> 'Failed' with the quoted messages.
Cause 2: CreateDocument (Automation) whose aws:createTags step inputs are ResourceIds, Tags and DryRun -> expect InvalidDocumentContent, observe 200.
Cause 3: CreateDocument (DocumentType=Command) and SendCommand it at an SSM-managed AL2023 instance; poll GetCommandInvocation -> stays Pending.
Cause 4: PutParameter a value, SendCommand AWS-RunShellScript with a commands value containing {{ssm:<name>}}, poll to terminal and read StandardOutputContent -> the literal reference.
Cause 5: DescribeRegions(AllRegions=True, opt-in-status=not-opted-in), build an ssm client for one of those Regions and call DescribeParameters -> 200.
Cause 6: two instances, one with no ingress rule admitting the other (and separately two VPCs with no peering route); from the guest, TCP connect to port 8080 -> ENETUNREACH instead of timeout.
Cause 7: create a managed prefix list, reference it from a security group rule, and run a Reachability Analyzer path analysis over the pair -> never settles.
Cause 8: CreateDocument with a parameter carrying allowedValues, SendCommand with a value outside them and then with one inside -> never settles.
