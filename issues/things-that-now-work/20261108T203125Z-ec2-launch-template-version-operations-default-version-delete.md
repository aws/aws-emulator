Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asgssh-launch-template-default-version-delete-rejected ec2.launch-template-version-deletion-default-protected ec2.ltver-launch-naming-a-nonexistent-template-version-rejected ec2.op-contract-describe-launch-template-versions ec2.op-contract-delete-launch-template ec2.asgeni-launch-template-latest-and-default-versions-diverge
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.asgssh-launch-template-default-version-delete-rejected|run#9|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: ec2.launch-template-version-deletion-default-protected|run#9|AssertionError|assert default_code == "InvalidParameterValue", (
Issue-Key: ec2.ltver-launch-naming-a-nonexistent-template-version-rejected|run#9|AssertionError|assert code == "InvalidLaunchTemplateId.VersionNotFound", (
Issue-Key: ec2.op-contract-describe-launch-template-versions|run#19|AssertionError|assert code == "InvalidLaunchTemplateId.Malformed", f"expected Malformed, got {code!r}"
Issue-Key: ec2.op-contract-delete-launch-template|run#18|AssertionError|assert code == "InvalidLaunchTemplateId.NotFound", f"expected NotFound, got {code}"
Issue-Key: ec2.asgeni-launch-template-latest-and-default-versions-diverge|run#13|AssertionError|assert default["LaunchTemplateData"] == v1, (
Issue-Commit: bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c

# EC2 launch-template version operations: default-version delete and RunInstances version resolution answer NotImplemented, id refusals are swapped, and reads inject MetadataOptions.State (5 causes)

