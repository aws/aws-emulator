Issue-Source: trace-sweep-divergence
Issue-Trace: codedeploy-deployment-group-onprem-tag-filter-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codedeploy-deployment-group-onprem-tag-filter-lifecycle
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.bakeTime
Issue-Key: codedeploy|GetDeploymentGroup|2xx->2xx|$.deploymentGroupInfo.managedHookConfig
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# CodeDeploy GetDeploymentGroup omits bakeTime and managedHookConfig from deploymentGroupInfo

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeploymentGroup

## Description
1 cause.

Cause 1 of 1: GetDeploymentGroup's deploymentGroupInfo projection drops the bakeTime and managedHookConfig members entirely, so an EC2/On-premises deployment group read back after creation and after an on-premises tag-filter update is missing two members AWS always populates.

AWS: recorded behavior only. The checked-in Smithy model (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json) declares DeploymentGroupInfo with applicationName, deploymentGroupId, deploymentGroupName, deploymentConfigName, ec2TagFilters, onPremisesInstanceTagFilters, autoScalingGroups, serviceRoleArn, targetRevision, triggerConfigurations, alarmConfiguration, autoRollbackConfiguration, deploymentStyle, outdatedInstancesStrategy, blueGreenDeploymentConfiguration, loadBalancerInfo, lastSuccessfulDeployment, lastAttemptedDeployment, ec2TagSet, onPremisesTagSet, computePlatform, ecsServices, ec2DeploymentOptions and terminationHookEnabled — it declares NEITHER bakeTime NOR managedHookConfig, and no ManagedHookConfig shape exists in it (smithy_query shape=ManagedHookConfig -> "shape 'ManagedHookConfig' not found"). The model vintage therefore predates both members. The recording shows the live service returning them on every GetDeploymentGroup for a group that was created without specifying them, as service-supplied defaults: deploymentGroupInfo.bakeTime is present with the default numeric value (0 when no bake time is configured) and deploymentGroupInfo.managedHookConfig is present as a structure whose executionRoleArn carries the sentinel for "no managed-hook execution role" (the string NONE when the group has no managed hook role). Both are unconditional in the recording: they appear on the read directly after CreateDeploymentGroup and again on the read after UpdateDeploymentGroup changed only onPremisesInstanceTagFilters, so they are not derived from anything in those requests. Status is 200 on both sides and every other member of deploymentGroupInfo matches, so the divergence is purely these two absent members.

Emulator: replay reports, at $.deploymentGroupInfo.bakeTime, "missing (expected 0, got None)" and, at $.deploymentGroupInfo.managedHookConfig, "missing (expected {'executionRoleArn': 'NONE'}, got None)" — i.e. the keys are absent from the serialized body rather than present with a wrong value. The same two keys are absent at both task#5 (read after create) and task#7 (read after the on-premises tag-filter update), so this is the projector's fixed field set, not a state-tracking slip for one particular group.

Why: a client that reads a deployment group's configuration back from the emulator sees two members its AWS counterpart always provides. Anything that dereferences deploymentGroupInfo.managedHookConfig.executionRoleArn to decide whether a managed hook role is configured gets a null/absent structure and either faults or silently takes the "no such field" branch instead of reading the NONE sentinel; anything that reads deploymentGroupInfo.bakeTime as a number (comparing it, summing it into a timeout, rendering it) gets null instead of the configured bake time, so a group with no bake time is indistinguishable from a group the API never described. Round-trip tooling that reads a group and feeds the result back into UpdateDeploymentGroup, and config-drift checks that diff an emulator read against a recorded AWS read, both report a difference that does not exist in the real service.

Fix: two parts, in this service's state projection and its model. The model's DeploymentGroupInfo shape needs the bakeTime member and a managedHookConfig member (with its ManagedHookConfig structure carrying executionRoleArn) added so the output schema can carry them at all, and the deployment-group projector that builds DeploymentGroupInfo must populate both for every group — bakeTime with the group's configured bake time, defaulting to 0 when none was configured, and managedHookConfig with the group's managed-hook execution role, defaulting to the NONE sentinel when no role is configured — rather than leaving them unset. The projector is crate::state::deployment_group_info in services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/state.rs, called from services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_deployment_group.rs:179; the same projection must stay consistent wherever DeploymentGroupInfo is also returned (BatchGetDeploymentGroups) so a group does not describe itself differently per operation. Note for the fixer: the already-open issue 20261125T205710Z-codedeploy-batchgetdeploymentgroups-returns-an-empty-errormessage-for-unretrievable.md touches BatchGetDeploymentGroups' errorMessage and does NOT cover these two missing DeploymentGroupInfo members.

Traces: codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#5 and codedeploy-deployment-group-onprem-tag-filter-lifecycle#task#7 (one divergence cluster, 2 affected steps).

## Evidence
Fixture codedeploy-deployment-group-onprem-tag-filter-lifecycle, step task#5 (representative; affected also task#7), operation GetDeploymentGroup, verdict mismatch.

Expected status 200, actual status 200 — the body diverges, not the status.

Replay detail, verbatim:
  $.deploymentGroupInfo.bakeTime: missing (expected 0, got None)
  $.deploymentGroupInfo.managedHookConfig: missing (expected {'executionRoleArn': 'NONE'}, got None)

Expected (AWS, recorded): deploymentGroupInfo contains bakeTime as a number — 0 being the default for a group with no bake time configured — and managedHookConfig as a structure containing executionRoleArn, whose value is the fixed sentinel string "NONE" when the group has no managed-hook execution role. The member names bakeTime, managedHookConfig and executionRoleArn and the sentinel "NONE" are fixed text; the bake-time number and any real role ARN are values taken from the group's own configuration/state.

Actual (emulator): both keys absent from deploymentGroupInfo. No error code and no error message are involved — the response is a successful 200 whose payload is short two members.

Model evidence: smithy_query(query="shape", shape="DeploymentGroupInfo") on services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json lists no bakeTime and no managedHookConfig member; smithy_query(query="shape", shape="ManagedHookConfig") returns "shape 'ManagedHookConfig' not found". GetDeploymentGroup output is {deploymentGroupInfo: DeploymentGroupInfo}, SUCCESS_STATUS 200.

Divergence key: service=codedeploy|GetDeploymentGroup|status=200->200|keys=$.deploymentGroupInfo.bakeTime,$.deploymentGroupInfo.managedHookConfig

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture codedeploy-deployment-group-onprem-tag-filter-lifecycle. It creates a CodeDeploy application and an EC2/On-premises deployment group with on-premises instance tag filters, reads the group back, updates the on-premises tag filters, and reads it back again.
3. At step task#5 (GetDeploymentGroup immediately after CreateDeploymentGroup), inspect the 200 response body under deploymentGroupInfo: the emulator omits bakeTime and managedHookConfig, where AWS returns bakeTime (0 when none configured) and managedHookConfig = {"executionRoleArn": "NONE"}.
4. At step task#7 (GetDeploymentGroup after UpdateDeploymentGroup changed only onPremisesInstanceTagFilters), the same two keys are again absent, confirming the projector never emits them for any group rather than losing them on update.
5. Equivalently by hand: CreateDeploymentGroup for any application without specifying a bake time or a managed hook role, then GetDeploymentGroup for it, and diff deploymentGroupInfo's key set against the recorded AWS response.
