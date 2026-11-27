Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted
Issue-Key: autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted|_poll#20|Timeout|return last
Issue-Commit: 8465be9c60d4839857b29976b5cf2d08632066fc

# autoscaling__ec2__elbv2: the cluster-security-group target-health check still ends at the replay cap after the verdict machinery was fixed — what remains is something the check does that the emulator's reconstruction of it does not

> _Note: this report is the remainder of
> `issues/things-that-now-work/20261119T084315Z-autoscaling-ec2-elbv2-2-net-new-causes-security.md`,
> whose two named causes are implemented. It carries that issue's `asghc` replay key so a later
> scan does not read the check as working._

## Operation
DescribeTargetHealth

## Description
One cause, and it is an INVESTIGATION rather than a known defect: the check
`autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted`
still ends as `Error: Check timed out after 60.0s` with no graded step, while every link of the
loop it drives now converges in the emulator well inside that cap.

What was fixed, and is no longer the cause: a registration's `unhealthy` verdict used to be
`UnhealthyThresholdCount x HealthCheckIntervalSeconds` away (three minutes for a target group
created with a twenty-second cadence and ten consecutive failures), so no verdict was observable
inside a polling client's budget. The checks of a registration are now spaced so the run of
consecutive failing checks a verdict takes fits inside the span this emulator keeps a verdict
observable in (`services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/state/target_health.rs`,
`health_check_interval` / `verdict_spacing`), while every read still reports the schedule the
create stored.

Evidence that the loop itself converges: `core/router/tests/all/asg_nlb_target_health_verdicts_at_any_cadence.rs`
drives the whole of that check's topology over the assembled emulator — one VPC and subnet, a
cluster security group that admits only itself, a launch template whose guest serves the
health-check port, a TCP target group on 44300 with an HTTP health check on :4000 at
`/health-check` at a twenty-second cadence with thresholds 2/10, an internal Network Load Balancer
associated with no security group, a listener forwarding to the group, and an Auto Scaling group of
one — and reads `unhealthy` / `Target.FailedHealthChecks` on the traffic port, then `healthy` with
no reason and the member `InService` / `Healthy` once `AuthorizeSecurityGroupIngress` admits the
subnet's block on the checked port. Both verdicts are reached in about 25 s.

So the remaining difference is something the recorded check does that this reconstruction does not.
The leading candidate, which could not be observed from the issue, the model or the replay output:
the recorded launch template's own USER-DATA program. A check of :4000 is answered only while the
guest of that instance leaves a server listening there, and the emulator's guest performs the
program forms it models (`services/service-com-amazonaws-ec2-amazonec2/src/state/guest_user_data.rs`,
`state/guest_services.rs`, `::service_common::static_web_servers`) — a package manager plus an init
unit, and the interpreter's own HTTP server module. A program that starts its server any other way
(a script file handed to an interpreter, a unit file written at boot, a listener opened by another
tool) leaves the guest listening on nothing, every check of the health-check port then fails
whatever the security group says, and the `healthy` half of this check can never arrive.

## Evidence
Replay of the two units this key's issue was filed for, on the branch that fixed the verdict
machinery:

```
asgdx-health-check-blocked-by-ingressless-sg-replaces-instances-until-subnet-admitted  passed
asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted   Check timed out after 60.0s
```

The emulator answers every control-plane call of that check; what no reading shows is which poll it
dies in, because the harness reports the timeout rather than the last value read.

## Steps to Reproduce
1. Replay `autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted`
   and RECORD the check's own body — in particular the launch template's `UserData` and the target
   group's health-check members — which is what this report could not observe.
2. Launch one instance through `RunInstances` with exactly that user data and read
   `DescribeInstances` plus the guest's own listening ports (a target group's health check reads
   them through `state::guest_services::instance_guest_server_ports`). If the guest is listening on
   nothing, the gap is the program form: implement it where the guest's programs live, or file the
   form as its own EC2 issue.
3. Otherwise, bisect the check's polls: `DescribeTargetHealth` for the first non-`initial` verdict,
   then the `healthy` one after `AuthorizeSecurityGroupIngress(tcp <health-check port> from the
   subnet block)`, then the Auto Scaling member's `LifecycleState` / `HealthStatus`, and name the
   one that does not arrive.
4. `core/router/tests/all/asg_nlb_target_health_verdicts_at_any_cadence.rs` is the reconstruction to
   extend with whatever that step turns up: it already asserts the rest of the loop end to end.
