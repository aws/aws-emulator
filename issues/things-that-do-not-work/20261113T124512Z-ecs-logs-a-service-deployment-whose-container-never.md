Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back
Issue-Key: ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back|_await_rollout_failure#4|TimeoutError|raise TimeoutError("the revision-2 deployment reporting rolloutState FAILED: the run budget "
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# ecs__logs: a service deployment whose container never passes its health check never reports rolloutState FAILED, so the rollback never happens

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Cause 1 of 1: updating a service to a revision whose container never passes its health check leaves the deployment in progress forever — the deployment never reports rolloutState 'FAILED' and the service is never rolled back.
AWS: recorded behavior only. With deploymentConfiguration.deploymentCircuitBreaker enabled (rollback true), a revision-2 rollout whose tasks keep failing their container health check is abandoned after the circuit breaker's failure threshold: DescribeServices shows that deployment with rolloutState 'FAILED' and a rolloutStateReason naming the circuit breaker, and the service converges back on the last good revision while the primary deployment returns to rolloutState 'COMPLETED'.
Emulator: `TimeoutError: the revision-2 deployment reporting rolloutState FAILED: the run budget ended first; not graded` — the bad deployment never leaves IN_PROGRESS within the budget, so no failure verdict and no rollback is ever observed.
Why: the circuit breaker is the control that stops a bad revision from consuming the service. A deployment that never fails means a client's deploy waits forever instead of being rolled back, and automation keyed on rolloutState FAILED never fires.
Fix: ECS deployment orchestration in the ECS crate — the deployment circuit breaker's failure accounting (driven by container health/task start failures) and the rolloutState/rolloutStateReason transitions plus the rollback to the last good revision.
Traces: ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back.

## Evidence
File "check_ecs__logs_svcreplace_deployment_whose_container_never_passes_its_healthcheck_is_rejected_and_rolled_back.py", line 562, in _await_rollout_failure -> TimeoutError: the revision-2 deployment reporting rolloutState FAILED: the run budget ended first; not graded. Expected DescribeServices deployments[] to show the revision-2 deployment with rolloutState 'FAILED' and the service back on revision 1.

## Steps to Reproduce
Create a cluster, a service on revision 1 with deploymentConfiguration.deploymentCircuitBreaker={enable:true, rollback:true} and a container healthCheck; register revision 2 whose container health check can never pass; UpdateService to revision 2; poll DescribeServices deployments[].rolloutState/rolloutStateReason and the service's taskDefinition.
