Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__ecs.ecs-capacity-provider-scales-asg-and-places-task
Issue-Key: ssm|GetParameter|ParameterNotFound
Issue-Commit: 2233db37534078619f28c37e0e1db9dccbe7de65

# autoscaling__ec2__ecs: SSM GetParameter answers ParameterNotFound for the AWS-published ECS-optimized AMI public parameter, so the ECS capacity-provider scaling check cannot start

> _Note: this issue was identified against the code base at commit `2233db37534078619f28c37e0e1db9dccbe7de65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Cause 1 of 1: the emulator's SSM Parameter Store does not serve the AWS-published public parameter hierarchy under /aws/service/ecs/, so a read of the ECS-optimized AMI pointer is refused instead of resolving to an AMI id. The scenario never gets past its first step: no launch template, no ASG, no ECS capacity provider, no managed-scaling observation.

AWS: GetParameter is a read of the AWS-published, account-agnostic public parameter namespace. Any caller in any account can read /aws/service/ecs/optimized-ami/amazon-linux-2/recommended/image_id and gets back a Parameter whose Value is the current ECS-optimized AMI id for the region, i.e. a string matching ^ami-[0-9a-f]+$ (the sibling .../recommended parameter carries a JSON document with an "image_id" member). Smithy: GetParameter.Name @required, target PSParameterName @length 1..2048; modeled errors InternalServerError, InvalidKeyId, ParameterNotFound, ParameterVersionNotFound. ParameterNotFound is the refusal for a parameter that does not exist — for an AWS-published public parameter AWS never raises it, because the parameter always exists. AWS publishes no Message for ParameterNotFound, so the empty message body is NOT the divergence here; the divergence is that the call is refused at all. Recorded behavior only for the exact set of names published under /aws/service/ecs/optimized-ami/.

Emulator: GetParameter(Name='/aws/service/ecs/optimized-ami/amazon-linux-2/recommended/image_id') is refused with error code ParameterNotFound (modeled httpError 400 / awsQueryError httpResponseCode 404) and an empty message, exactly as if the caller had asked for a parameter it never created. Nothing under the AWS-published /aws/service/ecs/ path is served.

Why: a client resolves the current ECS-optimized AMI from this public parameter precisely so it never hard-codes a region-bound, perpetually-stale AMI id — this is the documented, standard way to launch an ECS container instance, and it is what CloudFormation's AWS::SSM::Parameter::Value<AWS::EC2::Image::Id> template parameters and `resolve:ssm:` ImageId references read behind the scenes. Against the emulator every such client dies on its first call with a not-found for a parameter that exists in every real region, and any ECS capacity-provider / ASG-backed-cluster workflow is unreachable.

Fix: the SSM GetParameter handler's read path and the parameter-store state that backs it — the AWS-published /aws/service hierarchy must be seeded and served (read-only, not account-owned) alongside the account's own parameters, including the /aws/service/ecs/optimized-ami/<variant>/recommended and .../recommended/image_id names.

Related but not covering: open issue 20261103T215115Z-ssm-validationexception-constraint-text-documentalreadyexists-wording-the-cloudformation.md names the same gap for GetParametersByPath on /aws/service/ami-amazon-linux-latest (its cause 5, under service ssm) and open issues 20261103T173026Z-ssm-public-aws-published-aws-service-parameter-hierarchy-is.md and 20261103T154947Z-ssm-getparameter-cannot-resolve-aws-published-public-aws.md describe it from other corpora; none of them carries this check id or the /aws/service/ecs/ names, and the suite did not mark this check as owned by any of them.

Traces: autoscaling__ec2__ecs.ecs-capacity-provider-scales-asg-and-places-task.

## Evidence
Replay of simparity-behavioral-aws-v1. The check is listed under '### autoscaling__ec2__ecs' in the suite report.

[error] autoscaling__ec2__ecs.ecs-capacity-provider-scales-asg-and-places-task:
  botocore.errorfactory.ParameterNotFound: An error occurred (ParameterNotFound) when calling the GetParameter operation:
  (raised out of the check's _ssm_ami helper, whose docstring states it reads 'an AMI id from an AWS-published SSM public parameter, so the check never carries a hard-coded, region-bound, perpetually-stale AMI id')

Expected: a 200 whose Parameter.Value is the region's current ECS-optimized AMI id (a string beginning 'ami-'); the helper then asserts value.startswith('ami-').
Actual: error code ParameterNotFound, empty Message, no Parameter in the response.

The refused name is '/aws/service/ecs/optimized-ami/amazon-linux-2/recommended/image_id'. The request value in the message template is the requested Name; AWS publishes no fixed Message text for ParameterNotFound, so the only requirement violated is the refusal itself.

The check never reaches its subject (ECS managed scaling raising the ASG's DesiredCapacity from 0 to >=1 and placing the queued task), because the launch template cannot be built without the AMI id.

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (GetParameter input Name @required PSParameterName @length 1..2048; errors InternalServerError, InvalidKeyId, ParameterNotFound, ParameterVersionNotFound).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay simparity-behavioral-aws-v1 and re-run the check id in trace_ids.
3. Directly: call ssm.get_parameter(Name='/aws/service/ecs/optimized-ami/amazon-linux-2/recommended/image_id'). Expect a 200 whose Parameter.Value starts with 'ami-'; the emulator refuses with ParameterNotFound.
4. Also call ssm.get_parameter(Name='/aws/service/ecs/optimized-ami/amazon-linux-2/recommended') and ssm.get_parameters_by_path(Path='/aws/service/ecs/optimized-ami', Recursive=True) — AWS serves the published hierarchy; the emulator serves nothing.
5. With the AMI id resolved, the rest of the check proceeds: create a launch template with that ImageId plus ECS_CLUSTER user data, create an ASG min 0 / max 1 / desired 0, create an ECS capacity provider with managedScaling ENABLED over that ASG, attach it to the cluster, queue an EC2 task, and assert DesiredCapacity goes 0 -> >=1 and the task reaches RUNNING.
