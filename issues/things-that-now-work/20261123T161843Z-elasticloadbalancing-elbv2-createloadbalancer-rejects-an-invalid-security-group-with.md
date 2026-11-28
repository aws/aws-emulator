Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2err-create-load-balancer-input-rejections
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2err-create-load-balancer-input-rejections
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Related: 20261104T190207Z-elasticloadbalancing-elbv2-subnet-refusal-wording-and-describerules-paging-diverge.md
Issue-Commit: ab053b44c0c4551c75ed9277db4b5c52abcaed14

# ELBv2 CreateLoadBalancer rejects an invalid security group with the wrong error code and an EC2 SDK exception string

> _Note: this issue was identified against the code base at commit `ab053b44c0c4551c75ed9277db4b5c52abcaed14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 1: CreateLoadBalancer (ELBv2, awsQuery protocol) refuses a request whose SecurityGroups member names a security group that does not exist in the caller's VPC. The emulator does refuse it with HTTP 400, but it answers with a different modeled error than AWS and its Error/Message is a leaked AWS-SDK-for-Java exception rendering of a FOREIGN service's (EC2) error rather than Elastic Load Balancing's own sentence for this error class.

AWS: Smithy (com.amazonaws.elasticloadbalancingv2#CreateLoadBalancer) models both InvalidConfigurationRequestException (awsQueryError code InvalidConfigurationRequest, httpError 400, doc "The requested configuration is not valid.") and InvalidSecurityGroupException (awsQueryError code InvalidSecurityGroup, httpError 400, doc "The specified security group does not exist."). For this input AWS classifies the failure as the configuration-request error: the recorded response is HTTP 400 with an awsQuery ErrorResponse whose Error/Type is Sender, Error/Code is InvalidConfigurationRequest and Error/Message is the fixed sentence "One or more security groups are invalid". That message is entirely fixed text for this error class — ELBv2 interpolates no security group id into it, and it never embeds a nested service / status-code / error-code / request-id / proxy envelope; the request id belongs in ErrorResponse/RequestId only. (Error code and message here are from the recorded response; the model supplies the error shapes, their awsQuery codes and the 400 status.)

Emulator: replay of fixture elbv2err-create-load-balancer-input-rejections step task#7 returns HTTP 400 (status matches) but $.ErrorResponse.Error.Code = "InvalidSecurityGroup" instead of "InvalidConfigurationRequest", and $.ErrorResponse.Error.Message = "Invalid id: \"sg-0123456789abcdef0\" (Service: AmazonEC2; Status Code: 400; Error Code: InvalidGroupId.Malformed; Request ID: b0c2cf66-a507-4ed2-ae91-50acb9b635ab; Proxy: null)" instead of "One or more security groups are invalid". Breaking that message apart: the fixed — and wrong — text is the EC2 sentence 'Invalid id: "<id>"' plus the whole parenthesised SDK ClientException suffix "(Service: AmazonEC2; Status Code: 400; Error Code: InvalidGroupId.Malformed; Request ID: <uuid>; Proxy: null)", which names EC2 and the EC2 code InvalidGroupId.Malformed; the only variable parts are the security group id taken from the request's SecurityGroups member and the per-run request id, and those per-run values are not themselves the defect.

Why: a client relying on AWS's behavior switches on the awsQuery error code to pick its modeled exception, so it builds an InvalidSecurityGroupException where AWS raises InvalidConfigurationRequestException — code that catches the latter around CreateLoadBalancer never fires, and retry/remediation logic keyed on the error code takes the wrong branch. On top of that, tooling that matches or logs the documented message "One or more security groups are invalid" sees a different service's error text, and anything that treats an embedded "Error Code: InvalidGroupId.Malformed" as an EC2-level failure mis-routes an elasticloadbalancing failure to EC2 remediation. Because a fresh request id is interpolated into the message on every call, the message is also non-deterministic, which breaks retry comparison and log de-duplication on message equality.

Fix: the CreateLoadBalancer handler's input validation / error mapping in the ELBv2 service crate — the place where the internal security-group lookup failure is converted into the operation's awsQuery error response. It must raise the operation's own modeled InvalidConfigurationRequestException (awsQuery code InvalidConfigurationRequest, Type Sender, HTTP 400) carrying ELBv2's fixed message for an unusable security group, and must not propagate the underlying EC2 error's code or text: the SDK envelope (service name, status code, foreign error code, request id, proxy) must not appear in Error/Message at all, and the request id must be carried only in ErrorResponse/RequestId.

Traces: elbv2err-create-load-balancer-input-rejections#task#7 (verdict mismatch, expected_status 400, actual_status 400, divergent keys $.ErrorResponse.Error.Code and $.ErrorResponse.Error.Message).

Note: no open issue covers this operation. The nearest neighbour, 20261120T015329Z-elasticloadbalancing-elb-classic-applysecuritygroupstoloadbalancer-leaks-an-ec2-sdk-exception.md, is the same family of defect (a leaked EC2 SDK exception string) but on ELB Classic's ApplySecurityGroupsToLoadBalancer — a different service surface, a different operation and a different error code, and it does not carry this fixture step as a replay key. 20261104T190207Z-elasticloadbalancing-elbv2-subnet-refusal-wording-and-describerules-paging-diverge.md covers ELBv2 SUBNET refusal wording and DescribeRules paging, not the security-group path of CreateLoadBalancer.

## Evidence
Replay of fixture elbv2err-create-load-balancer-input-rejections, step task#7, operation CreateLoadBalancer, service elasticloadbalancing, verdict mismatch (expected_status 400, actual_status 400).

Expected (recorded AWS response; error shapes/codes/status per Smithy com.amazonaws.elasticloadbalancingv2#CreateLoadBalancer errors):
  HTTP 400
  <ErrorResponse><Error><Type>Sender</Type><Code>InvalidConfigurationRequest</Code><Message>One or more security groups are invalid</Message></Error><RequestId>...</RequestId></ErrorResponse>

Actual (emulator):
  HTTP 400
  $.ErrorResponse.Error.Code    = "InvalidSecurityGroup"                     (expected "InvalidConfigurationRequest")
  $.ErrorResponse.Error.Message = "Invalid id: \"sg-0123456789abcdef0\" (Service: AmazonEC2; Status Code: 400; Error Code: InvalidGroupId.Malformed; Request ID: b0c2cf66-a507-4ed2-ae91-50acb9b635ab; Proxy: null)"   (expected "One or more security groups are invalid")
  -> value_mismatch on $.ErrorResponse.Error.Code and $.ErrorResponse.Error.Message

Fixed vs variable in the actual message: fixed and wrong = the EC2 sentence 'Invalid id: "<id>"' and the SDK suffix "(Service: AmazonEC2; Status Code: 400; Error Code: InvalidGroupId.Malformed; Request ID: <uuid>; Proxy: null)"; variable (not themselves the defect) = the security group id echoed from the request and the per-run request id. The required message for this error class has no variable parts.

Model references used: smithy_query operation=CreateLoadBalancer (errors include InvalidConfigurationRequestException -> awsQueryError InvalidConfigurationRequest/400 and InvalidSecurityGroupException -> awsQueryError InvalidSecurityGroup/400); smithy_query shape=InvalidConfigurationRequestException,InvalidSecurityGroupException (docs "The requested configuration is not valid." / "The specified security group does not exist.").

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1), or reproduce manually against the ELBv2 (elasticloadbalancing) awsQuery endpoint: call CreateLoadBalancer with Name=<a fresh name>, Type=application, Subnets.member.1/2=<two valid subnets in the VPC> and SecurityGroups.member.1=sg-0123456789abcdef0 (a security group id that is not valid/does not exist in that VPC).
3. Inspect the awsQuery ErrorResponse. The status is 400 as AWS returns, but Error/Code is InvalidSecurityGroup instead of InvalidConfigurationRequest, and Error/Message is an EC2 SDK exception string ('Invalid id: "sg-..." (Service: AmazonEC2; Status Code: 400; Error Code: InvalidGroupId.Malformed; Request ID: ...; Proxy: null)') instead of "One or more security groups are invalid".
4. Corresponds to fixture elbv2err-create-load-balancer-input-rejections, step task#7.
