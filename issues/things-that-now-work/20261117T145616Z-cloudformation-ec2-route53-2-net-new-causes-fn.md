Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__route53.deploy-cfn-ec2-eip-getatt-publicip-feeds-route53-recordset-same-create cloudformation__ec2__route53.deploy-cfn-route53-recordset-unsupported-eip-getatt-attribute-rejected
Issue-Key: cloudformation__ec2__route53.deploy-cfn-ec2-eip-getatt-publicip-feeds-route53-recordset-same-create|run#10|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__ec2__route53.deploy-cfn-route53-recordset-unsupported-eip-getatt-attribute-rejected|run#12|AssertionError|assert rec_failures == [_BAD_ATTR_REASON], (
Issue-Commit: 464916eef2b3c982c1baee9561b862edbea1b318

# cloudformation__ec2__route53: 2 net-new causes — Fn::GetAtt on AWS::EC2::EIP resolves to nothing (so a Route53 RecordSet in the same stack is created with an empty ResourceRecords and rolls the stack back), and an attribute the resource type does not declare is not rejected

> _Note: this issue was identified against the code base at commit `464916eef2b3c982c1baee9561b862edbea1b318`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in CloudFormation's intrinsic-function resolution for AWS::EC2::EIP, grounded in the two failing cloudformation__ec2__route53 checks.

=== Cause 1 of 2: Fn::GetAtt [<EIP>, 'PublicIp'] resolves to nothing, so the consuming AWS::Route53::RecordSet is submitted with an empty ResourceRecords list and the stack rolls back. ===
AWS: recorded behavior only (the CFN resource schemas are not part of the route53/ec2 Smithy models). AWS::EC2::EIP exposes the attribute `PublicIp`, and `Fn::GetAtt [Eip, PublicIp]` resolves to the Elastic IP address EC2 allocated — the same value `Ref` of that EIP returns and the same value DescribeAddresses reports as PublicIp. A RecordSet whose `ResourceRecords: [{Fn::GetAtt: [Eip, PublicIp]}]` therefore creates a one-value A record and the stack reaches CREATE_COMPLETE.
Emulator: the GetAtt-fed resource fails. The stack ends `ROLLBACK_COMPLETE` with resource rows `[('Eip','CREATE_COMPLETE'), ('Zone','CREATE_COMPLETE'), ('RecordGetAtt','CREATE_FAILED'), ('RecordRef','CREATE_COMPLETE'), ...]` — the `Ref`-fed sibling record in the SAME stack succeeds, which isolates the fault to GetAtt. The companion check shows what the downstream Route 53 call received, verbatim: `1 validation error detected: Value '0 items' at 'changeBatch.changes.1.member.resourceRecordSet.resourceRecords' failed to satisfy constraint: Member must have length greater than or equal to 1` — i.e. the unresolved GetAtt was passed down as an EMPTY list rather than resolved or reported as a template error.
Why: any template that wires an allocated address into DNS — the canonical EIP + Route 53 A-record pattern — cannot be deployed: the stack rolls back and the caller sees a Route 53 length-constraint error about `changeBatch` it never wrote, instead of either a working record or a CloudFormation error naming the attribute.
Fix: the intrinsic-function resolver for Fn::GetAtt in the CloudFormation deploy engine, specifically the AWS::EC2::EIP resource provider's attribute projection (it must expose PublicIp, and the resolver must not substitute an empty value for an unresolved attribute).
Traces: cloudformation__ec2__route53.deploy-cfn-ec2-eip-getatt-publicip-feeds-route53-recordset-same-create

