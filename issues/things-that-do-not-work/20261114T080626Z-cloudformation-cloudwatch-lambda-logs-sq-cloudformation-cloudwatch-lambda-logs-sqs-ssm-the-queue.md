Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow
Issue-Key: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow|_deploy_cfn#14|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__cloudwatch__lambda__logs__sqs__ssm: the queue-worker stack rolls back instead of reaching CREATE_COMPLETE, so the change-set update workflow is never exercised

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the positive-control deploy of the queue-worker template ends in ROLLBACK_COMPLETE.
AWS:      recorded behavior only. The check deploys the suite's `queue-worker.yaml` — an SQS queue, a Lambda worker with its execution role, a CloudWatch Logs log group, a CloudWatch alarm and an SSM parameter — as the baseline for a change-set-driven update, and AWS drives that stack to CREATE_COMPLETE; the check's whole change-set workflow (create change set against the broken variant, inspect it, execute, verify) runs only after the baseline stack exists. Nothing in the recording shows a refusal: the baseline is a plain multi-service template.
Emulator: the baseline CreateStack settles at ROLLBACK_COMPLETE (`AssertionError: Stack <name> ended in ROLLBACK_COMPLETE`), so at least one of the template's resource handlers fails its create and CloudFormation unwinds the stack. The check stops at the first step and never reaches a change set.
Why:      a client cannot stand up the most ordinary serverless shape there is — a queue, a worker function, its log group, an alarm and a parameter — in one template. Every resource is rolled back, so nothing downstream (the queue worker, the alarm, the parameter) exists, and a deploy pipeline sees a failed stack with no usable estate.
Fix:      the failing resource handler's create path inside the CloudFormation provisioner for this template's resource set (AWS::SQS::Queue, AWS::Lambda::Function + AWS::IAM::Role, AWS::Logs::LogGroup, AWS::CloudWatch::Alarm, AWS::SSM::Parameter); the per-resource CREATE_FAILED event's reason on the rollback names which handler refused and must be the starting point.
Why this is not covered by an existing issue: the replay report lists this check as unowned — no open issue carries its replay key — and the similar-looking open items for this service family (for example 20261110T220525Z-cloudformation-cloudwatch-ecr-ecs-elbv2-logs-a-nested.md, about nested-stack Outputs carrying no OutputValue) are about a different failure and a different template.
Traces:   cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow.

## Evidence
Expected: the baseline queue-worker stack reaches CREATE_COMPLETE, after which the check creates and executes a change set against the no-config-permission variant.
Actual:
  File "check_cloudformation__cloudwatch__lambda__logs__sqs__ssm_cfn_change_set_queue_worker_update_workflow.py", line 298, in _deploy_cfn
  raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
  AssertionError: Stack sim2real-xsvc-stk-354453a86a89 ended in ROLLBACK_COMPLETE

(The stack name is a per-run generated value; the requirement is the terminal status CREATE_COMPLETE.)

## Steps to Reproduce
1. CreateStack with the suite's `queue-worker.yaml` deployment template (SQS queue + Lambda worker and execution role + Logs log group + CloudWatch alarm + SSM parameter), with CAPABILITY_IAM.
2. Poll DescribeStacks until the stack reaches a terminal status.
3. Observe ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
4. Read DescribeStackEvents and note which resource reported CREATE_FAILED and with what reason.
