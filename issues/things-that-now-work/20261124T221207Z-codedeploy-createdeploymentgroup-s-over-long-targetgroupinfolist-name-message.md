Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected|run#17|AssertionError|assert tg_message == _TARGET_GROUP_TOO_LONG_MESSAGE.format(name=too_long), tg_message
Issue-Key: codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact|_poll#27|Timeout|val = fn()
Issue-Commit: 6f44d1835fbd492fdd033592217b9af14d15284a

# CodeDeploy: CreateDeploymentGroup's over-long targetGroupInfoList name message is reworded, and the DEPLOYMENT_FAILURE auto-rollback check still cannot start (2 causes)

> _Note: this issue was identified against the code base at commit `6f44d1835fbd492fdd033592217b9af14d15284a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeploymentGroup

## Description
Two distinct root causes in the codedeploy emulator, both grounded in failing behavioral checks. A fixer must close BOTH before submitting.

=== Cause 1 of 2: CreateDeploymentGroup rejects an over-long target group name with the right code but AWS-divergent message text ===
Failing check: codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected

AWS: CreateDeploymentGroup validates the SYNTAX of each loadBalancerInfo.targetGroupInfoList[].name at create time (it does not resolve the target group's existence). Smithy (com.amazonaws.codedeploy#CodeDeploy_20141006): CreateDeploymentGroup input member loadBalancerInfo -> LoadBalancerInfo.targetGroupInfoList -> TargetGroupInfo.name -> TargetGroupName; modeled error InvalidLoadBalancerInfoException (errorKind client, HTTP 400, doc "An invalid load balancer name, or no load balancer name, was specified"). The model states no length bound, so the message text is recorded behavior only: a name longer than 32 characters is refused with HTTP 400, error code InvalidLoadBalancerInfoException and the message
  "The target group name <name> specified in targetGroupInfoList exceeds the maximum allowed length of 32 characters."
In that message the FIXED text is "The target group name " ... " specified in targetGroupInfoList exceeds the maximum allowed length of 32 characters." (unquoted name, trailing period, the limit 32 spelled inline as part of the fixed text); the only VARIABLE part is <name>, copied verbatim from the request's targetGroupInfoList entry. A name at or under 32 characters is accepted and round-trips even though no such target group exists.

Emulator: the status (400) and the error code (InvalidLoadBalancerInfoException) are correct, but the message is reworded to
  "The target group name '<name>' cannot be longer than '32' characters"
i.e. it quotes the name, quotes the limit, says "cannot be longer than" instead of "specified in targetGroupInfoList exceeds the maximum allowed length of", never names the member the bad value came from, and drops the trailing period. (Observed with a 33-character name; the specific name string is a per-run value, not part of the requirement.)

Why: a client that surfaces or matches on the service message gets text AWS never emits, and loses the member attribution ("specified in targetGroupInfoList") that tells a caller WHICH field of a multi-field loadBalancerInfo was rejected — the error code alone also covers a missing/invalid classic load balancer name, so the message is the only discriminator.

Fix: the CreateDeploymentGroup handler's loadBalancerInfo validation — where the target-group-name length check raises InvalidLoadBalancerInfoException — must emit AWS's message template rather than a generic "cannot be longer than" length-check string.

Traces: codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected (assertion at the 33-character target group name: expected the "specified in targetGroupInfoList exceeds the maximum allowed length of 32 characters." message, got the "cannot be longer than '32' characters" message). Every other assertion in that check — the InvalidLoadBalancerInfoException code itself, the blue/green round-trip and service-supplied defaults, the InvalidDeploymentStyleException and InvalidBlueGreenDeploymentConfigurationException pairings — passed, so this message is the only divergence in it.

=== Cause 2 of 2: the Server-platform auto-rollback check makes no progress; a running EC2 instance never becomes an Online SSM managed node ===
Failing check: codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact
NOTE: open issue 20261121T222346Z-codedeploy-the-deployment-failure-auto-rollback-check-cannot.md describes this same family of host-readiness failure, but the replay report did NOT mark this check as owned by it, so its replay key is not carried by any open issue and it is re-filed here per the sweep rules. If that issue is being worked, close them together; this entry is not a claim of a second, different host-readiness defect.

AWS: a running EC2 instance whose instance profile grants SSM managed-node permissions registers as a Systems Manager managed node, ssm:DescribeInstanceInformation returns an InstanceInformation entry for the instance id with PingStatus 'Online', and SendCommand/GetCommandInvocation run in-guest commands to a terminal Status. On such a host CodeDeploy's Server compute platform behaves as modeled: a deployment group may carry autoRollbackConfiguration{enabled:true, events:['DEPLOYMENT_FAILURE']} (Smithy AutoRollbackConfiguration / AutoRollbackEvent DEPLOYMENT_FAILURE, "The event type or types that trigger a rollback"); when a deployment fails its ValidateService hook the service creates a SEPARATE rollback deployment — GetDeployment on the failed deployment reports rollbackInfo.rollbackDeploymentId, GetDeployment on the rollback reports rollbackInfo.rollbackTriggeringDeploymentId pointing back at it, deploymentInfo.creator is DeploymentCreator 'codeDeployRollback', its revision is the last revision that succeeded, its status 'Succeeded', its instance target's Install lifecycle event 'Succeeded', and the deployed file on the host holds the prior revision's bytes again.

Emulator: the check is cut off by the harness cap with "Error: Check timed out after 60.0s" and no assertion diagnostic, having issued no CodeDeploy operation under test. It blocks in its fleet-readiness gate: the launched instance never appears in DescribeInstanceInformation's InstanceInformationList, so PingStatus 'Online' is never reported and the in-guest command leg is never reached. Hypothesis: a RunInstances-created instance is modeled only in EC2 state and never projected into the SSM managed-node inventory backing DescribeInstanceInformation/SendCommand, so the Server-platform deployment path has no reachable host.

Why: automatic rollback is unobservable on this emulator. A client relying on autoRollbackConfiguration gets neither the rollback record (rollbackInfo, creator 'codeDeployRollback', the re-deployed prior revision) nor its host-visible effect, and the blocking symptom is a silently EMPTY readiness list rather than an error response, so the client's own readiness poll hangs to its deadline instead of failing diagnosably.

Fix: the EC2 instance lifecycle must cross-register a running instance into the SSM managed-node state behind DescribeInstanceInformation (an entry with PingStatus 'Online' once the instance reaches 'running'), the SSM in-guest command plane must drive SendCommand invocations to a terminal Status, and the CodeDeploy Server-platform deployment executor plus the auto-rollback trigger in deployment state must, on a hook failure under a DEPLOYMENT_FAILURE autoRollbackConfiguration, create a distinct rollback deployment with creator 'codeDeployRollback', cross-link both deployments through rollbackInfo, and re-deploy the last successful revision.

Traces: codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact (timed out inside the fleet-readiness poll, before any graded CodeDeploy operation).

## Evidence
Cause 1 — codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected:
Expected (AWS, recorded): CreateDeploymentGroup(applicationName=<Server app>, deploymentStyle={BLUE_GREEN, WITH_TRAFFIC_CONTROL}, blueGreenDeploymentConfiguration=..., loadBalancerInfo={targetGroupInfoList:[{name: <33-character name>}]}) -> HTTP 400, Error.Code 'InvalidLoadBalancerInfoException', Error.Message 'The target group name <name> specified in targetGroupInfoList exceeds the maximum allowed length of 32 characters.' (fixed text as quoted, <name> taken verbatim from the request).
Actual (emulator): HTTP 400 with the correct code, but Error.Message 'The target group name '<name>' cannot be longer than '32' characters' — AssertionError raised at the message comparison (check line 130, `assert tg_message == _TARGET_GROUP_TOO_LONG_MESSAGE.format(name=too_long)`), reporting the emulator text. The code assertion immediately above it passed, so only the wording diverges. A name of 32 characters or fewer naming a non-existent target group is accepted by the emulator, matching AWS.

Cause 2 — codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact:
Expected (AWS): after RunInstances of one tagged instance with an SSM-capable instance profile, ssm:DescribeInstanceInformation(Filters=[{Key:'InstanceIds',Values:[<id>]}]) returns an entry with PingStatus 'Online' and SendCommand invocations reach Status 'Success'; then revision A deploys 'Succeeded' and its marker is on disk; revision B (ValidateService exits 1) installs (Install 'Succeeded') and the deployment ends 'Failed' with ValidateService 'Failed'; GetDeployment(B).deploymentInfo.rollbackInfo.rollbackDeploymentId names a DISTINCT deployment whose creator is 'codeDeployRollback', whose rollbackInfo.rollbackTriggeringDeploymentId is B, whose status is 'Succeeded' and whose revision is revision A; the host file holds A's marker again and the rollback's Install event reads 'Succeeded'.
Actual (emulator): 'Error: Check timed out after 60.0s', no assertion diagnostic and no frame — the run never leaves the readiness poll because InstanceInformationList stays empty for the launched instance, so no CodeDeploy operation under test (CreateDeployment / GetDeployment / GetDeploymentTarget) is ever issued and rollbackInfo, the rollback's creator/status and the restored artifact are never produced. Instance ids, bucket names, marker strings and per-run timings are per-run values and not part of the requirement.

## Steps to Reproduce
Cause 1:
1. codedeploy:CreateApplication(computePlatform='Server').
2. CreateDeploymentGroup with deploymentStyle={deploymentType:'BLUE_GREEN', deploymentOption:'WITH_TRAFFIC_CONTROL'}, a blueGreenDeploymentConfiguration, and loadBalancerInfo={targetGroupInfoList:[{name:'s'*32}]} — observe it is ACCEPTED and round-trips through GetDeploymentGroup even though no such target group exists (matches AWS).
3. Repeat with loadBalancerInfo={targetGroupInfoList:[{name:'s'*33}]} — the call is refused with HTTP 400 / InvalidLoadBalancerInfoException (correct), but the message reads "The target group name '<name>' cannot be longer than '32' characters" instead of AWS's "The target group name <name> specified in targetGroupInfoList exceeds the maximum allowed length of 32 characters.".

Cause 2:
1. Create a VPC with a public subnet, internet gateway and route; create an IAM role with SSM managed-node permissions plus s3 read on a revision bucket, and an instance profile holding it.
2. Upload two zip revisions to S3: A whose appspec.yml installs payload.txt to /opt/sim2real with a ValidateService hook exiting 0, and B identical except ValidateService exits 1 after waiting on a release file.
3. ec2:RunInstances one instance in that subnet with the instance profile and a tag the deployment group filters on; wait for state 'running'.
4. Poll ssm:DescribeInstanceInformation(Filters=[{Key:'InstanceIds',Values:[<the id>]}]) — InstanceInformationList stays empty indefinitely, so PingStatus 'Online' is never reported and the readiness gate never returns (the check is cut off at the 60 s harness cap).
5. (Blocked by step 4, and what the check grades) CreateDeploymentGroup with those ec2TagFilters, deploymentConfigName='CodeDeployDefault.AllAtOnce' and autoRollbackConfiguration={enabled:true, events:['DEPLOYMENT_FAILURE']}; deploy A to 'Succeeded'; deploy B, release its hold, wait for 'Failed'; then GetDeployment(B) for rollbackInfo.rollbackDeploymentId, GetDeployment(<rollback>) for creator 'codeDeployRollback', rollbackTriggeringDeploymentId == B, status 'Succeeded' and revision A, GetDeploymentTarget for the rollback's Install event, and read the deployed file back through SSM expecting A's marker.
