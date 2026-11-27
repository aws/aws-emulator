Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs.fargate-container-receives-the-ecs-set-region-and-execution-env
Issue-Key: ecs__logs.fargate-container-receives-the-ecs-set-region-and-execution-env|run#27|AssertionError|assert got == want, f"the container saw {got!r}, expected {want!r}"
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# ecs__logs: a Fargate container's ECS-injected environment is empty — AWS_EXECUTION_ENV, AWS_REGION and AWS_DEFAULT_REGION are all unset inside the task

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: ECS does not inject its platform environment variables into the running container.
AWS: a task launched with launchType FARGATE runs with ECS-supplied environment variables in addition to the task definition's own `environment`: AWS_EXECUTION_ENV is set to `AWS_ECS_FARGATE`, and AWS_REGION and AWS_DEFAULT_REGION are both set to the Region the task runs in. A container that echoes them (its output captured through the awslogs driver) therefore reports the fixed value AWS_ECS_FARGATE for the execution environment and the task's Region for both Region variables (recorded behavior only — the ECS Smithy model describes containerDefinitions.environment but not the platform-injected variables).
Emulator: the container sees all three variables unset/empty — the echoed line reads `EXEC=`, `REGION=`, `DEFAULT=`.
Why: AWS SDKs inside a task resolve their Region from AWS_REGION/AWS_DEFAULT_REGION and report their execution environment from AWS_EXECUTION_ENV. With these empty, in-task SDK calls fail Region resolution (or fall back to a wrong Region) and any code branching on “am I on Fargate?” takes the wrong branch, which no application-level configuration can work around.
Fix: the container runtime/environment assembly for a launched task (the task-launch path in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/, where the container's env is built from the task definition) must add the ECS-injected variables: AWS_EXECUTION_ENV=AWS_ECS_FARGATE and AWS_REGION/AWS_DEFAULT_REGION = the task's Region.
Traces: check ecs__logs.fargate-container-receives-the-ecs-set-region-and-execution-env.

## Evidence
AssertionError: the container saw ['EXEC=', 'REGION=', 'DEFAULT='], expected ['EXEC=AWS_ECS_FARGATE', 'REGION=<task-region>', 'DEFAULT=<task-region>'] (the recorded run expected us-west-2, i.e. the Region the task ran in; the Region value is taken from the environment, the literal AWS_ECS_FARGATE is fixed text).

## Steps to Reproduce
Replay check ecs__logs.fargate-container-receives-the-ecs-set-region-and-execution-env: register a task definition whose container echoes $AWS_EXECUTION_ENV, $AWS_REGION and $AWS_DEFAULT_REGION with the awslogs driver, RunTask launchType=FARGATE, then read the container's log events from the log group and compare the echoed values.
