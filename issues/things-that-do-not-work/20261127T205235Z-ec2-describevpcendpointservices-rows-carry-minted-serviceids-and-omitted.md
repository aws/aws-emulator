Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-vpc-endpoint-services
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-vpc-endpoint-services
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceNameSet
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.baseEndpointDnsNameSet.item
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.serviceId
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.vpcEndpointPolicySupported
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.supportedIpAddressTypeSet.item
Issue-Related: 20261116T190237Z-ec2-region-availability-zone-and-vpc-endpoint-service.md
Issue-Related: 20261123T135115Z-ec2-describevpcendpointservices-serves-a-synthesized-service-catalog-not.md
Issue-Related: 20261124T114924Z-ec2-describevpcendpointservices-servicedetailset-rows-are-minted-locally-missing.md
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ec2 DescribeVpcEndpointServices rows carry minted serviceIds and omitted members, and filtered queries answer empty

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpointServices

## Description
The VPC endpoint-service catalog served by DescribeVpcEndpointServices is minted locally: whole services are missing for a filtered query, serviceIds are generated rather than AWS's, per-service flags are wrong, and members AWS always emits are omitted. This is a BUNDLE OF 4 CAUSES; all four must be closed.

Cause 1 of 4: a ServiceName-filtered query answers an EMPTY catalog.
AWS: recorded behavior only. A DescribeVpcEndpointServices call naming a service answers 200 with serviceNameSet.item listing the matching service names and serviceDetailSet.item carrying one structure per match (acceptanceRequired, availabilityZoneSet/availabilityZoneIdSet, baseEndpointDnsNameSet, managesVpcEndpoints, owner 'amazon', privateDnsName, privateDnsNameVerificationState, privateDnsNameSet, serviceId, serviceName, serviceRegion, serviceType, supportedIpAddressTypeSet, tagSet, vpcEndpointPolicySupported).
Emulator: both serviceDetailSet and serviceNameSet are empty strings — the service simply is not in the emulator's catalog, so the query matches nothing. This one cluster covers 16 recorded steps.
Why: a client cannot discover or create an interface endpoint for any service outside the emulator's short built-in list; the endpoint-creation workflow stops at discovery.
Fix: the VPC endpoint-service catalog behind DescribeVpcEndpointServices — populate it from the recorded AWS catalog and evaluate the ServiceName filter against it.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#4 (cluster spans 16 steps of that fixture).

Cause 2 of 4: serviceId is generated instead of AWS's, and privateDnsNameSet is serialised as a single entry where AWS returns a list.
AWS: recorded behavior only. serviceId is the catalog's stable 'vpce-svc-' + 17 hex chars value for that service name; privateDnsNameSet.item is a LIST (a service with a FIPS alias returns both private DNS names).
Emulator: a different, locally minted serviceId for the same serviceName, and privateDnsNameSet.item collapsed to a single structure (type_mismatch).
Why: an endpoint created against the wrong serviceId cannot be correlated with AWS, and a client enumerating privateDnsNameSet sees only the first name, so the FIPS alias is lost.
Fix: the endpoint-service catalog's id assignment and the list serialisation of privateDnsNameSet.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#1.

Cause 3 of 4: vpcEndpointPolicySupported is wrong for a service that does not support endpoint policies, and privateDnsName is region-scoped for a GLOBAL service.
AWS: recorded behavior only. vpcEndpointPolicySupported reflects the per-service capability ('false' for the recorded service), and a global service's privateDnsName is the global form ('<service>.global.api.aws'), not a regionalised one.
Emulator: vpcEndpointPolicySupported 'true' for that service, and a regionalised privateDnsName; serviceId also minted (see cause 2).
Why: a client that attaches a policy to an endpoint whose service does not support one is told it may, and a global service's private DNS name is wrong, so name-based reachability differs.
Fix: the per-service capability flags and private-DNS naming (global vs regional) in the endpoint-service catalog.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#2.

Cause 4 of 4: payerResponsibility omitted and supportedIpAddressTypeSet wrong.
AWS: recorded behavior only. AWS emits payerResponsibility ('ServiceOwner' for an AWS-owned service) and reports supportedIpAddressTypeSet.item as exactly the families the service supports (the recorded service is IPv4-only).
Emulator: payerResponsibility absent, and supportedIpAddressTypeSet.item carries ['ipv4','ipv6'] for an IPv4-only service.
Why: a client that selects IpAddressType=ipv6 on the strength of the advertised set creates an endpoint AWS would refuse, and an SDK field for payerResponsibility reads null.
Fix: the endpoint-service catalog's payerResponsibility and supported-IP-address-family data.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#3.

The replay also flags baseEndpointDnsNameSet entries served under a '.vpce.localhost' suffix where the recording has '.vpce.amazonaws.com'; the suffix is environment-dependent, so it is reported here as context only and is not part of the requirement above.

Related open issue (does not carry these replay keys): 20261124T114924Z-ec2-describevpcendpointservices-servicedetailset-rows-are-minted-locally-missing.md.

## Evidence
Cause 1 — 200/200: $.DescribeVpcEndpointServicesResponse.serviceDetailSet type_mismatch (expected {'item': [ {serviceName 'aws.api.us-west-2.kafka-api', serviceId 'vpce-svc-08a20d48733eddfc6', acceptanceRequired 'true', availabilityZoneIdSet ['usw2-az2','usw2-az1','usw2-az3','usw2-az4'], ...}, {serviceName 'aws.api.us-west-2.kafka-api-fips', ...} ]}, got ''); serviceNameSet type_mismatch (expected {'item': ['aws.api.us-west-2.kafka-api','aws.api.us-west-2.kafka-api-fips']}, got ''); affected_count 16.
Cause 2 — 200/200: serviceId expected 'vpce-svc-0ee30985e06084f10' got 'vpce-svc-97e6b3275ede2efb5'; privateDnsNameSet.item type_mismatch (expected [{'privateDnsName': 'cassandra-streams.us-west-2.api.aws'}, {'privateDnsName': 'cassandra-streams-fips.us-west-2.api.aws'}], got a single structure).
Cause 3 — 200/200: serviceId expected 'vpce-svc-071d9fedb23aa1344' got 'vpce-svc-4d8f2eab55353b3c3'; vpcEndpointPolicySupported expected 'false' got 'true'; privateDnsName expected 'codecatalyst.global.api.aws' got 'codecatalyst.us-west-2.api.aws'.
Cause 4 — 200/200: payerResponsibility missing (expected 'ServiceOwner', got None); serviceId expected 'vpce-svc-0109061b704c5f476' got 'vpce-svc-2fd9d201a4db8d072'; supportedIpAddressTypeSet.item expected 'ipv4' got ['ipv4','ipv6'].
The quoted service names and ids are the recorded catalog's values; the requirement is that the catalog's content, flags, member set and list shapes match AWS's.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: ec2-catalog-describe-vpc-endpoint-services step task#4 — DescribeVpcEndpointServices filtered to a service name the emulator's catalog lacks; observe empty serviceDetailSet/serviceNameSet.
3. Cause 2: step task#1 — compare serviceId and privateDnsNameSet shape.
4. Cause 3: step task#2 — compare vpcEndpointPolicySupported and privateDnsName for a global service.
5. Cause 4: step task#3 — compare payerResponsibility and supportedIpAddressTypeSet.
