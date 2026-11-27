Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__elbv2.metrics-elbv2-apigw-alb-empty-target-group-503-counted-as-elb-5xx-not-target-5xx cloudwatch__ec2__elbv2.metrics-elbv2-apigw-alb-failing-health-endpoint-flips-healthy-and-unhealthy-host-count cloudwatch__ec2__elbv2.metrics-elbv2-apigw-alb-request-metrics-exact-counts-code-split-and-literal-dimensions cloudwatch__ec2__elbv2.obs-elbv2-1-alb-rejects-malformed-requests-elb-504-and-keepalive-connection-metrics
Issue-Key: cloudwatch__ec2__elbv2.metrics-elbv2-apigw-alb-empty-target-group-503-counted-as-elb-5xx-not-target-5xx|_http#1|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: cloudwatch__ec2__elbv2.metrics-elbv2-apigw-alb-failing-health-endpoint-flips-healthy-and-unhealthy-host-count|_http#1|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: cloudwatch__ec2__elbv2.metrics-elbv2-apigw-alb-request-metrics-exact-counts-code-split-and-literal-dimensions|_http#1|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: cloudwatch__ec2__elbv2.obs-elbv2-1-alb-rejects-malformed-requests-elb-504-and-keepalive-connection-metrics|_http#1|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# cloudwatch__ec2__elbv2: the ALB's published DNSName is an elb.amazonaws.com host the emulator does not serve, so no request can be sent and no ALB metric is ever produced

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
One net-new root cause (Cause 1 of 1). Related open filings that do NOT carry these replay keys: 20261108T002731Z-cloudwatch-ec2-elbv2-an-alb-s-own-enis.md and 20261108T221642Z-cloudwatch-ec2-elbv2-an-alb-s-own-enis-c4c110.md describe ALB ENIs/alarms for this heading; the four ids below are refused in the harness's published-address gate and the suite marks none of them OWNED.

Cause 1 of 1: ELBv2 publishes LoadBalancer.DNSName in the real-AWS elb.amazonaws.com domain — an address other than the one the emulator is reached on.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer -> LoadBalancers[].DNSName, "The public DNS name of the load balancer"; LoadBalancer.Scheme documents that an internet-facing load balancer's DNS name resolves to the load balancer nodes. AWS/ApplicationELB RequestCount, HTTPCode_ELB_5XX_Count, HTTPCode_Target_*_Count, HealthyHostCount/UnHealthyHostCount and the connection/keepalive metrics are all produced from requests that arrive at that address. Recorded behavior only for the wire details.
Emulator: CreateLoadBalancer answers 200 with DNSName of the form <lb-name>-<id>.<region>.elb.amazonaws.com (name and digits are per-run values; the defect is the domain), which is not one of the emulator's own hosts, so the warm-up probe is refused with simparity.published.PublishedUrlRejected "endpoint_url was given http://<published-host>/warm/ok (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".
Why: a client cannot drive a single request through the load balancer, so the exact request counts, the ELB-5XX vs Target-5XX split, the literal CloudWatch dimension values (LoadBalancer app/<name>/<id>, TargetGroup targetgroup/<name>/<id>) and the healthy/unhealthy host-count flip are all unobservable, while CreateLoadBalancer reports success.
Fix: the ELBv2 load-balancer state/serializer path that mints and reports DNSName (CreateLoadBalancer / DescribeLoadBalancers must publish the emulator's own reachable host[:port]), plus the listener socket path that serves requests on that address and publishes the AWS/ApplicationELB datapoints.
Traces: the four ids in trace_ids.

## Evidence
Replay of suite simparity-behavioral-aws-v1: cloudwatch__ec2__elbv2 4/8 (50%), 4 failed — all four with the identical frame.

[mismatch] cloudwatch__ec2__elbv2.metrics-elbv2-apigw-alb-request-metrics-exact-counts-code-split-and-literal-dimensions
  simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-mlb-2f27e468ed74-5664292645.us-west-2.elb.amazonaws.com/warm/ok (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:44339/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Same frame and message, different per-run host, for metrics-elbv2-apigw-alb-empty-target-group-503-counted-as-elb-5xx-not-target-5xx, metrics-elbv2-apigw-alb-failing-health-endpoint-flips-healthy-and-unhealthy-host-count and obs-elbv2-1-alb-rejects-malformed-requests-elb-504-and-keepalive-connection-metrics.

Expected: the published DNSName answers the warm-up GET and the AWS/ApplicationELB datapoints follow. Actual: an AWS-owned elb.amazonaws.com host the emulator does not serve.

Smithy consulted: elasticloadbalancingv2 CreateLoadBalancer output LoadBalancers[] (LoadBalancer.DNSName, LoadBalancer.Scheme).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the four ids in trace_ids reproduce.
2. elbv2.create_load_balancer(Type='application', Scheme='internet-facing', Subnets=[two subnets]) and read LoadBalancers[0].DNSName — the emulator returns <name>-<id>.<region>.elb.amazonaws.com.
3. Add an HTTP listener forwarding to a target group and send GET http://<DNSName>/warm/ok. On AWS this is served and increments AWS/ApplicationELB RequestCount; here the published host is not one of the emulator's own hosts and the request is refused before connect.
4. Re-replay the ids in trace_ids to confirm the fix.
