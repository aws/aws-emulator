Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.automation-create-tags-step-without-resource-type-tags-a-vpc-and-its-security-group ec2__ssm.ssm-automation-run-command-step-runs-on-a-managed-host ec2__ssm.create-tags-runbook-step-with-ec2-dry-run-input-rejected ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides ec2__ssm.ssm-describe-parameters-in-a-not-opted-in-region-rejected ec2__ssm.ssm-own-command-document-runs-on-a-managed-host ec2__ssm.ssm-send-command-refuses-values-outside-the-document-parameter-constraints ec2__ssm.ssm-run-command-resolves-a-parameter-store-reference-before-the-host-runs-it ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer
Issue-Key: ec2__ssm.automation-create-tags-step-without-resource-type-tags-a-vpc-and-its-security-group|run#15|AssertionError|assert ex["AutomationExecutionStatus"] == "Success", (
Issue-Key: ec2__ssm.ssm-automation-run-command-step-runs-on-a-managed-host|run#11|AssertionError|assert ex["AutomationExecutionStatus"] == "Success", (
Issue-Key: ec2__ssm.create-tags-runbook-step-with-ec2-dry-run-input-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal|_legs#5|AssertionError|assert l1[_LISTEN_PORT]["outcome"] == "timeout", (
Issue-Key: ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides|_legs#5|AssertionError|assert l1[_LISTEN_PORT]["outcome"] == "timeout", (
Issue-Key: ec2__ssm.ssm-describe-parameters-in-a-not-opted-in-region-rejected|run#12|AssertionError|raise AssertionError(f"DescribeParameters in the not-opted-in Region {region!r} answered {body!r}")
Issue-Key: ec2__ssm.ssm-own-command-document-runs-on-a-managed-host|_poll#15|AssertionError|raise AssertionError(
Issue-Key: ec2__ssm.ssm-send-command-refuses-values-outside-the-document-parameter-constraints|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.ssm-run-command-resolves-a-parameter-store-reference-before-the-host-runs-it|run#12|AssertionError|assert inv["StandardOutputContent"].strip() == value, (
Issue-Key: ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer|<lambda>#5|Timeout|sent = _call_with_retry(lambda: ssm.send_command(
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ec2__ssm: 7 causes — aws:createTags/aws:runCommand automation actions unimplemented, {{ssm:}} references unresolved in Run Command, blocked packets report ENETUNREACH instead of timing out, and DescribeParameters answers in a Region that is not opted in

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationExecution

## Description
Seven net-new causes where SSM meets EC2.

Cause 1 of 7: the aws:createTags and aws:runCommand automation actions are not executed.
AWS: an aws:createTags step tags the named resources (ResourceType may be omitted and is inferred) and an aws:runCommand step sends the document to the named managed nodes; both steps run and the execution ends Success.
Emulator: the execution ends `Failed` with `The aws:createTags action of step <step> runs where this emulator does not reach, so nothing ran for this step` (and the same sentence for aws:runCommand). Fixed text is that sentence; the action and step names are the variable parts.
Why: runbooks that tag resources or run commands — the two most common Automation actions — always fail.
Fix: the Automation action dispatcher and these two action implementations (services/service-com-amazonaws-ssm-amazonssm/src/automation/…).
Traces: ec2__ssm.automation-create-tags-step-without-resource-type-tags-a-vpc-and-its-security-group, ec2__ssm.ssm-automation-run-command-step-runs-on-a-managed-host.

Cause 2 of 7: CreateDocument accepts an aws:createTags step carrying an input the action does not take.
AWS: a runbook whose aws:createTags step declares an EC2 DryRun input is refused at CreateDocument (HTTP 400); the document is not stored.
Emulator: returns 200 with the full DocumentDescription (Status 'Creating', DocumentType 'Automation', SchemaVersion '0.3').
Why: an invalid runbook is stored and startable, so the caller never learns the step is malformed.
Fix: the schemaVersion 0.3 action-input validation in ops/create_document.rs.
Traces: ec2__ssm.create-tags-runbook-step-with-ec2-dry-run-input-rejected.

Cause 3 of 7: a packet a security group or a missing route drops is answered ENETUNREACH instead of being dropped.
AWS: a TCP connect to an address that no security-group rule admits, or that has no peering route, is silently discarded — the client's connect TIMES OUT; it never gets an ICMP/stack-level unreachable.
Emulator: the guest's connect fails immediately with `{'outcome': 'oserror', 'errno': 101, 'msg': '[Errno 101] Network is unreachable'}`.
Why: 'dropped' and 'unreachable' are different signals: client code (and every network-truth diagnosis) distinguishes a timeout from a refusal, so the emulator teaches the opposite of AWS's behaviour.
Fix: the virtual network's drop path for packets with no admitting rule/route (EC2 networking in the emulator's guest stack).
Traces: ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal, ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides.

