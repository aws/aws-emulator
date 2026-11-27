Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed ecs__events__sqs.xsvc-ecs-1-unpullable-image-task-configuration-failure-not-delivered-to-sibling
Issue-Key: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed|_settle_rule#11|Timeout|time.sleep(interval_s)
Issue-Key: ecs__events__sqs.xsvc-ecs-1-unpullable-image-task-configuration-failure-not-delivered-to-sibling|_receive#30|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Commit: a54ade7ee60c9fa02ca1ca5cbef236b2fbd7f50e

# ecs__events__sqs: 2 net-new causes — 'ECS Deployment State Change' and 'ECS Service Action' events are never observable, so deployment-progress and task-configuration-failure routing reach no verdict

> _Note: this issue was identified against the code base at commit `a54ade7ee60c9fa02ca1ca5cbef236b2fbd7f50e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Two net-new causes, both in the ECS service-level event surface (the already-open ecs__events__sqs issues all concern 'ECS Task State Change' only — e.g. 20261122T102325Z-ecs-events-sqs-stoptask-publishes-no-further-ecs.md, 20261120T181150Z-ecs-events-sqs-ecs-task-state-change-events.md — none of them covers the two detail-types below, and neither carries these checks' replay keys).

Cause 1 of 2: the 'ECS Deployment State Change' event family and the circuit-breaker deployment lifecycle behind it are never observable (failing check: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed).
AWS: a service deployment publishes events with source 'aws.ecs' and detail-type 'ECS Deployment State Change' to the default bus: detail.eventName SERVICE_DEPLOYMENT_IN_PROGRESS and SERVICE_DEPLOYMENT_COMPLETED with detail.eventType 'INFO', each naming detail.deploymentId equal to the id DescribeServices reports for the PRIMARY deployment; for a service with deploymentCircuitBreaker enabled, a deployment whose image cannot be pulled is driven to deployments[].rolloutState 'FAILED' with a rolloutStateReason that says the circuit breaker failed it, a SERVICE_DEPLOYMENT_FAILED event with detail.eventType 'ERROR' naming that failed deploymentId is published, and with rollback enabled the PRIMARY deployment returns to the last good task definition with rolloutState COMPLETED. Smithy models CreateService/UpdateService/DescribeServices and the deploymentConfiguration.deploymentCircuitBreaker member, but says nothing about eventing: the event envelope above is documented/recorded behavior (AmazonECS developer guide, ecs_service_deployment_events).
Emulator: the check is terminated by the harness ('Check timed out after 60.0s') and reported as a mismatch, having reached no assertion — neither the healthy deployment's IN_PROGRESS/COMPLETED delivery (the positive control for the wiring), nor the circuit-breaker FAILED rolloutState, nor the SERVICE_DEPLOYMENT_FAILED delivery, nor the rollback is observable.
Why: a client that gates a rollout on deployment events never sees a deployment start, complete or fail, and a failed rollout is indistinguishable from one still in progress; automated rollback verification hangs instead of failing.
Fix: the service deployment state machine (rolloutState/rolloutStateReason and circuit-breaker rollback) and the ECS → EventBridge emitter for detail-type 'ECS Deployment State Change' (eventName, eventType, deploymentId, resources = the service ARN).

Cause 2 of 2: the 'ECS Service Action' event family is never observable, so a task-configuration failure is neither delivered to its own service's rule nor withheld from a healthy sibling's (failing check: ecs__events__sqs.xsvc-ecs-1-unpullable-image-task-configuration-failure-not-delivered-to-sibling).
AWS: a service whose image tag does not exist publishes source 'aws.ecs', detail-type 'ECS Service Action' with detail.eventName SERVICE_TASK_CONFIGURATION_FAILURE, detail.eventType 'ERROR', detail.clusterArn the cluster's ARN, resources exactly [the failing service's ARN], and detail.reason beginning 'CannotPullContainerError' and naming the unpullable image tag; a healthy sibling service publishes SERVICE_STEADY_STATE with detail.eventType 'INFO' and resources exactly [its own service ARN]. Because resources is scoped per service, a rule scoped to the healthy service must receive its SERVICE_STEADY_STATE and never the sibling's failure, even when the rule's detail.eventName filter lists SERVICE_TASK_CONFIGURATION_FAILURE. Smithy says nothing about eventing — documented/recorded behavior only.
Emulator: the check is terminated by the harness ('Check timed out after 60.0s') and reported as a mismatch with no assertion reached: no SERVICE_TASK_CONFIGURATION_FAILURE and no SERVICE_STEADY_STATE arrive on any of the three rules, so the negative (the near-miss rule staying silent) cannot be distinguished from a bus that publishes nothing.
Why: a client relying on ECS Service Action events gets no alert that a service cannot pull its image and no steady-state signal for a healthy one; cross-service routing cannot be validated at all, because silence is produced for every rule.
Fix: the service scheduler path that records a task-configuration failure (CannotPullContainerError) and steady state, and the ECS → EventBridge emitter for detail-type 'ECS Service Action' (eventName, eventType, clusterArn, reason, and per-service resources scoping).

Traces: ecs__events__sqs.ecs-deployment-state-change-events-deliver-in-progress-then-failed (cause 1); ecs__events__sqs.xsvc-ecs-1-unpullable-image-task-configuration-failure-not-delivered-to-sibling (cause 2).

## Evidence
Cause 1 — expected (AWS): with a FARGATE service created with deploymentCircuitBreaker {enable:true, rollback:true}, the queue behind a rule scoped to the service ARN receives envelopes detail-type 'ECS Deployment State Change' with detail.eventName SERVICE_DEPLOYMENT_IN_PROGRESS and SERVICE_DEPLOYMENT_COMPLETED (both eventType INFO) whose detail.deploymentId equals the PRIMARY deployment id from DescribeServices; after UpdateService to a revision with an unpullable image tag, DescribeServices reports that new deployment with rolloutState FAILED and a rolloutStateReason mentioning the circuit breaker, a SERVICE_DEPLOYMENT_FAILED envelope with eventType ERROR naming that deploymentId is delivered, and the PRIMARY deployment returns to the good revision with rolloutState COMPLETED.
Actual (emulator): 'Error: Check timed out after 60.0s' — mismatch, no assertion reached; no deployment event delivered and no FAILED/rolled-back deployment observable.

Cause 2 — expected (AWS): the unpullable service's own rule receives detail-type 'ECS Service Action' with detail.eventName SERVICE_TASK_CONFIGURATION_FAILURE, detail.eventType 'ERROR', detail.clusterArn = arn:aws:ecs:<region>:<account>:cluster/<cluster>, resources = [arn:aws:ecs:<region>:<account>:service/<cluster>/<bad-service>] and detail.reason starting with 'CannotPullContainerError' and naming the missing image tag; the healthy sibling's rule receives only SERVICE_STEADY_STATE (eventType INFO, resources = [its own service ARN]); the near-miss rule, scoped to the healthy service but filtering eventName on the failure, receives no failure event.
Actual (emulator): 'Error: Check timed out after 60.0s' — mismatch, no assertion reached; none of the three queues ever receives an 'ECS Service Action' envelope, so neither the positive nor the negative leg has a verdict.

## Steps to Reproduce
Cause 1:
1. ecs create-cluster; register two revisions of one family, one with image busybox:latest and one with busybox:sim2real-no-such-tag.
2. events put-rule with pattern {"source":["aws.ecs"],"detail-type":["ECS Deployment State Change"],"resources":["arn:aws:ecs:<region>:<account>:service/<cluster>/<service>"]} and put-targets to an SQS queue with an events.amazonaws.com send policy.
3. ecs create-service --launch-type FARGATE --desired-count 1 --deployment-configuration 'deploymentCircuitBreaker={enable=true,rollback=true},minimumHealthyPercent=100,maximumPercent=200' with the good revision.
4. Poll describe-services until the PRIMARY deployment rolloutState is COMPLETED, then receive-message: SERVICE_DEPLOYMENT_IN_PROGRESS and SERVICE_DEPLOYMENT_COMPLETED (INFO) naming that deployment id must be present.
5. ecs update-service --task-definition <bad revision>; poll describe-services for the new PRIMARY, then for rolloutState FAILED with a circuit-breaker rolloutStateReason; receive-message: SERVICE_DEPLOYMENT_FAILED (ERROR) naming that deployment id; poll until PRIMARY is the good revision again.

Cause 2:
1. ecs create-cluster; create three rules on pattern {"source":["aws.ecs"],"detail-type":["ECS Service Action"]} scoped by resources to the bad service ARN, the good service ARN, and the good service ARN plus detail.eventName in [SERVICE_TASK_CONFIGURATION_FAILURE, SERVICE_STEADY_STATE]; target each at its own SQS queue with a send policy.
2. Register two revisions of one family: busybox:latest and busybox:sim2real-no-such-tag.
3. ecs create-service twice (FARGATE, desiredCount 1, deploymentCircuitBreaker disabled): one on the bad revision, one on the good revision.
4. Poll the three queues: the bad service's queue must receive SERVICE_TASK_CONFIGURATION_FAILURE (ERROR, clusterArn set, reason starting 'CannotPullContainerError' and naming the bad tag, resources = [bad service ARN]); both good-scoped queues must receive SERVICE_STEADY_STATE (INFO, resources = [good service ARN]) and no failure event. Observe instead that nothing is delivered and the scenario is cut off with no verdict.
