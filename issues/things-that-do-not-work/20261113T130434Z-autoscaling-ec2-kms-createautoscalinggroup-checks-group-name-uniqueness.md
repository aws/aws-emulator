Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates
Issue-Key: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates|run#21|AssertionError|assert code == "ValidationError", (
Issue-Commit: 4a275bd9e1d7ab3b23ca6d4b9d4220e7609e8c33

# autoscaling__ec2__kms: CreateAutoScalingGroup checks group-name uniqueness before the launch-template reference — a request naming both an existing group and a nonexistent LaunchTemplateId is refused AlreadyExists instead of ValidationError "Malformed launch template Id: <id>"

> _Note: this issue was identified against the code base at commit `4a275bd9e1d7ab3b23ca6d4b9d4220e7609e8c33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Cause 1 of 1: CreateAutoScalingGroup evaluates the duplicate-name check before it validates the LaunchTemplate reference, so the wrong refusal wins when a request is invalid in both ways.
Failing check id: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates.
AWS: Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json) models CreateAutoScalingGroup's errors as AlreadyExistsFault (awsQuery error code "AlreadyExists", HTTP 400), LimitExceededFault, ResourceContentionFault and ServiceLinkedRoleFailure; the argument-validation refusal (query error code "ValidationError", HTTP 400) is the service-common one and is not bound per operation, so recorded behavior only for the ordering and the text. The recording shows that the launch-template reference is validated FIRST: a CreateAutoScalingGroup whose LaunchTemplate.LaunchTemplateId names a template that does not exist is refused HTTP 400 ValidationError with the message "Malformed launch template Id: <LaunchTemplateId>" — fixed text "Malformed launch template Id: " followed by the LaunchTemplateId copied from the request — even when the AutoScalingGroupName in the same request is already taken, in which case AWS would otherwise answer AlreadyExists with "AutoScalingGroup by this name already exists - A group with the name <name> already exists". The refused request changes nothing: the existing group keeps its MinSize/DesiredCapacity/MaxSize.
Emulator: the same request is refused with error code "AlreadyExists" (the duplicate-name refusal), so the launch-template reference is never examined; the ValidationError and its "Malformed launch template Id: <id>" message are never produced for this input. The duplicate-name leg of the same check (an existing name over a VALID template) passes, which places the defect in the order of the two validations rather than in either one alone.
Why: a client that provisions by "create and interpret the refusal" branches on the error code — AlreadyExists means "the group is already there, adopt or move on", ValidationError means "my request is malformed, fix the launch-template reference". With the order reversed, a rebuild that passes a stale or mistyped launch-template id is told the group already exists, so the client adopts a group built from a different template, or retries forever against a template id that can never resolve; the real defect in the request is never reported.
Fix: the CreateAutoScalingGroup handler's validation order in the autoscaling service — resolve and validate the LaunchTemplate specification (LaunchTemplateId/LaunchTemplateName and Version) before the AutoScalingGroupName uniqueness check, and emit the ValidationError refusal with the malformed-launch-template-id message when the reference does not resolve. Not covered by the open issue 20261107T081710Z-autoscaling-auto-scaling-accepts-requests-aws-refuses-8-net.md (whose "unknown launch template" cause is about such a request being ACCEPTED, 200, rather than about which of two refusals wins) nor by 20261110T100847Z-autoscaling-refusal-fidelity-wrong-codes-and-emulator-authored.md.
Traces: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates.

## Evidence
Check autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates, verdict mismatch.
Sequence: a launch template is created, a group is created from it, the duplicate-template and duplicate-group refusals are asserted (both pass: InvalidLaunchTemplateName.AlreadyExistsException "Launch template name already in use.", and AlreadyExists "AutoScalingGroup by this name already exists - A group with the name <name> already exists"). Then the same already-used group name is submitted with LaunchTemplate={"LaunchTemplateId": "lt-00000000000000000", "Version": "$Latest"} — a well-formed id that does not exist.
Expected: HTTP 400, error code "ValidationError", message "Malformed launch template Id: lt-00000000000000000" (fixed text plus the request's LaunchTemplateId).
Actual (AssertionError from the check): "a duplicate group name over a nonexistent launch template returned 'AlreadyExists' (expected the launch template validation, not the name validation, to win)".
The check's remaining assertion — that the refused call leaves the existing group's (MinSize, DesiredCapacity, MaxSize) untouched — was never reached.

## Steps to Reproduce
1. ec2:CreateLaunchTemplate with a valid LaunchTemplateData (the check uses an EBS root volume encrypted with a customer CMK) and note LaunchTemplateId.
2. autoscaling:CreateAutoScalingGroup(AutoScalingGroupName=<name>, MinSize=0, MaxSize=2, DesiredCapacity=0, LaunchTemplate={LaunchTemplateId:<id>, Version:"1"}, VPCZoneIdentifier=<subnet>).
3. Repeat step 2 verbatim — expect HTTP 400 AlreadyExists, message "AutoScalingGroup by this name already exists - A group with the name <name> already exists" (this is correct today).
4. autoscaling:CreateAutoScalingGroup with the SAME AutoScalingGroupName but LaunchTemplate={LaunchTemplateId:"lt-00000000000000000", Version:"$Latest"} — expected HTTP 400 ValidationError with message "Malformed launch template Id: lt-00000000000000000". Observed error code AlreadyExists (the duplicate-name refusal).
5. autoscaling:DescribeAutoScalingGroups(AutoScalingGroupNames=[<name>]) — the refused call must not have changed MinSize/DesiredCapacity/MaxSize.