Cause 4 of 7: DescribeParameters answers normally in a Region the account has not opted into.
AWS: a call to an opt-in Region that is not enabled for the account is refused (HTTP 4xx, an opt-in/UnrecognizedClient class error); it does not return a result set.
Emulator: DescribeParameters in `ap-east-1` answers 200 with `{'Parameters': []}`.
Why: a client reads an empty-but-successful inventory for a Region it cannot actually use, so a mis-targeted call looks like 'nothing there'.
Fix: the Region/opt-in gate in the request router before the SSM handlers.
Traces: ec2__ssm.ssm-describe-parameters-in-a-not-opted-in-region-rejected.

Cause 5 of 7: a command sent to a real managed node never reaches a terminal status, and document parameter constraints are never checked.
AWS: SendCommand against a registered managed node moves the invocation to a terminal status (Success/Failed) with ResponseCode and timestamps set; a parameter value outside the document's declared constraints is refused at SendCommand time.
Emulator: GetCommandInvocation keeps reporting `Status: 'Pending', StatusDetails: 'Pending', ResponseCode: -1` with empty ExecutionStartDateTime/ElapsedTime, and the constraint-refusal scenario is cut at the harness cap with no refusal observed.
Why: Run Command against a managed node is unusable and its negative path unobservable.
Fix: the command delivery/settlement path for registered managed nodes and the document-parameter validator in ops/send_command.rs.
Traces: ec2__ssm.ssm-own-command-document-runs-on-a-managed-host, ec2__ssm.ssm-send-command-refuses-values-outside-the-document-parameter-constraints.

Cause 6 of 7: `{{ssm:<name>}}` references in SendCommand parameters are not resolved before the node runs the command.
AWS: SSM resolves a Parameter Store reference in a command parameter, so the node prints the parameter's VALUE.
Emulator: the node receives and prints the literal `{{ssm:/<parameter name>}}`.
Why: documents that reference Parameter Store run with the reference text as data — silently wrong output rather than an error.
Fix: the parameter-reference resolution step in ops/send_command.rs before dispatch.
Traces: ec2__ssm.ssm-run-command-resolves-a-parameter-store-reference-before-the-host-runs-it.

Cause 7 of 7: the prefix-list / Reachability Analyzer resolution scenario makes no observable progress.
AWS: a security-group rule that references a managed prefix list resolves to its entries and Reachability Analyzer reports the path.
Emulator: the check is cut at the harness cap (`Check timed out after 60.0s`) with no result. Hypothesis: the prefix-list reference or the analyzer path never settles.
Why: prefix-list-based rules cannot be reasoned about at all.
Fix: the EC2 prefix-list resolution used by security-group evaluation and the Reachability Analyzer path computation.
Traces: ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer.

## Evidence
Cause 1: automation ended 'Failed', expected 'Success': 'The aws:createTags action of step labelNetwork runs where this emulator does not reach, so nothing ran for this step'; 'The aws:runCommand action of step rc runs where this emulator does not reach…'.
Cause 2: 'expected a refusal, but the call succeeded: {DocumentDescription: {Name: sim2real-xsvc-ctdry-…, DocumentType: Automation, SchemaVersion: 0.3, …}, HTTPStatusCode: 200}'.
Cause 3: "L1: with no ingress rule the packet must be dropped and the connect must TIME OUT; got {'port': 8080, 'outcome': 'oserror', 'errno': 101, 'msg': '[Errno 101] Network is unreachable'}" (same for the no-peering leg).
Cause 4: "DescribeParameters in the not-opted-in Region 'ap-east-1' answered {'Parameters': [], HTTPStatusCode: 200}".
Cause 5: last observed {'Status': 'Pending', 'StatusDetails': 'Pending', 'ResponseCode': -1, 'ExecutionStartDateTime': '', …}; the constraint check ended 'Check timed out after 60.0s'.
Cause 6: "the host must print the resolved value 'resolved-…'; stdout was '{{ssm:/sim2real-behav-rcref-…}}\n'".
Cause 7: 'Check timed out after 60.0s'.

## Steps to Reproduce
1. Start a 0.3 runbook with an aws:createTags step (no ResourceType) over a VPC and its security group, and another with an aws:runCommand step against a managed node.
2. CreateDocument for a runbook whose aws:createTags step passes an EC2 DryRun input.
3. From a guest, connect to a peer port with no admitting security-group rule, and to a peer in another VPC with no peering route; observe timeout vs ENETUNREACH.
4. Call DescribeParameters against ap-east-1.
5. SendCommand a custom Command document to a registered managed node and poll GetCommandInvocation; then send a parameter value outside the document's constraints.
6. SendCommand with a parameter value of '{{ssm:/<existing parameter>}}' and read the node's stdout.
7. Create a security-group rule referencing a managed prefix list and run Reachability Analyzer over the path.
