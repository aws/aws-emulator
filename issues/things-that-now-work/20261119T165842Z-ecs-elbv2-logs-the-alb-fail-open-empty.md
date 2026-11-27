Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503
Issue-Key: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503|_assert_stays#30|Timeout|observed = probe()
Issue-Commit: 1a6df7b32e4c8350e561c708bcd111a16150a036

# ecs__elbv2__logs: the ALB fail-open / empty-target-group-503 scenario still reaches no verdict — the check is cut off at the replay ceiling with no target state and no served response

> _Note: this issue was identified against the code base at commit `1a6df7b32e4c8350e561c708bcd111a16150a036`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One root cause, reached by 1 check (Cause 1 of 1).

NOTE ON OWNERSHIP: the open issue 20261110T220525Z-ecs-elbv2-logs-2-net-new-causes-the.md names this same check id in its Issue-Trace-Ids, but the suite did NOT mark this failure OWNED — its recorded replay key for this check (`...|_poll_until#17|Timeout|time.sleep(interval_s)`) no longer matches how the failure surfaces now (a bare "Check timed out after 60.0s" with no _poll_until frame). It is re-filed here so the live failure carries a key a fixer can re-replay; a fixer closing that issue should treat this one as the same defect and verify with the id in trace_ids.

Cause 1 of 1: none of the three load-balancing outcomes this scenario exists to observe is reachable — the ALB's fail-open behaviour when every target is unhealthy, the load balancer's own 503 for a target group with no registered targets, and the return to healthy 200s after the health-check path is repaired. The check makes no observable progress and is killed by the replay ceiling.

AWS: Smithy (elasticloadbalancingv2, model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json): DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth {State, Reason, Description}, where TargetHealthReasonEnum includes `Target.ResponseCodeMismatch` ("The health checks did not return an expected HTTP code"); CreateListener DefaultActions[] {Type 'forward', TargetGroupArn}; RegisterTargets/DeregisterTargets. Per the target-group health-checks documentation the check cites: when EVERY target in the group is unhealthy the load balancer fails open and forwards to them anyway, so a reachable container still answers (HTTP 200 with its own body); when the group has NO registered target the load balancer answers its own HTTP 503; and repairing the health-check path returns the target to State 'healthy' and the responses to healthy 200s. Those three outcomes, not any particular id or address, are the requirement.

Emulator: "Check timed out after 60.0s" — no terminal target state and no served response is produced, so neither the fail-open 200, nor the empty-group 503, nor the recovery is ever observable. Hypothesis: the ip target never leaves its initial state because no real health-check probe is performed, so the poll for 'unhealthy'/'healthy' cannot terminate and the data-plane assertions are never reached.

Why: fail-open is precisely the behaviour that keeps a service reachable during a bad health-check rollout, and the empty-group 503 is how a client distinguishes "nothing is registered" from "the target refused". With neither observable, a client cannot predict what its load balancer does when health checks go wrong — the exact situation the behaviour exists for — and a deployment guard built on the 503 silently never fires.

Fix: the ELBv2 target-health state machine (it must probe the configured health-check path and publish State 'unhealthy' with Reason Target.ResponseCodeMismatch for a non-matching code, then 'healthy' once the path is repaired) and the ELBv2 data-plane forwarding path (fail open to all-unhealthy targets; generate the load balancer's own 503 for an empty target group).

Traces: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator; service heading ecs__elbv2__logs (2/6 measured, the sibling health-check failure under this heading is OWNED, this one is not).

[mismatch] ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503
  Error: Check timed out after 60.0s

Expected (AWS): the single ip target reaches State 'unhealthy' with Reason Target.ResponseCodeMismatch (its health-check path answers a code outside the matcher) and the load balancer fails open, still serving the container's HTTP 200; after deregistering all targets, a request to the listener is answered with the load balancer's own HTTP 503; after repairing the health-check path, the target returns to 'healthy' and requests are answered 200 again.
Actual (emulator): no terminal target state and no served response within the replay budget — the scenario reaches none of the three graded outcomes.

Smithy consulted: elasticloadbalancingv2 DescribeTargetHealth output TargetHealthDescriptions[].TargetHealth {State, Reason, Description} (TargetHealthReasonEnum: Elb.RegistrationInProgress, Elb.InitialHealthChecking, Target.ResponseCodeMismatch, Target.Timeout, Target.FailedHealthChecks, ...), CreateListener DefaultActions[].{Type,TargetGroupArn}. Fail-open and empty-group-503 semantics: the ELBv2 target-group health-check documentation the check cites.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the id in trace_ids reproduces ("Check timed out after 60.0s").
2. Create an ALB over two subnets with an HTTP:80 listener forwarding to an ip-type target group whose health-check path deliberately returns a code outside the success matcher.
3. Run an awsvpc Fargate task that serves a known body on port 80 and register its ip as the group's only target.
4. Poll DescribeTargetHealth. Expected: State 'unhealthy' with Reason Target.ResponseCodeMismatch. Actual: the state never settles.
5. GET http://<load balancer address>/. Expected: HTTP 200 (fail open to the all-unhealthy group) with the container's body.
6. Deregister the target and GET again. Expected: the load balancer's own HTTP 503.
7. Repair the health-check path, wait for State 'healthy', and GET again. Expected: HTTP 200.
8. Re-replay the id in trace_ids to confirm the fix.
