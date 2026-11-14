Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.deploy-cfn-service-desired-count-materializes-tasks ecs.deep-ecs-2-ec2place-draining-instance-relocates-service-task-to-other-host
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.deploy-cfn-service-desired-count-materializes-tasks|_deploy_cfn#8|WaiterError|waiter.wait(StackName=stack_name, WaiterConfig={"Delay": 2, "MaxAttempts": 450})
Issue-Key: ecs.deep-ecs-2-ec2place-draining-instance-relocates-service-task-to-other-host|_poll_until#2|Timeout|last = probe()
Issue-Commit: 965b3053fd6a88603311dd441a6858d4fb538678

# ecs: 2 net-new causes — a CloudFormation stack of Cluster+TaskDefinition+Service rolls back instead of reaching CREATE_COMPLETE, and a DRAINING container instance never relocates its service task

> _Note: this issue was identified against the code base at commit `965b3053fd6a88603311dd441a6858d4fb538678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Two net-new root causes in the ECS surface. Both are grounded in failing checks the suite lists under `### ecs` and neither check is marked ALREADY FILED or OWNED by an open issue in this run. BOTH must be closed before submitting.

Cause 1 of 2: a CloudFormation stack that declares an AWS::ECS::Cluster, an AWS::ECS::TaskDefinition and an AWS::ECS::Service (LaunchType FARGATE, NetworkMode awsvpc, DesiredCount 2) terminates in ROLLBACK_COMPLETE instead of CREATE_COMPLETE, so the service is never created and the data-plane assertion about its tasks is unreachable.
  AWS:      a template whose only resources are AWS::ECS::Cluster {ClusterName}, AWS::ECS::TaskDefinition {Family, RequiresCompatibilities [FARGATE], NetworkMode awsvpc, Cpu '256', Memory '512', one essential ContainerDefinition} and AWS::ECS::Service {Cluster: !Ref Cluster, ServiceName, TaskDefinition: !Ref TaskDef, DesiredCount, LaunchType FARGATE, NetworkConfiguration.AwsvpcConfiguration {Subnets, SecurityGroups, AssignPublicIp ENABLED}} with Outputs !GetAtt Cluster.Arn, !GetAtt Service.Name and !Ref TaskDef creates cleanly: the stack reaches CREATE_COMPLETE, and the service's DesiredCount materializes that many RUNNING tasks observable through ListTasks/DescribeTasks. Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113): CreateService input requires only `serviceName`; `cluster`, `taskDefinition`, `desiredCount`, `launchType` and `networkConfiguration` are all optional members and this request sets valid values for each; CreateService returns HTTP 200 with `service`, and its refusals are AccessDeniedException (403), ClientException / ClusterNotFoundException / InvalidParameterException / NamespaceNotFoundException / PlatformTaskDefinitionIncompatibilityException / PlatformUnknownException / UnsupportedFeatureException (400) and ServerException (500) — none of which applies to this request. The ECS service-linked role AWSServiceRoleForECS is created and visible before the stack is deployed, so 'Unable to assume the service linked role' is excluded. Recorded behavior only for the stack transition: the recording reaches CREATE_COMPLETE.
  Emulator: the stack is driven to ROLLBACK_COMPLETE. botocore's StackCreateComplete waiter fails immediately: `botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "ROLLBACK_COMPLETE" at least once`. Some resource in the stack is refused or never signals CREATE_COMPLETE, and the whole stack is rolled back; the check never gets as far as ListTasks, so no per-resource status, error code or message is established by this check beyond the terminal stack status.
  Why:      a client that provisions an ECS service through CloudFormation — the ordinary way ECS services are deployed — gets no service at all and no usable diagnosis: the stack succeeds in AWS and is destroyed in the emulator. Every downstream behaviour (the service's DesiredCount materializing tasks, GetAtt Service.Name / Cluster.Arn outputs, the stack-delete teardown of the service and its tasks) is unobservable, and a stack that rolls back is indistinguishable to the client from a genuinely invalid template.
  Fix:      the CloudFormation resource providers for AWS::ECS::Cluster, AWS::ECS::TaskDefinition and AWS::ECS::Service (resource creation, the !Ref/!GetAtt attributes `Cluster.Arn`, `Service.Name` and the TaskDefinition's Ref) together with the ECS CreateService handler and its validation they call into: a valid Fargate/awsvpc service declaration must create and reach CREATE_COMPLETE rather than failing and rolling the stack back. This is NOT the cause carried by the open 20261114T080626Z-cloudformation-ecs-2-net-new-causes-describeservices-omits.md or 20261111T111216Z-cloudformation-ecs-2-net-new-causes-aws-ecs-cluster-in.md: in both of those the stack reaches CREATE_COMPLETE and the divergence is in the read-back; here the stack never creates at all, so neither covers this check.
  Traces:   ecs.deploy-cfn-service-desired-count-materializes-tasks.

Cause 2 of 2: putting a container instance into DRAINING never relocates the ECS service's task onto the other host — the check cannot progress and is cut at its budget.
  AWS:      UpdateContainerInstancesState(cluster, containerInstances=[<ci>], status=DRAINING) returns HTTP 200 with the updated instance and an empty `failures` (Smithy: input requires `containerInstances` and `status`; output {containerInstances, failures}; modeled errors AccessDeniedException 403, ClientException/ClusterNotFoundException/InvalidParameterException 400, ServerException 500). The service scheduler then stops the task running on the draining instance with `stopCode: ServiceSchedulerInitiated`, starts a REPLACEMENT task (a new taskArn) on the remaining ACTIVE container instance, the service returns to runningCount 1 / pendingCount 0, and the drained instance stays `status: DRAINING` with runningTasksCount 0 and pendingTasksCount 0. Recorded behavior only for the timing; the model gives the shapes and the error set.
  Emulator: nothing converges — the check is cut with `Error: Check timed out after 60.0s` while still waiting on its first condition (the service's single task reaching RUNNING on a registered EC2 container instance, before the DRAINING call is even issued). No status, error code or message is produced; the call simply never settles.
  Why:      a client draining a host for maintenance gets neither a relocation nor a refusal. Capacity cannot be rotated: the task is never stopped with ServiceSchedulerInitiated, no replacement is scheduled, and the drained instance's running/pending counts never fall to zero, so a drain loop that waits for them hangs forever.
  Fix:      whatever stands an emulator-launched EC2 instance up as an ACTIVE, agentConnected ECS container instance, plus the ECS service scheduler's drain path (UpdateContainerInstancesState -> stop the instance's service tasks with stopCode ServiceSchedulerInitiated, reschedule onto another ACTIVE instance, keep the drained instance DRAINING with zero running/pending tasks). The open 20261109T013920Z-ecs-4-net-new-causes-ec2-launched-hosts.md describes the same EC2-host-registration plane and lists this check id in its prose, but it does NOT carry this run's replay key for it (it is keyed on `_poll_until#2|Timeout|time.sleep(interval_s)`, whereas this run fails with the harness-level `Check timed out after 60.0s`), which is why the suite did not mark this check OWNED. Fixing that issue will very likely close this cause too — replay the id below to confirm rather than closing it unrun.
  Traces:   ecs.deep-ecs-2-ec2place-draining-instance-relocates-service-task-to-other-host.

## Evidence
Cause 1 — ecs.deploy-cfn-service-desired-count-materializes-tasks
  Expected (recorded AWS): `cloudformation create-stack` with the Cluster/TaskDefinition/Service template above and Parameters {ClusterName, Family, ServiceName, Subnet1, SecurityGroupId, DesiredCount=2} reaches StackStatus CREATE_COMPLETE; `ecs list-tasks --cluster <cluster> --service-name <service> --desired-status RUNNING` then returns 2 task ARNs and `ecs describe-tasks` reports both with lastStatus RUNNING.
  Actual (emulator):
    File "context.py", line 123, in wrapper
      return func(*args, **kwargs)
    File "waiter.py", line 378, in wait
      raise WaiterError(
    botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "ROLLBACK_COMPLETE" at least once
  The stack name, cluster name, family, service name, subnet id and security-group id are per-run values; the requirement is that this resource set creates (CREATE_COMPLETE) instead of rolling back. The waiter fires on the first ROLLBACK_COMPLETE observation, so no ECS call after create-stack is reached.

Cause 2 — ecs.deep-ecs-2-ec2place-draining-instance-relocates-service-task-to-other-host
  Expected (recorded AWS): on a two-host EC2 cluster running an EC2-launchType service with desiredCount 1,
    1) `ecs update-container-instances-state --cluster <c> --container-instances <ci-A> --status DRAINING` -> HTTP 200, `failures` empty;
    2) `ecs list-tasks --cluster <c> --service-name <s> --desired-status RUNNING` + describe-tasks -> exactly 1 RUNNING task whose containerInstanceArn is <ci-B> and whose taskArn differs from the original;
    3) `ecs describe-services` -> runningCount 1, pendingCount 0;
    4) `ecs describe-tasks` on the original task -> lastStatus STOPPED with stopCode `ServiceSchedulerInitiated` and containerInstanceArn <ci-A>;
    5) `ecs describe-container-instances --container-instances <ci-A>` -> runningTasksCount 0, pendingTasksCount 0, status DRAINING.
  Actual (emulator):
    Error: Check timed out after 60.0s
  The body's first wait is `_poll_until(_running_tasks, lambda ts: len(ts) == 1, timeout_s=300, ...)` — "the service's single task reaches RUNNING" — so the run is cut before the DRAINING call is issued and none of the five expectations is reached. Container-instance ARNs, task ARNs and cluster/service names are per-run values; the never-converging drain is the divergence.

