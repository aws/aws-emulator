Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.deploy-cfn-ec2-later-failure-rolls-back-eni-before-its-security-group
Issue-Key: cloudformation__ec2.deploy-cfn-ec2-later-failure-rolls-back-eni-before-its-security-group|run#35|AssertionError|assert parsed is not None, f"Bad's failure is not a handler message: {failures[0][2]!r}"
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# cloudformation__ec2: a failed resource's ResourceStatusReason renders the relayed EC2 error in the legacy SDK-v1 envelope (semicolons, Error Code:, Proxy: null) instead of AWS's comma-separated envelope

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Cause 1 of 1: CloudFormation's ResourceStatusReason for a CREATE_FAILED resource is serialized in the retired AWS SDK for Java v1 shape instead of the shape AWS emits today.

AWS: recorded behavior only (CloudFormation's ResourceStatusReason text is not modelled; DescribeStackEvents.StackEvents[].ResourceStatusReason is a plain String in the Smithy model). When a resource provider relays a service refusal, AWS words the reason as the fixed template `Resource handler returned message: "<service message> (Service: <ServiceId>, Status Code: <http status>, Request ID: <request id>)" (RequestToken: <token>, HandlerErrorCode: <code>)`. The separators between the Service / Status Code / Request ID segments are COMMAS, and the envelope carries exactly those three segments - no `Error Code:` segment and no `Proxy: null` segment. Fixed text: `Resource handler returned message: "`, ` (Service: `, `, Status Code: `, `, Request ID: `, `)" (RequestToken: `, `, HandlerErrorCode: `, `)`. Variable parts taken from the relayed refusal and the request: the service message (here `The CIDR '<cidr>' is invalid.` for a subnet block outside the VPC), the ServiceId (`Ec2`), the HTTP status (`400`), the request id, the stack operation's request token, and the HandlerErrorCode (`InvalidRequest`).

Emulator: the reason reads `Resource handler returned message: "The CIDR '10.84.4.0/24' is invalid. (Service: Ec2; Status Code: 400; Error Code: InvalidSubnet.Range; Request ID: b0666568-dec4-f220-bd7f-be261dcb6bd2; Proxy: null)" (RequestToken: bafbbe6e-5c27-d719-030c-c359eccbfe63, HandlerErrorCode: InvalidRequest)` - semicolon separators, an extra `Error Code: <ec2 code>` segment and a trailing `Proxy: null` segment. (The cidr, request id and request token above are this run's values filling the template, not part of it.)

Why: a client or operator that parses the handler reason to recover which service refused, with what status and what request id - the standard way a rolled-back stack is diagnosed, and what CDK/CloudFormation tooling scrapes - fails to match the envelope at all and so cannot attribute the failure. The rollback itself is correct here; only the reason's wording diverges.

Fix: the CloudFormation resource-provider error relay that formats a downstream service refusal into ResourceStatusReason (the handler-message serializer shared by the EC2 resource handlers), not the EC2 operation - EC2's own InvalidSubnet.Range refusal is right.

Note: sibling issues already describe this same envelope defect for other resource families (open: 20261111T095650Z-cloudformation-cloudfront-ec2-elbv2-a-cloudformation-resource-handler.md, 20261126T041316Z-cloudformation-cloudwatch-the-create-failed-reason-for-a.md). Neither covers the cloudformation__ec2 check below, which the suite reports as unowned.

Traces: cloudformation__ec2.deploy-cfn-ec2-later-failure-rolls-back-eni-before-its-security-group.

## Evidence
Expected (AWS): failures[0].ResourceStatusReason matches `^Resource handler returned message: "(?<text>.*) \(Service: (?<svc>\w+), Status Code: (?<http>\d+), Request ID: [^)]*\)" \(RequestToken: [^,]*, HandlerErrorCode: (?<kind>\w+)\)$` with text="The CIDR '10.84.4.0/24' is invalid.", svc=Ec2, http=400, kind=InvalidRequest.

Actual (emulator): 'Resource handler returned message: "The CIDR \'10.84.4.0/24\' is invalid. (Service: Ec2; Status Code: 400; Error Code: InvalidSubnet.Range; Request ID: b0666568-dec4-f220-bd7f-be261dcb6bd2; Proxy: null)" (RequestToken: bafbbe6e-5c27-d719-030c-c359eccbfe63, HandlerErrorCode: InvalidRequest)' -> AssertionError: Bad's failure is not a handler message.

## Steps to Reproduce
1. CreateStack with resources Vpc (10.83.0.0/16), Subnet (10.83.4.0/24), Sg, Eni, plus a late `Bad` AWS::EC2::Subnet (DependsOn: Eni) whose CidrBlock 10.84.4.0/24 lies outside the VPC.
2. Wait for the stack to settle at ROLLBACK_COMPLETE.
3. DescribeStackEvents and read the ResourceStatusReason of the `Bad` CREATE_FAILED event.
4. Observe the semicolon-separated envelope carrying `Error Code:` and `Proxy: null` instead of AWS's comma-separated `(Service: Ec2, Status Code: 400, Request ID: ...)`.