=== Cause 2 of 2: Fn::GetAtt naming an attribute the resource type does not declare is not rejected; the failure instead surfaces as a Route 53 validation error. ===
AWS: recorded behavior only. `Fn::GetAtt [Eip, PrivateIpAddress]` on AWS::EC2::EIP names an attribute that type's schema does not declare, and CloudFormation fails the consuming resource with the ResourceStatusReason exactly `Requested attribute PrivateIpAddress does not exist in schema for AWS::EC2::EIP` — fixed text with two values from the template: the requested attribute name and the resource type name.
Emulator: the record resource's CREATE_FAILED reason is instead the downstream service's generic envelope: `Resource handler returned message: "1 validation error detected: Value '0 items' at 'changeBatch.changes.1.member.resourceRecordSet.resourceRecords' failed to satisfy constraint: Member must have length greater than or equal to 1 (Service: Route53, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — the request id and token are per-run values; the defect is that the refusal is a Route 53 length error at all. The unknown attribute was silently resolved to nothing and handed to the service.
Why: the author of a template with a mistyped attribute is told the wrong thing by the wrong service — the diagnosis points at a changeBatch member they never wrote rather than at the attribute name, and nothing distinguishes a mistyped attribute from a resource that genuinely produced no value.
Fix: attribute validation in the CloudFormation template/GetAtt resolution path — an attribute absent from the resource type's schema must fail the resource with CFN's schema message before any service call is made.
Traces: cloudformation__ec2__route53.deploy-cfn-route53-recordset-unsupported-eip-getatt-attribute-rejected

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service heading cloudformation__ec2__route53: 2 failing checks, 2 causes.

Cause 1 — cloudformation__ec2__route53.deploy-cfn-ec2-eip-getatt-publicip-feeds-route53-recordset-same-create (check line 196). Template: Eip (AWS::EC2::EIP, Domain vpc), Zone (AWS::Route53::HostedZone), RecordGetAtt (AWS::Route53::RecordSet, ResourceRecords [{Fn::GetAtt: [Eip, PublicIp]}]), RecordRef (same but [{Ref: Eip}]).
  Expected: CREATE_COMPLETE, and the GetAtt record's ResourceRecords == [the allocated PublicIp].
  Actual: AssertionError: stack 'sim2real-cfneiprs-...' ended 'ROLLBACK_COMPLETE': [('Eip','CREATE_COMPLETE'), ('Zone','CREATE_COMPLETE'), ('RecordGetAtt','CREATE_FAILED'), ('RecordRef','CREATE_COMPLETE'), ('RecordGetAtt','DELETE_COMPLETE'), ('RecordRef','DELETE_COMPLETE'), ('Zone','DELETE_COMPLETE'), ('Eip','DELETE_COMPLETE'), (<stack>,'ROLLBACK_COMPLETE')]

Cause 2 — cloudformation__ec2__route53.deploy-cfn-route53-recordset-unsupported-eip-getatt-attribute-rejected (check line 183):
  Expected Rec's CREATE_FAILED reason: ['Requested attribute PrivateIpAddress does not exist in schema for AWS::EC2::EIP']
  Actual: ['Resource handler returned message: "1 validation error detected: Value \'0 items\' at \'changeBatch.changes.1.member.resourceRecordSet.resourceRecords\' failed to satisfy constraint: Member must have length greater than or equal to 1 (Service: Route53, Status Code: 400, Request ID: a014e591-...)(SDK Attempt Count: 1)" (RequestToken: e8c9fc7f-..., HandlerErrorCode: InvalidRequest)']
  The '0 items' is the direct evidence that the GetAtt value arrived as an empty list.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Cause 1: CreateStack with resources Eip (AWS::EC2::EIP, Domain=vpc), Zone (AWS::Route53::HostedZone), RecordGetAtt (AWS::Route53::RecordSet, HostedZoneId {Ref: Zone}, Type A, TTL 60, ResourceRecords [{Fn::GetAtt: [Eip, PublicIp]}]) and RecordRef (identical but ResourceRecords [{Ref: Eip}]). Poll DescribeStacks: the stack ends ROLLBACK_COMPLETE with RecordGetAtt CREATE_FAILED while RecordRef reached CREATE_COMPLETE.
3. Cause 2: the same template with ResourceRecords [{Fn::GetAtt: [Eip, PrivateIpAddress]}]; read DescribeStackEvents for the record resource's ResourceStatusReason — it is a Route 53 '0 items' validation envelope instead of 'Requested attribute PrivateIpAddress does not exist in schema for AWS::EC2::EIP'.
4. Replay cloudformation__ec2__route53.deploy-cfn-ec2-eip-getatt-publicip-feeds-route53-recordset-same-create and cloudformation__ec2__route53.deploy-cfn-route53-recordset-unsupported-eip-getatt-attribute-rejected.