## Steps to Reproduce
Cause 1:
1. Ensure the ECS service-linked role exists: `iam create-service-linked-role --aws-service-name ecs.amazonaws.com`, then poll `iam get-role --role-name AWSServiceRoleForECS` until it resolves.
2. Pick a VPC subnet (MapPublicIpOnLaunch preferred) and the VPC's `default` security group.
3. `cloudformation create-stack --stack-name <s> --capabilities CAPABILITY_IAM CAPABILITY_NAMED_IAM --template-body <the Cluster + TaskDefinition(FARGATE/awsvpc, Cpu 256, Memory 512, one essential container) + Service(LaunchType FARGATE, DesiredCount 2, AwsvpcConfiguration{Subnets:[<subnet>], SecurityGroups:[<sg>], AssignPublicIp: ENABLED}) template, Outputs !GetAtt Cluster.Arn / !GetAtt Service.Name / !Ref TaskDef> --parameters ClusterName=<c>,Family=<f>,ServiceName=<svc>,Subnet1=<subnet>,SecurityGroupId=<sg>,DesiredCount=2`.
4. Poll `cloudformation describe-stacks --stack-name <s>`: the emulator reaches ROLLBACK_COMPLETE; AWS reaches CREATE_COMPLETE. Inspect `cloudformation describe-stack-events --stack-name <s>` to see which of the three resources is refused.
5. On AWS, `ecs list-tasks --cluster <c> --service-name <svc> --desired-status RUNNING` then returns 2 ARNs and `ecs describe-tasks` reports both RUNNING.
Replay: ecs.deploy-cfn-service-desired-count-materializes-tasks.

