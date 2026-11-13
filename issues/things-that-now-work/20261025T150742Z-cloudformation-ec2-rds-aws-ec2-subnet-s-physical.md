Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative
Issue-Key: cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative|run#13|AssertionError|assert "(Service: Rds" in reasons, (
Issue-Commit: d2e421bd781628705310382509b1712cb5427402

# cloudformation__ec2__rds: AWS::EC2::Subnet's physical resource id is a stack-generated name, not an EC2 subnet id, so every cross-service consumer of a Ref'd subnet is refused (1 cause)

> _Note: this issue was identified against the code base at commit `d2e421bd781628705310382509b1712cb5427402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation mints a logical-name-based physical resource id for AWS::EC2::Subnet instead of the subnet id EC2 assigned, so a downstream resource that consumes {"Ref": "SubnetA"} is handed a string no AWS service can resolve, and the stack fails for the wrong reason with the wrong attribution.

AWS: recorded behavior only for the resource-type mapping (AWS::EC2::Subnet is a CloudFormation resource type); the authoritative shape is EC2's own: a subnet id is `subnet-` followed by 8 or 17 hexadecimal characters, and Ref on AWS::EC2::Subnet returns exactly that id. In the recorded run a template that creates a VPC, two subnets from !GetAZs and an AWS::RDS::DBSubnetGroup over them must, when both subnets land in ONE availability zone, fail on the DBSubnetGroup with RDS's own error: the CREATE_FAILED event's ResourceStatusReason is `Resource handler returned message: "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <az>. Add subnets to cover at least 2 AZs. (Service: Rds, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. Fixed text: `Resource handler returned message:`, `The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage:`, `Add subnets to cover at least 2 AZs.`, `(Service: Rds, Status Code: 400, Request ID:`, `(RequestToken:`, `HandlerErrorCode: InvalidRequest)`. Variable parts taken from state/request: the AZ name, the per-call Request ID and the CloudFormation RequestToken. The stack then rolls back to ROLLBACK_COMPLETE and rds:DescribeDBSubnetGroups(DBSubnetGroupName=<name>) answers DBSubnetGroupNotFoundFault with message `DB Subnet group '<name>' not found.`
Emulator: the subnets are created but their physical resource id / Ref value is the stack-generated name `<stack-name>-SubnetA-<hex>` rather than a `subnet-...` id. RDS therefore never gets as far as the AZ-coverage rule: the DBSubnetGroup's CREATE_FAILED reason is `Resource handler returned message: "<stack-name>-SubnetA-<hex> is not an EC2 subnet id (expected subnet- followed by 8 or 17 hexadecimal characters)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. The failure is no longer attributed to RDS (`(Service: Rds` is absent), names an id-format problem instead of AZ coverage, and states no minimum AZ count.
Why: any stack that wires a CFN-created subnet into another service (RDS DB subnet groups, ELBv2, ECS, Lambda VPC config, ASG) is handed an unusable identifier, so correct templates fail and incorrect templates fail for the wrong reason. A client debugging the rollback is told its subnet id is malformed when the real defect it should see is its own AZ spread, and refusal-taxonomy tests that key on the originating service (`(Service: Rds`) cannot pass.
Fix: the AWS::EC2::Subnet resource provisioner must record the subnet id returned by ec2:CreateSubnet as the resource's PhysicalResourceId and as the value of Ref (and keep AvailabilityZone available for !GetAtt), instead of synthesizing `<stack>-<logical-id>-<hex>`. Related open issues describe the same family of symptom but do not carry this check's replay key: 20261024T120544Z-rds-db-subnet-group-fidelity-wrong-not-found.md and 20261025T014625Z-rds-above-range-maxrecords-refused-as-validationerror-not.md; 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md covers synthesized physical ids generally.
Traces: cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative.

## Evidence
Check cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative
  Expected (recorded against real CloudFormation, us-west-2): the SubnetGroup resource's CREATE_FAILED ResourceStatusReason contains all of `(Service: Rds`, `Availability Zone (AZ) coverage requirement`, `at least 2 AZs`, `HandlerErrorCode: InvalidRequest`.
  Actual: AssertionError: the stack event must attribute the failure to RDS, not to CloudFormation's own validation, got ' Resource handler returned message: "sim2real-xsvc-stk-<id>-SubnetA-<hex> is not an EC2 subnet id (expected subnet- followed by 8 or 17 hexadecimal characters)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'
  Root fact: the physical id handed to RDS for the Ref'd AWS::EC2::Subnet is `<stack-name>-SubnetA-<hex>`, not a `subnet-` + 8/17 hex id. (The <id>/<hex>/<uuid> parts are per-run values; the requirement is the id FORM and the RDS attribution, not those values.)

## Steps to Reproduce
1. cfn:CreateStack with a template that declares Vpc{Type:AWS::EC2::VPC}, SubnetA and SubnetB{Type:AWS::EC2::Subnet, Properties:{VpcId:{Ref:Vpc}, CidrBlock:..., AvailabilityZone: !Select from !GetAZs}} with both subnets pinned to the SAME availability zone, and SubnetGroup{Type:AWS::RDS::DBSubnetGroup, Properties:{DBSubnetGroupName:<name>, SubnetIds:[{Ref:SubnetA},{Ref:SubnetB}]}}.
2. Poll cfn:DescribeStacks until terminal, then cfn:DescribeStackEvents(StackName=...).
3. Read the SubnetGroup resource's CREATE_FAILED ResourceStatusReason: it reports the Ref'd subnet value is "not an EC2 subnet id" instead of RDS's AZ-coverage refusal, showing the Ref resolved to `<stack>-SubnetA-<hex>`.
4. Equivalently: cfn:DescribeStackResources(StackName=..., LogicalResourceId="SubnetA") and compare PhysicalResourceId with ec2:DescribeSubnets output — AWS's is a `subnet-...` id.
