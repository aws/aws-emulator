Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery
Issue-Key: ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch|run#29|TimeoutError|raise TimeoutError(
Issue-Key: ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery|_poll_log_events#29|Timeout|time.sleep(interval_s)
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# ecr__eks__logs: nothing delivers a Fargate pod's stdout into the aws-logging ConfigMap's CloudWatch Logs group, so both the delivery check and its no-permission twin never reach a verdict (1 cause)

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFargateProfile

## Description
Cause 1 of 1: the EKS-on-Fargate log router is not emulated — a pod placed by an ACTIVE Fargate profile writes to stdout, but no log group, no log stream and no log event ever appears in CloudWatch Logs for the destination the `aws-logging` ConfigMap names, whatever the Pod execution role allows. Failing checks: ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch, ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery.

AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) models eks#FargateProfile.selectors as "The selectors to match for a Pod to use this Fargate profile" and eks#FargateProfile.podExecutionRoleArn as "The Amazon Resource Name (ARN) of the Pod execution role to use for any Pod that matches the selectors in the Fargate profile"; the delivery itself is documented prose, not modelled — Amazon EKS User Guide, "Fargate logging": EKS runs a built-in Fluent Bit log router beside each Fargate pod, configured by the `aws-logging` ConfigMap of the `aws-observability` namespace. With a `[OUTPUT] Name cloudwatch_logs` stanza naming `log_group_name` (and `log_stream_prefix`, `auto_create_group`), the router creates that group when permitted, opens a stream per pod under that prefix, and puts every line the pod's containers wrote to stdout/stderr there — acting AS THE POD EXECUTION ROLE, so delivery happens only while that role's policies allow logs:CreateLogGroup / logs:CreateLogStream / logs:DescribeLogStreams / logs:PutLogEvents, and no stream and no event appear when they do not. Those two outcomes (delivered vs. not delivered) are the requirement; the group and stream names, the marker text and the per-run latency are values the request and the run supply, not part of it.

Emulator: the `aws-observability` namespace and the `aws-logging` ConfigMap are accepted and served back by the cluster's Kubernetes API, and the pod is admitted and placed, but nothing ever reads that ConfigMap as a log-router configuration: DescribeLogGroups / GetLogEvents in the same account hold no stream and no event for the configured group for the whole run. The delivery check polls the configured group for the unique marker its pod prints and ends with `TimeoutError: marker '<per-run marker>' in log group <per-run group>: the run budget ended first; not graded` (check_ecr__eks__logs_eks_fargate_pod_logs_delivered_to_cloudwatch.py line 150); its negative twin — which withholds the CloudWatch Logs grant from the pod execution role, then grants it and expects a NEW pod's output to arrive — never gets its control leg to deliver either and ends `Check timed out after 60.0s`. The marker name, group name and pod name in those messages are per-run generated values; the requirement-bearing fact is that nothing is delivered.

Why: a client cannot observe any Fargate workload's output on an emulated cluster — every scenario whose subject is a pod's logs (log-driven assertions, CloudWatch Logs Insights over pod output, alarm-on-log-pattern) has an empty log group. The negative case is worse than a gap: "no permission, therefore no delivery" passes vacuously against an emulator in which nothing is delivered for any role, so a real over-permissiveness in the Pod-execution-role check would be invisible.

Fix: the EKS service's Fargate pod data plane — the component that owns pod execution on a profile's capacity — must read the `aws-logging` ConfigMap of `aws-observability` as the log router's configuration and deliver what a pod's containers wrote through the CloudWatch Logs service as the Pod execution role, with that role's policies deciding whether the group/stream creation and PutLogEvents succeed. The Amazon ECS crate's awslogs delivery path is the shape to follow rather than copy (service_common::cloudwatch_logs); file and line citations are under Traces.

Related open issues, neither of which the replay marked as owning these checks: 20261123T225753Z-ecr-eks-logs-a-fargate-pod-admitted-by.md and 20261126T000000Z-ecr-eks-logs-a-fargate-pods-containers-are-never-started.md both carry these two check ids, but their recorded replay keys are the pod-phase waits (`pod <name> leaving Pending`, `_wait_pod_phase`) — the pod lifecycle those describe has moved on: the delivery check now gets past the pod phase and stops in the marker poll at line 150, which is the divergence recorded here. The container-start cause of 20261126T000000Z, if still open, is a prerequisite of this one, not a duplicate of it.

Traces: ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch (expects the delivery; TimeoutError on the marker poll), ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery (expects none, then expects delivery after the grant; times out at 60.0s).

## Evidence
[error] ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch
  File "check_ecr__eks__logs_eks_fargate_pod_logs_delivered_to_cloudwatch.py", line 150, in run
    raise TimeoutError(
  TimeoutError: marker '<per-run marker>' in log group '<per-run log group>': the run budget ended first; not graded

[mismatch] ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery
  Error: Check timed out after 60.0s
  (same scenario inverted: the pod execution role is created WITHOUT logs:CreateLogStream/PutLogEvents and the destination group is pre-created with the caller's own credentials, so the only thing missing is the role's grant; the check then needs the control leg — a new pod after the grant — to deliver, and nothing ever does.)

Expected (AWS): with an ACTIVE Fargate profile selecting namespace `default` and an `aws-logging` ConfigMap declaring `[OUTPUT] Name cloudwatch_logs / Match kube.* / log_group_name <group> / log_stream_prefix <prefix> / auto_create_group true`, the pod's stdout marker arrives as a log event in <group> in a stream named with <prefix>, while a pod execution role lacking logs:CreateLogStream/logs:PutLogEvents produces no stream and no event in <group>.

Actual (emulator): <group> holds no log stream and no log event at any point of the run for either role. The ConfigMap is stored and readable through the served Kubernetes API, the cluster and profile report ACTIVE, and the pod is accepted — so the divergence is the absent log-router delivery, not the control-plane calls, not the TLS handshake (namespace/ConfigMap/pod creations all succeeded over the advertised endpoint) and not a missing runner dependency (the `kubernetes` client is present; an ImportError would have been reported instead).

## Steps to Reproduce
1. CreateRole for an EKS cluster role (trusting eks.amazonaws.com, AmazonEKSClusterPolicy) and a Fargate pod execution role (trusting eks-fargate-pods.amazonaws.com, AmazonEKSFargatePodExecutionRolePolicy); for the delivery leg also attach an inline policy allowing logs:CreateLogGroup, logs:CreateLogStream, logs:DescribeLogStreams, logs:PutLogEvents on "*".
2. CreateCluster with private subnets; wait for status ACTIVE.
3. CreateFargateProfile with that pod execution role and selectors=[{namespace: "default"}]; wait for status ACTIVE.
4. Build a kubeconfig from DescribeCluster (endpoint + base64-decoded certificateAuthority.data). Through the served Kubernetes API create namespace `aws-observability` and a ConfigMap `aws-logging` whose output.conf declares `[OUTPUT] Name cloudwatch_logs / Match kube.* / region <region> / log_group_name <group> / log_stream_prefix from-fluent-bit- / auto_create_group true`.
5. POST a pod into namespace `default` whose command prints a unique marker to stdout.
6. Poll CloudWatch Logs DescribeLogStreams/FilterLogEvents on <group> for that marker.
   -> no stream and no event ever appear; the poll ends at the run budget. Expected: the marker delivered into <group> under a `from-fluent-bit-` stream.
7. Repeat with the inline CloudWatch Logs policy withheld from the pod execution role and <group> pre-created by the caller: AWS still delivers nothing, so the two legs are indistinguishable on this emulator and the negative assertion proves nothing.