> _Note: this issue was identified against the code base at commit `bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLaunchTemplateVersions

## Description
Five net-new root causes in EC2's launch-template surface, grounded in six failing behavioral checks. Each cause is independent and must be closed before the checks pass. EC2 is an ec2Query service and its Smithy operations carry no modeled errors (smithy_query: DeleteLaunchTemplateVersions / DescribeLaunchTemplateVersions / DescribeLaunchTemplates / DeleteLaunchTemplate all report errors: []), so the error codes and messages below are recorded behavior.

Cause 1 of 5: DeleteLaunchTemplateVersions answers NotImplemented instead of refusing the deletion of a template's DEFAULT version.
AWS: recorded behavior only. DeleteLaunchTemplateVersions naming the default version is refused with HTTP 400, error code InvalidParameterValue and the message "The default version cannot be deleted. Either specify another version as default or delete the launch template." That message is entirely fixed text - it carries no request or state value. The refusal has no effect: both versions survive, the default version keeps DefaultVersion true, and the non-default version still deletes cleanly afterwards (reported under SuccessfullyDeletedLaunchTemplateVersions) while the deleted version number is never recycled by a later CreateLaunchTemplateVersion.
Emulator: the call is refused with error code NotImplemented, so the operation never applies its default-version rule at all. Replay details: "deleting the default version must be refused; got 'NotImplemented'" and "expected InvalidParameterValue deleting the default version, got NotImplemented".
Why: a client cannot delete any launch-template version. The documented guard rail is unobservable, and the per-item response shape (SuccessfullyDeletedLaunchTemplateVersions / UnsuccessfullyDeletedLaunchTemplateVersions with ResponseError code launchTemplateVersionDoesNotExist and message "The launch template version does not exist.") is never produced, so version-pruning automation fails outright rather than being told which versions it may not remove.
Fix: the DeleteLaunchTemplateVersions handler and its validation - it is still answering the not-implemented path instead of resolving the versions, rejecting the default one with InvalidParameterValue and reporting the rest per item. The open scaffold-stub issue for this operation (see the suite's ALREADY FILED entry for ec2.op-contract-delete-launch-template-versions, services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_launch_template_versions.rs) names the same operation; these two checks fail on the refusal semantics the implementation must then carry and are not on that issue's replay key.
Traces: ec2.asgssh-launch-template-default-version-delete-rejected, ec2.launch-template-version-deletion-default-protected.

Cause 2 of 5: RunInstances naming a launch-template version that does not exist answers NotImplemented instead of the version-not-found refusal.
AWS: recorded behavior only. RunInstances with LaunchTemplate={LaunchTemplateId: <existing template>, Version: "<n>"} where version n was never created on a template that has fewer versions is refused with HTTP 400 and error code InvalidLaunchTemplateId.VersionNotFound; no instance is launched. The variable part of the refusal is the template id and the version number taken from the request.
Emulator: the call is refused with error code NotImplemented, so the launch-template version is never resolved.
Why: a client cannot distinguish "you named a version that does not exist" (a request defect it must fix) from "this API is unavailable". Any launch that goes through a launch-template reference - which is how Auto Scaling, CloudFormation and the console all launch - cannot be validated, and version-pinning mistakes surface as an unrelated server-side failure.
Fix: the RunInstances handler's launch-template resolution step (resolving LaunchTemplateId/Name plus the Version selector, including $Latest/$Default, before the launch) and the refusal it raises for an unresolvable version. Related open issues name the same NotImplemented answer under other service headings - 20261106T015719Z-ec2-iam-ec2-runinstances-with-a-launchtemplate-answers-notimplemented-so.md and 20261107T075601Z-ec2-kms-runinstances-with-a-launchtemplate-answers-501-notimplemented-so.md - neither carries this check's replay key.
Traces: ec2.ltver-launch-naming-a-nonexistent-template-version-rejected.

Cause 3 of 5: a syntactically well-formed launch-template id that was never minted is refused NotFound where AWS refuses it as Malformed.
AWS: recorded behavior only. DescribeLaunchTemplateVersions(LaunchTemplateId="lt-0123456789abcdef0") - a fabricated id of the right shape - is refused with HTTP 400, error code InvalidLaunchTemplateId.Malformed and the message "Malformed launch template Id: <the id from the request>": the fixed text is "Malformed launch template Id: " and the variable part is the id as submitted. EC2 validates the id before any lookup, so this is Malformed, not NotFound.
Emulator: the same call is refused with error code InvalidLaunchTemplateId.NotFound. Replay detail: "expected Malformed, got 'InvalidLaunchTemplateId.NotFound'".
Why: the two codes mean different things to a client - Malformed says the identifier is invalid and retrying can never help, NotFound says the resource is gone and a create/retry is appropriate. Error-code-driven retry and fallback logic takes the wrong branch, and a typo in an id is reported as a missing resource.
Fix: the launch-template id validation in the DescribeLaunchTemplateVersions handler (the id-format check that must run before the state lookup) and the error it raises.
Traces: ec2.op-contract-describe-launch-template-versions.

Cause 4 of 5: a launch-template id that really existed and was deleted is refused Malformed where AWS refuses it as NotFound - the mirror image of cause 3.
AWS: recorded behavior only. After DeleteLaunchTemplate succeeds, DescribeLaunchTemplates(LaunchTemplateIds=[<that id>]) is refused with HTTP 400, error code InvalidLaunchTemplateId.NotFound and the message "At least one of the launch templates specified in the request does not exist." (entirely fixed text), while DescribeLaunchTemplateVersions(LaunchTemplateId=<that id>) and a second DeleteLaunchTemplate on it are refused InvalidLaunchTemplateId.NotFound with "The specified launch template, with template ID <id>, does not exist." - fixed text around the template id taken from the request.
Emulator: DescribeLaunchTemplates on the deleted id is refused with error code InvalidLaunchTemplateId.Malformed. Replay detail: "expected NotFound, got InvalidLaunchTemplateId.Malformed". The same id was accepted and resolved by DescribeLaunchTemplates moments earlier (before the delete), so the id's format is not the issue - the handler falls back to Malformed when the lookup misses.
Why: deleting a template and then confirming it is gone - the normal teardown-verification step of any script, test harness or CloudFormation delete - reports a malformed-identifier defect instead of a clean not-found, so cleanup code that treats NotFound as success loops or aborts.
Fix: the DescribeLaunchTemplates handler's lookup-miss path (and the shared launch-template id resolution it uses), which must distinguish an id that fails format validation from a well-formed id with no live template behind it.
Traces: ec2.op-contract-delete-launch-template.

Cause 5 of 5: launch-template version reads inject a MetadataOptions.State member the request never set.
AWS: Smithy: the read shape com.amazonaws.ec2#LaunchTemplateInstanceMetadataOptions carries State (enum pending|applied, doc "The state of the metadata option changes"), while the input shape com.amazonaws.ec2#LaunchTemplateInstanceMetadataOptionsRequest has exactly HttpTokens, HttpPutResponseHopLimit, HttpEndpoint, HttpProtocolIpv6, InstanceMetadataTags and NO State member. Recorded behavior: a launch template created with MetadataOptions={HttpTokens: required} reads back from DescribeLaunchTemplateVersions with MetadataOptions == {HttpTokens: required} - version 1's LaunchTemplateData is byte-for-byte what the create request carried, with no State added.
Emulator: DescribeLaunchTemplateVersions returns version 1's LaunchTemplateData with MetadataOptions == {State: "applied", HttpTokens: "required"}. Replay detail: AssertionError "version 1 changed: {'BlockDeviceMappings': [...], 'InstanceType': 't3.micro', 'MetadataOptions': {'State': 'applied', 'HttpTokens': 'required'}}" - the only divergence from the submitted data is the added State. The surrounding assertions ($Latest -> 2, $Default -> 1, DefaultVersion false on version 2, the SourceVersion inheritance) passed.
Why: the documented read -> edit -> CreateLaunchTemplateVersion round trip breaks client-side: handing the returned LaunchTemplateData back to CreateLaunchTemplateVersion is refused by botocore's own validation ("Unknown parameter in LaunchTemplateData.MetadataOptions: State"), so console/CDK/"bump the template" scripts can never derive a new version. Any client that diffs a stored version against its intended data also sees a phantom change.
Fix: the LaunchTemplateData serializer on the launch-template read paths (DescribeLaunchTemplateVersions and the CreateLaunchTemplate / CreateLaunchTemplateVersion responses) must not emit MetadataOptions.State for a launch-template version. Cause 1 of the open issue 20261106T015719Z-autoscaling-ec2-auto-scaling-launch-from-a-launch-template-loses.md describes the same defect, but it is filed under the autoscaling__ec2 heading and its replay key names three other checks, not this one.
Traces: ec2.asgeni-launch-template-latest-and-default-versions-diverge.

## Evidence
Check [mismatch] ec2.asgssh-launch-template-default-version-delete-rejected
  Expected (AWS): DeleteLaunchTemplateVersions(LaunchTemplateId=LT, Versions=["1"]) on a template whose version 1 is the default -> error code InvalidParameterValue, message "The default version cannot be deleted. Either specify another version as default or delete the launch template."; versions [(1, True), (2, False)] unchanged.
  Actual (emulator): error code NotImplemented.

Check [mismatch] ec2.launch-template-version-deletion-default-protected
  Expected (AWS): same refusal as above, then version 99 reported per item with ResponseError code launchTemplateVersionDoesNotExist / message "The launch template version does not exist.", and version 2 deleted cleanly.
  Actual (emulator): "expected InvalidParameterValue deleting the default version, got NotImplemented".

Check [mismatch] ec2.ltver-launch-naming-a-nonexistent-template-version-rejected
  Expected (AWS): RunInstances naming version "3" of a two-version launch template -> error code InvalidLaunchTemplateId.VersionNotFound, no instance launched.
  Actual (emulator): error code NotImplemented.

Check [mismatch] ec2.op-contract-describe-launch-template-versions
  Expected (AWS): DescribeLaunchTemplateVersions(LaunchTemplateId="lt-0123456789abcdef0") -> error code InvalidLaunchTemplateId.Malformed, message "Malformed launch template Id: lt-0123456789abcdef0" (fixed prefix + the id from the request).
  Actual (emulator): error code InvalidLaunchTemplateId.NotFound.

Check [mismatch] ec2.op-contract-delete-launch-template
  Expected (AWS): after DeleteLaunchTemplate succeeds, DescribeLaunchTemplates(LaunchTemplateIds=[<the deleted id>]) -> error code InvalidLaunchTemplateId.NotFound, message "At least one of the launch templates specified in the request does not exist."
  Actual (emulator): error code InvalidLaunchTemplateId.Malformed. (The id is a per-run value; the requirement is that a well-formed id with no live template is NotFound.)

Check [mismatch] ec2.asgeni-launch-template-latest-and-default-versions-diverge
  Expected (AWS): DescribeLaunchTemplateVersions(Versions=["$Default"]) returns version 1's LaunchTemplateData identical to the data submitted to CreateLaunchTemplate, i.e. MetadataOptions == {HttpTokens: required} with no State member.
  Actual (emulator): MetadataOptions == {State: "applied", HttpTokens: "required"} - a member the request shape cannot carry.

## Steps to Reproduce
1. ec2 CreateLaunchTemplate(LaunchTemplateName=NAME, LaunchTemplateData={InstanceType: t3.micro, MetadataOptions: {HttpTokens: required}, BlockDeviceMappings: [{DeviceName: /dev/xvda, Ebs: {VolumeSize: 100, VolumeType: gp3, DeleteOnTermination: true}}]}) -> LT.
2. ec2 CreateLaunchTemplateVersion(LaunchTemplateId=LT, SourceVersion="1", LaunchTemplateData={InstanceType: t3.small}).
3. ec2 DescribeLaunchTemplateVersions(LaunchTemplateId=LT, Versions=["$Default"]) and compare LaunchTemplateData with step 1's data.
   Observed: MetadataOptions carries an extra State: "applied" (Cause 5). Feeding that data back into CreateLaunchTemplateVersion is refused locally by botocore.
4. ec2 DeleteLaunchTemplateVersions(LaunchTemplateId=LT, Versions=["1"]) - version 1 is the default.
   Observed: error code NotImplemented; AWS refuses with InvalidParameterValue and "The default version cannot be deleted. Either specify another version as default or delete the launch template." (Cause 1).
5. ec2 DescribeLaunchTemplateVersions(LaunchTemplateId="lt-0123456789abcdef0") - a fabricated but well-formed id.
   Observed: InvalidLaunchTemplateId.NotFound; AWS answers InvalidLaunchTemplateId.Malformed with "Malformed launch template Id: lt-0123456789abcdef0" (Cause 3).
6. ec2 DeleteLaunchTemplate(LaunchTemplateId=LT), then ec2 DescribeLaunchTemplates(LaunchTemplateIds=[LT]).
   Observed: InvalidLaunchTemplateId.Malformed; AWS answers InvalidLaunchTemplateId.NotFound with "At least one of the launch templates specified in the request does not exist." (Cause 4).
7. ec2 RunInstances(LaunchTemplate={LaunchTemplateId: <a template with 2 versions>, Version: "3"}, MinCount=1, MaxCount=1).
   Observed: error code NotImplemented; AWS answers InvalidLaunchTemplateId.VersionNotFound (Cause 2).
