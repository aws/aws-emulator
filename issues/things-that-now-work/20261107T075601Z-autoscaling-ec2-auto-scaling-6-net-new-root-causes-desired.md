Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asgdx-desired-capacity-above-max-rejected autoscaling__ec2.asghc-asg-instance-inherits-template-sg-imdsv2-and-group-tag autoscaling__ec2.asghc-asg-keeps-the-latest-version-literal-while-the-template-moves autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed autoscaling__ec2.asghc-create-asg-with-a-template-sg-from-another-vpc-rejected autoscaling__ec2.asghc-create-asg-with-unknown-health-check-type-rejected
Issue-Key: autoscaling__ec2.asgdx-desired-capacity-above-max-rejected|run#9|AssertionError|assert got == ("ValidationError", "Desired capacity:2 must be between the specified "
Issue-Key: autoscaling__ec2.asghc-asg-instance-inherits-template-sg-imdsv2-and-group-tag|run#17|AssertionError|assert md == {"HttpTokens": "required", "HttpPutResponseHopLimit": 2,
Issue-Key: autoscaling__ec2.asghc-asg-keeps-the-latest-version-literal-while-the-template-moves|run#8|AssertionError|assert ref == {"LaunchTemplateId": lt["id"], "LaunchTemplateName": lt["name"],
Issue-Key: autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds|run#10|AssertionError|assert g["LaunchTemplate"] == {"LaunchTemplateId": lt["id"], "LaunchTemplateName": lt["name"],
Issue-Key: autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed|run#15|AssertionError|assert (code, msg) == ("InvalidLaunchTemplateId.NotFound", want_gone_msg), (
Issue-Key: autoscaling__ec2.asghc-create-asg-with-a-template-sg-from-another-vpc-rejected|_error#11|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: autoscaling__ec2.asghc-create-asg-with-unknown-health-check-type-rejected|run#12|AssertionError|assert msg == want, f"expected message {want!r}, got {msg!r}"
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# Auto Scaling: 6 net-new root causes — desired-capacity refusal wording, launch-template reference resolution, IMDS propagation, cross-VPC SG accepted

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Six net-new root causes behind the surviving autoscaling__ec2 failures. Each must be closed before submitting.

Cause 1 of 6: CreateAutoScalingGroup's DesiredCapacity-above-MaxSize refusal carries emulator-authored prose.
AWS: Smithy models the refusal only as ValidationError (autoscaling common error, HTTP 400); the recorded wording is fixed text with the three request values interpolated: "Desired capacity:<DesiredCapacity> must be between the specified min size:<MinSize> and max size:<MaxSize>". Fixed text = "Desired capacity:", " must be between the specified min size:", " and max size:"; variable parts = the DesiredCapacity/MinSize/MaxSize taken from the request.
Emulator: same code ValidationError, different message — "The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is 2 and the range is [0, 1]".
Why: a client (or CDK/CFN wrapper) that matches on the documented sentence to tell a bounds error from another ValidationError sees an unrecognised string and misclassifies it.
Fix: CreateAutoScalingGroup's input validation / its error-message templates.
Traces: autoscaling__ec2.asgdx-desired-capacity-above-max-rejected.

Cause 2 of 6: launch-template MetadataOptions are dropped on the ASG launch path.
AWS: an instance launched by an ASG from a launch template inherits the template's MetadataOptions; a template declaring HttpTokens=required, HttpPutResponseHopLimit=2 produces an instance whose DescribeInstances MetadataOptions report required/2 (recorded behavior; ec2 Smithy: LaunchTemplateInstanceMetadataOptions / InstanceMetadataOptionsResponse).
Emulator: the launched instance reports the account defaults {"HttpTokens": "optional", "HttpPutResponseHopLimit": 1, "HttpEndpoint": "enabled"}.
Why: an IMDSv2-enforcement check against the fleet passes in the emulator while the equivalent AWS fleet is non-compliant (or vice versa); no scenario that depends on IMDSv2 being required can be exercised.
Fix: the ASG launch path that materialises instances from a launch template version (template-data -> RunInstances translation).
Traces: autoscaling__ec2.asghc-asg-instance-inherits-template-sg-imdsv2-and-group-tag.

Cause 3 of 6: DescribeAutoScalingGroups echoes the launch-template reference instead of resolving it.
AWS: Smithy autoscaling LaunchTemplateSpecification carries LaunchTemplateId, LaunchTemplateName and Version; AWS stores the reference resolved, so a group created by name reads back with BOTH the name and the resolved id (and a group created by id reads back with the resolved name), with the Version literal ($Latest/$Default/number) kept exactly as declared.
Emulator: only the member that was supplied comes back — a by-name group reads {"LaunchTemplateName": ..., "Version": "$Latest"} with no LaunchTemplateId, a by-id group reads {"LaunchTemplateId": ..., "Version": "$Latest"} with no LaunchTemplateName.
Why: tooling that joins a group to its template through the other identifier (the CLI/console view, drift detection, any `--query LaunchTemplate.LaunchTemplateName`) renders None and cannot follow the group to its template.
Fix: the state/serializer for AutoScalingGroup.LaunchTemplate — resolve both identifiers at create/update time.
Traces: autoscaling__ec2.asghc-asg-keeps-the-latest-version-literal-while-the-template-moves, autoscaling__ec2.asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds.

