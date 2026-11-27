Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes|run#47|AssertionError|assert rc_b == 0.0, (
Issue-Commit: bdca4f331e3f84a5ee858acb1e0644b3d6ffabda

# elbv2: ALB-generated fixed-response replies are counted in AWS/ApplicationELB RequestCount, which AWS only increments when a target was chosen

> _Note: this issue was identified against the code base at commit `bdca4f331e3f84a5ee858acb1e0644b3d6ffabda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the ELBv2 data-plane metric emitter increments the AWS/ApplicationELB `RequestCount` metric for requests the load balancer answered itself (a listener-rule `fixed-response` action), instead of only for requests for which a target was selected.

AWS: not modeled in Smithy — this is CloudWatch metric-emission behavior of the ELBv2 data plane, so: recorded behavior only, plus the ALB CloudWatch-metrics contract the check states as its premise H1: `RequestCount` "is only incremented for requests where the load balancer node was able to choose a target". A request terminated by a `fixed-response` rule action never reaches target selection, so for a minute in which every request was answered by a fixed-response action AWS publishes no `RequestCount` contribution at all for the `LoadBalancer` dimension (the Sum over that minute is 0 / the datapoint is absent). By the same rule, an ALB-generated 4xx is not a target response, so `HTTPCode_Target_4XX_Count` and `HTTPCode_Target_2XX_Count` for the `TargetGroup`+`LoadBalancer` dimension pair are likewise not incremented for that minute. These are counting rules, not values: the quantities in any one run (25 fixed-response requests, 25 target 200s, 15 target 404s, and the particular minute boundaries) are that run's inputs filling the template, never part of the requirement.

Emulator: for the minute in which N requests were all served by a fixed-response action and no target was ever chosen, `GetMetricStatistics` on AWS/ApplicationELB `RequestCount` with dimension `LoadBalancer=app/<name>/<id>`, Period 60, Statistic Sum returns N (the replay measured 25.0 where AWS returns 0.0), i.e. the emulator attributes every HTTP exchange at the listener to `RequestCount` regardless of whether a target was selected. The positive control in the same check passes — the target-served minute measured 25.0 as expected — so metric delivery, dimension derivation and the 60 s period all work; only the inclusion rule is wrong. The check asserts `RequestCount == 0` first and stops there, so the two companion legs (`HTTPCode_Target_4XX_Count == 0` and `HTTPCode_Target_2XX_Count == 0` for the fixed-response minute) were never reached and are unverified; they share this root cause ("attribute the exchange even though no target was involved") and should be fixed and re-checked together.

Why: a client cannot distinguish load-balancer-generated traffic from target traffic. Dashboards and alarms built on `RequestCount` over-report real backend load by exactly the volume of redirect/fixed-response/edge-rejected requests; an ALBRequestCountPerTarget-style scaling policy or a request-count alarm therefore scales out or alarms on traffic that never touched a target, and the documented "ALB-generated vs target-generated" diagnostic split (used to tell an ALB 4xx/5xx from a target 4xx/5xx) collapses.

Fix: the ELBv2 data-plane request-completion path that publishes AWS/ApplicationELB metrics. The metric emission must be gated on whether the request reached target selection: emit `RequestCount` only after a target was chosen, and emit the `HTTPCode_Target_*` family only from an actual target response — a response synthesized by the listener/rule action (fixed-response, redirect, or an ALB-generated error) must contribute to neither, and should instead feed only the ELB-side counters.

Traces: the failing check `elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes` (verdict: mismatch). No existing open issue covers it: the nearest neighbours are 20261110T192713Z-cloudwatch-ec2-elbv2-an-alb-answers-http-502.md (ALB 502/health-count metric scenarios, a different cause and a different service heading) and 20261117T035019Z-cloudwatch-ec2-elbv2-an-aws-applicationelb-unhealthyhostcount-alarm.md (UnHealthyHostCount alarm state); neither concerns the target-chosen inclusion rule for RequestCount.

## Evidence
Check: elbv2.alb-generated-responses-excluded-from-requestcount-and-target-codes [mismatch]

Assertion that failed (check_elbv2_alb_generated_responses_excluded_from_requestcount_and_target_codes.py line 584):
  AssertionError: RequestCount over 2026-11-20T17:29:00+00:00..2026-11-20T17:30:00+00:00 was 25.0, expected 0: all 25 requests in that minute were answered by a fixed-response action, so the load balancer never chose a target (control: the target minute measured 25.0)

Expected (AWS): Sum of AWS/ApplicationELB RequestCount, Period 60, Dimensions [{LoadBalancer: app/<name>/<id>}], over the minute in which every request was terminated by a listener-rule fixed-response action = 0 (no target was chosen). In the same run the control minute, in which the same number of requests were forwarded to a target, = that number (25.0 here) with Unit "Count" — so the emulator's publisher, dimensions and period are correct.

Actual (emulator): the fixed-response minute reports Sum = 25.0, identical to the target-served control minute — every listener exchange is counted, whether or not a target was selected.

Unreached legs of the same cause (not yet observed, blocked by the first assertion): HTTPCode_Target_4XX_Count over the fixed-response minute for Dimensions [{TargetGroup: targetgroup/<name>/<id>}, {LoadBalancer: app/<name>/<id>}] must be 0 while the target-404 control minute is non-zero; HTTPCode_Target_2XX_Count over the fixed-response minute must likewise be 0.

## Steps to Reproduce
1. Create a VPC with two subnets in different AZs, a security group allowing HTTP ingress, an internet-facing Application Load Balancer, and an instance/IP target group with one healthy target that serves HTTP 200 on a known path and HTTP 404 on another.
2. Create an HTTP listener whose DEFAULT action is `fixed-response` with StatusCode 404 (an ALB-generated body), plus two path-pattern rules that `forward` to the target group: one path whose target answers 200, one whose target answers 404.
3. Warm all three routes, then, aligned to a minute boundary, send a batch of N requests to the path that hits the fixed-response default action (every reply is the ALB-generated 404). Note that minute as window B.
4. In two later, separately minute-aligned batches, send N requests to the target-200 path (window C) and M requests to the target-404 path (window D).
5. Poll cloudwatch GetMetricStatistics until the C window reports AWS/ApplicationELB RequestCount Sum >= N with Dimensions [{LoadBalancer: <last-3-ARN-segments, app/<name>/<id>>}], Period 60, Statistics [Sum] — this is the positive control proving metrics for this wiring have landed.
6. Now read RequestCount Sum for window B with the same LoadBalancer dimension.
   Expected: 0.0 (no target was ever chosen in that minute). Observed: N (25.0).
7. Also read HTTPCode_Target_4XX_Count and HTTPCode_Target_2XX_Count for window B with Dimensions [{TargetGroup: targetgroup/<name>/<id>}, {LoadBalancer: app/<name>/<id>}]; both must be 0.0 while the D and C windows are non-zero.
