Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__cloudformation.deploy-job-queue-rejects-priority-above-maximum batch__cloudformation.deploy-job-queue-update-in-place-and-through-replacement
Issue-Key: batch__cloudformation.deploy-job-queue-rejects-priority-above-maximum|run#10|AssertionError|assert status == "CREATE_COMPLETE", f"positive control stack ended {status!r}"
Issue-Key: batch__cloudformation.deploy-job-queue-update-in-place-and-through-replacement|run#20|AssertionError|assert status == "UPDATE_COMPLETE", f"priority update ended {status!r}, expected UPDATE_COMPLETE"
Issue-Commit: 70a3a5b79f16f8a83ae30f7fd1a45ab35d74c677

# batch__cloudformation: 2 net-new causes — a valid AWS::Batch::JobQueue create rolls the stack back, and a Priority-only UpdateStack rolls the update back instead of updating in place

> _Note: this issue was identified against the code base at commit `70a3a5b79f16f8a83ae30f7fd1a45ab35d74c677`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the CloudFormation provisioning of AWS::Batch::JobQueue: a template CloudFormation accepts and deploys on AWS fails here, both on create and on update, so the whole stack is rolled back. Both must be closed before submitting.

Cause 1 of 2 — a stack whose AWS::Batch::JobQueue is entirely within the resource schema fails its create and rolls the whole Batch estate back.
AWS: the AWS::Batch::JobQueue resource schema bounds Priority at 1000, so a template declaring AWS::Batch::SchedulingPolicy (QuotaSharePolicy.IdleResourceAssignmentStrategy FIFO) + AWS::Batch::ServiceEnvironment (SAGEMAKER_TRAINING, CapacityLimits [{MaxCapacity, CapacityUnit}]) + AWS::Batch::JobQueue (JobQueueType SAGEMAKER_TRAINING, Priority 1000, SchedulingPolicyArn {Ref SP}, ServiceEnvironmentOrder [{Order 1, ServiceEnvironment {Ref SE}}]) reaches CREATE_COMPLETE; the service environment then reads back from batch:DescribeServiceEnvironments with status VALID and an ARN, and the job queue from batch:DescribeJobQueues with status VALID, its JobQueueName and priority 1000. (Only Priority 1001 — above the schema maximum — fails, and it fails in CloudFormation's own template validation before any resource is created, with the stack-level CREATE_FAILED reason "Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.")
Emulator: the Priority 1000 stack — the check's POSITIVE control — ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE, so the valid estate is never created and the in-schema/out-of-schema distinction cannot be observed at all.
Why: a client deploying a SAGEMAKER_TRAINING Batch estate through CloudFormation (the normal way these resources are created) cannot deploy it here; worse, a legal template and an illegal one both end ROLLBACK_COMPLETE, so a template-validation failure is indistinguishable from a provisioning failure and the rollback reason tells the author nothing about Priority.
Fix: the AWS::Batch::JobQueue (and the ServiceEnvironment/SchedulingPolicy it depends on) resource provider in the CloudFormation crate's Batch resource handling, and the CreateJobQueue path in services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/create_job_queue.rs it calls — a SAGEMAKER_TRAINING job queue with ServiceEnvironmentOrder and a SchedulingPolicyArn must create successfully; Priority must be validated against the schema maximum of 1000 in CloudFormation's template validation, before any resource is created. Open issue 20261115T105344Z-batch-cloudformation-a-stack-declaring-a-sagemaker-training-aws-batch.md reports a similar SAGEMAKER_TRAINING JobQueue rollback but does not carry this check's replay key and does not cover the Priority-bound validation asserted here.
Traces: batch__cloudformation.deploy-job-queue-rejects-priority-above-maximum

Cause 2 of 2 — a Priority-only UpdateStack on AWS::Batch::JobQueue ends UPDATE_ROLLBACK_COMPLETE instead of updating the queue in place.
AWS: in the AWS::Batch::JobQueue resource schema Priority is mutable ("No interruption") while JobQueueName is create-only ("Replacement"). An UpdateStack that changes only Priority (1 -> 5) therefore reaches UPDATE_COMPLETE, keeps the job queue's ARN (stack output Ref unchanged, equal to Fn::GetAtt JobQueueArn), and batch:DescribeJobQueues reads priority 5 under the same name. A subsequent UpdateStack that changes JobQueueName reaches UPDATE_COMPLETE too, creates a new queue under a new ARN, and the update's cleanup phase emits the ordered events (JQ CREATE_COMPLETE, JQ DELETE_COMPLETE, <stack> UPDATE_COMPLETE) while the old ARN disappears from batch:DescribeJobQueues.
Emulator: the first, in-place update ends UPDATE_ROLLBACK_COMPLETE — the Priority-only change is rejected or fails mid-update and the stack is rolled back, so neither the in-place mutation nor the later replacement path is ever reached.
Why: changing a job queue's priority is a routine operational edit; a client that applies it through its stack gets a rolled-back update and an unchanged queue, with no indication which property caused it. Because the rollback is terminal for the update, the create-only/mutable distinction the schema encodes is unobservable, and infrastructure code that relies on in-place updates (no ARN churn, no dependent references invalidated) cannot be validated here.
Fix: the AWS::Batch::JobQueue resource provider's update path in the CloudFormation crate — a Priority-only diff must be executed as an in-place update (through batch:UpdateJobQueue, preserving the queue ARN) rather than failing, and a JobQueueName diff must be executed as a replacement with the old queue deleted in the update's cleanup phase.
Traces: batch__cloudformation.deploy-job-queue-update-in-place-and-through-replacement

## Evidence
Both from the simparity-behavioral-aws-v1 replay against this checkout (batch__cloudformation: 2/9 passed, 7 failed). Expected = real AWS behavior asserted by the check; actual = emulator.

Cause 1 — batch__cloudformation.deploy-job-queue-rejects-priority-above-maximum:
  CreateStack with SchedulingPolicy + SAGEMAKER_TRAINING ServiceEnvironment + JobQueue(JobQueueType SAGEMAKER_TRAINING, Priority 1000)
  expected terminal StackStatus 'CREATE_COMPLETE' (then DescribeServiceEnvironments status VALID with an ARN, DescribeJobQueues status VALID with priority 1000)
  got      'ROLLBACK_COMPLETE'
  AssertionError: positive control stack ended 'ROLLBACK_COMPLETE'
  (the negative half — Priority 1001 -> ROLLBACK_COMPLETE with the stack-level CREATE_FAILED reason 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.' and no Batch resources left behind — was never reached)

Cause 2 — batch__cloudformation.deploy-job-queue-update-in-place-and-through-replacement:
  CreateStack with a MANAGED FARGATE ComputeEnvironment + JobQueue(Priority 1) reaches CREATE_COMPLETE, then UpdateStack with the identical template except Priority 5
  expected terminal StackStatus 'UPDATE_COMPLETE', stack output JQRef unchanged (same job queue ARN), DescribeJobQueues priority 5, name unchanged
  got      'UPDATE_ROLLBACK_COMPLETE'
  AssertionError: priority update ended 'UPDATE_ROLLBACK_COMPLETE', expected UPDATE_COMPLETE
  (the following JobQueueName replacement, its (JQ CREATE_COMPLETE, JQ DELETE_COMPLETE, <stack> UPDATE_COMPLETE) event subsequence and the removal of the old ARN from Batch's listing were never reached)

## Steps to Reproduce
Boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1), or by hand:

Cause 1: aws cloudformation create-stack --stack-name s1 --template-body with Resources {SP: AWS::Batch::SchedulingPolicy {Name, QuotaSharePolicy {IdleResourceAssignmentStrategy: FIFO}}, SE: AWS::Batch::ServiceEnvironment {ServiceEnvironmentName, ServiceEnvironmentType: SAGEMAKER_TRAINING, CapacityLimits: [{MaxCapacity: 1, CapacityUnit: ml.m6i.large}]}, JQ: AWS::Batch::JobQueue {JobQueueName, JobQueueType: SAGEMAKER_TRAINING, Priority: 1000, SchedulingPolicyArn: {Ref: SP}, ServiceEnvironmentOrder: [{Order: 1, ServiceEnvironment: {Ref: SE}}]}} -> poll describe-stacks: emulator ROLLBACK_COMPLETE, AWS CREATE_COMPLETE. Repeat with Priority 1001: AWS rolls back with the template-validation reason quoted above.

Cause 2: aws cloudformation create-stack --stack-name s2 --template-body with Resources {CE: AWS::Batch::ComputeEnvironment MANAGED/FARGATE (MaxvCpus 2, Subnets, SecurityGroupIds), JQ: AWS::Batch::JobQueue {JobQueueName, Priority: 1, ComputeEnvironmentOrder: [{Order: 1, ComputeEnvironment: {Ref: CE}}]}} and Outputs {JQRef: {Ref: JQ}} -> CREATE_COMPLETE; then aws cloudformation update-stack with the same template and Priority 5 -> poll describe-stacks: emulator UPDATE_ROLLBACK_COMPLETE, AWS UPDATE_COMPLETE with the queue ARN unchanged and batch describe-job-queues reporting priority 5. Then update again changing only JobQueueName -> AWS UPDATE_COMPLETE with a new ARN and the old queue deleted in the cleanup phase.

Then re-replay: batch__cloudformation.deploy-job-queue-rejects-priority-above-maximum, batch__cloudformation.deploy-job-queue-update-in-place-and-through-replacement.