Cause 4 of 6: EC2 reports a well-formed launch-template id that no longer exists as malformed.
AWS: DescribeLaunchTemplates with a syntactically valid lt-<17 hex> id that has been deleted is refused InvalidLaunchTemplateId.NotFound, message "At least one of the launch templates specified in the request does not exist." — Malformed is reserved for ids that do not match the id syntax (recorded behavior; ec2 Smithy models these as request-level errors).
Emulator: answers InvalidLaunchTemplateId.Malformed, "The following launch template ids are malformed: lt-..." for a well-formed deleted id.
Why: a caller that distinguishes "you typed garbage" from "it is gone" (the ASG scale-out-after-template-deleted path does exactly this) takes the wrong branch and never retries after re-pointing the group.
Fix: EC2 launch-template id validation/lookup in the DescribeLaunchTemplates handler (syntax check before existence check, distinct codes).
Traces: autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed.

Cause 5 of 6: CreateAutoScalingGroup accepts a launch template whose security group belongs to another VPC.
AWS: the group's subnets (VPCZoneIdentifier) and the template's security groups must sit in one VPC; AWS refuses the create with ValidationError and creates no group (recorded behavior — the model exposes only ValidationError for this).
Emulator: the call SUCCEEDS (HTTP 200) and the group is created.
Why: a misconfiguration AWS rejects at create time is stored, so the scenario's negative branch is unreachable and the group later "launches" instances that could not exist.
Fix: CreateAutoScalingGroup validation — cross-check template security-group VPC against the subnets' VPC.
Traces: autoscaling__ec2.asghc-create-asg-with-a-template-sg-from-another-vpc-rejected.

Cause 6 of 6: the invalid-HealthCheckType refusal message is emulator-authored.
AWS: Smithy AutoScalingGroup.HealthCheckType is a free string (max 32) validated server-side; an unsupported value is refused ValidationError with the fixed sentence "Valid health check types are: [EC2, ELB, EBS, VPC_LATTICE]."
Emulator: ValidationError with "The HealthCheckType value HTTP is not valid: it must be a comma-separated list of [\"EC2\", \"EBS\", \"ELB\", \"VPC_LATTICE\"]".
Why: the message is the only machine-readable hint of the allowed set; a client parsing it (or asserting on it) sees a different sentence and a different ordering/quoting of the enum.
Fix: CreateAutoScalingGroup/UpdateAutoScalingGroup HealthCheckType validation message.
Traces: autoscaling__ec2.asghc-create-asg-with-unknown-health-check-type-rejected.

## Evidence
From the behavioral replay (verdict/detail per check):
- asgdx-desired-capacity-above-max-rejected [mismatch]: AssertionError: CreateAutoScalingGroup with DesiredCapacity=2 over MaxSize=1 was answered ('ValidationError', 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is 2 and the range is [0, 1]'); expected ('ValidationError', 'Desired capacity:2 must be between the specified min size:0 and max size:1').
- asghc-asg-instance-inherits-template-sg-imdsv2-and-group-tag [mismatch]: AssertionError: IMDS options did not propagate: {'HttpTokens': 'optional', 'HttpPutResponseHopLimit': 1, 'HttpEndpoint': 'enabled'} (template declared required/2).
- asghc-asg-keeps-the-latest-version-literal-while-the-template-moves [mismatch]: AssertionError: the group's template reference: {'LaunchTemplateName': '...', 'Version': '$Latest'} (no LaunchTemplateId).
- asghc-asg-round-trips-elb-health-check-grace-and-capacity-bounds [mismatch]: AssertionError: a template referenced by id must come back with its name resolved and the version literal kept: {'LaunchTemplateId': 'lt-...', 'Version': '$Latest'}.
- asghc-asg-scale-out-after-template-deleted-cancels-until-repointed [mismatch]: AssertionError: EC2 still describes the deleted template: got ('InvalidLaunchTemplateId.Malformed', 'The following launch template ids are malformed: lt-...').
- asghc-create-asg-with-a-template-sg-from-another-vpc-rejected [mismatch]: AssertionError: the call succeeded; real AWS rejects it, so there is no error code.
- asghc-create-asg-with-unknown-health-check-type-rejected [mismatch]: AssertionError: expected message 'Valid health check types are: [EC2, ELB, EBS, VPC_LATTICE].', got 'The HealthCheckType value HTTP is not valid: it must be a comma-separated list of ["EC2", "EBS", "ELB", "VPC_LATTICE"]'.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks named in trace_ids. Each is self-contained: it builds a VPC/subnet/SG, creates a launch template, then exercises one CreateAutoScalingGroup/DescribeAutoScalingGroups path (bounds refusal, IMDS propagation, template-reference round-trip, deleted-template lookup, cross-VPC SG, HealthCheckType).