Cause 2:
1. `ecs create-cluster --cluster-name <c>`; create an ECS instance role/profile (AmazonEC2ContainerServiceforEC2Role) and launch TWO t3.micro instances with user data `echo ECS_CLUSTER=<c> >> /etc/ecs/ecs.config`; wait for both to appear as ACTIVE, agentConnected container instances.
2. `ecs register-task-definition` with one 64 CPU / 64 MiB busybox container running `sh -c "sleep 1800"`.
3. `ecs create-service --cluster <c> --service-name <s> --task-definition <td> --desired-count 1 --launch-type EC2 --deployment-configuration minimumHealthyPercent=100,maximumPercent=200`; wait for its single task to be RUNNING and note its containerInstanceArn <ci-A> and taskArn.
4. `ecs update-container-instances-state --cluster <c> --container-instances <ci-A> --status DRAINING` -> must be accepted with no failures.
5. Poll until exactly 1 RUNNING task exists on the other host <ci-B> with a NEW taskArn and the service reports runningCount 1 / pendingCount 0; the original task must read back STOPPED with stopCode ServiceSchedulerInitiated on <ci-A>; <ci-A> must report runningTasksCount 0, pendingTasksCount 0 and status DRAINING. In the emulator the run never gets past step 3's RUNNING wait and is cut at the harness budget.
Replay: ecs.deep-ecs-2-ec2place-draining-instance-relocates-service-task-to-other-host.
