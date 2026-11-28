Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-vpc-endpoint-services
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-vpc-endpoint-services
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item[*]
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item[*].privateDnsNameSet.item[*].privateDnsName
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceNameSet.item[*]
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.privateDnsNameSet.item[*].privateDnsName
Issue-Related: 20261109T225653Z-ec2-catalog-pages-r9g-z1d-and-vt1-plus.md
Issue-Related: 20261123T135115Z-ec2-describevpcendpointservices-serves-a-synthesized-service-catalog-not.md
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# ec2 DescribeVpcEndpointServices serves a divergent AWS-managed endpoint-service catalog

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpointServices

## Description
DescribeVpcEndpointServices answers 200 but the AWS-managed endpoint-service catalog it serves diverges from AWS's in membership, in the DNS names it advertises and in what a name-scoped query returns. 19 replay clusters on this operation have no owning issue; all 19 are in fixture ec2-catalog-describe-vpc-endpoint-services (verified: no diverging DescribeVpcEndpointServices cluster has any other fixture_id), spanning its catalog steps from task#15 to task#36. The open issue 20261127T205235Z-ec2-describevpcendpointservices-rows-carry-minted-serviceids-and-omitted.md names the same operation but does not carry these clusters' replay keys, so they are filed here; the whole step range of that fixture is in scope, not only the four steps named under Traces.

Cause 1 of 4: the catalog's membership and order do not match AWS's.
AWS: recorded behavior only for the row set; Smithy types the result as DescribeVpcEndpointServicesResult.ServiceNames (ValueStringList, xmlName serviceNameSet) and ServiceDetails (ServiceDetailSet, xmlName serviceDetailSet), and AWS returns one row per AWS-managed endpoint service in the Region, serviceNameSet and serviceDetailSet in agreement. The recordings show rows the emulator never serves (for example the com.amazonaws.<region>.b2bi row, and the fips variant rows such as com.amazonaws.<region>.xray-fips).
Emulator: the row at each index names a different service than the recording's row at that index, so the replay reports every serviceDetailSet.item[i] as `missing` plus the emulator's own rows as `extra`, and the corresponding serviceNameSet.item[i] entries mismatch. The emulator's catalog is missing whole services, which shifts every later index.
Why: a client that resolves an interface-endpoint service by scanning the catalog (or that asserts a service is available in the Region) concludes a real AWS endpoint service does not exist, and index-based reads land on the wrong service.
Fix: the DescribeVpcEndpointServices handler's endpoint-service catalog data (complete the row set and keep serviceNameSet aligned with serviceDetailSet).
Traces: ec2-catalog-describe-vpc-endpoint-services#task#15, ec2-catalog-describe-vpc-endpoint-services#task#16.

Cause 2 of 4: baseEndpointDnsNameSet advertises a non-AWS DNS suffix.
AWS: Smithy ServiceDetail.BaseEndpointDnsNames (ValueStringList, xmlName baseEndpointDnsNameSet) is documented as "The DNS names for the service"; for an AWS-managed service AWS returns the fixed form <service-endpoint-prefix>.<region>.vpce.amazonaws.com. The variable parts are the endpoint prefix and the Region; the ".vpce.amazonaws.com" suffix is fixed text.
Emulator: returns <service-endpoint-prefix>.<region>.vpce.localhost — the suffix is replaced with a host that is not the AWS form and not an address the service is reachable at.
Why: a client that builds an endpoint-specific DNS name from baseEndpointDnsNameSet (the documented use of the member) produces an unresolvable name, and any assertion on the AWS endpoint-DNS form fails.
Fix: the DescribeVpcEndpointServices serializer/catalog projection that renders baseEndpointDnsNameSet.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#15, ec2-catalog-describe-vpc-endpoint-services#task#35.

Cause 3 of 4: privateDnsNameSet omits the per-service variants AWS publishes.
AWS: Smithy ServiceDetail.PrivateDnsNames (PrivateDnsDetailsSet, xmlName privateDnsNameSet) carries every private DNS name assigned to the service. The recordings show services with four entries — the regional name, the .api.aws name, and the two corresponding -fips names — for services that have FIPS endpoints.
Emulator: serves only the regional and .api.aws pair for every service, so the -fips entries are absent.
Why: a client enabling private DNS and then resolving the FIPS name for a service that has one finds no such private DNS name.
Fix: the DescribeVpcEndpointServices catalog data for privateDnsNameSet.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#16.

Cause 4 of 4: a name-scoped query answers with no rows for services the emulator's catalog does not hold.
AWS: Smithy DescribeVpcEndpointServicesRequest.ServiceNames (ValueStringList, xmlName ServiceName) scopes the query; AWS answers 200 with one serviceDetailSet row per named service (the recording returns two rows for a service plus its -fips variant).
Emulator: answers 200 with serviceDetailSet.item[0] and item[1] both `got None` — an empty result — because the named services are absent from its catalog.
Why: a client that looks a service up by name to obtain its serviceId, availability zones or supported IP address types gets an empty answer and treats the service as unavailable in the Region.
Fix: the DescribeVpcEndpointServices handler's name-scoped lookup together with the catalog data it reads.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#36.

## Evidence
Replay cluster ec2-catalog-describe-vpc-endpoint-services#task#15 (expected 200, actual 200): $.DescribeVpcEndpointServicesResponse.serviceDetailSet.item[0] missing — expected the com.amazonaws.us-west-2.access-analyzer row with baseEndpointDnsNameSet item 'access-analyzer.us-west-2.vpce.amazonaws.com', got the same service with baseEndpointDnsNameSet item 'access-analyzer.us-west-2.vpce.localhost'. The cluster's divergence key lists serviceDetailSet.item[0..64] plus serviceNameSet.item[1..64] — i.e. the whole page diverges.

Replay cluster ec2-catalog-describe-vpc-endpoint-services#task#16 (expected 200, actual 200): expected serviceDetailSet.item[0] = com.amazonaws.us-west-2.b2bi with privateDnsNameSet entries ['b2bi.us-west-2.amazonaws.com', 'b2bi.us-west-2.api.aws', 'b2bi-fips.us-west-2.amazonaws.com', 'b2bi-fips.us-west-2.api.aws']; got com.amazonaws.us-west-2.backup with only ['backup.us-west-2.amazonaws.com', 'backup.us-west-2.api.aws'] and baseEndpointDnsNameSet 'backup.us-west-2.vpce.localhost' — the b2bi row is absent from the emulator's catalog, shifting the page.

Replay cluster ec2-catalog-describe-vpc-endpoint-services#task#35 (expected 200, actual 200): expected item[0] = com.amazonaws.us-west-2.wafv2 with 'wafv2.us-west-2.vpce.amazonaws.com'; got com.amazonaws.us-west-2.waf-regional with 'waf-regional.us-west-2.vpce.localhost'.

Replay cluster ec2-catalog-describe-vpc-endpoint-services#task#36 (expected 200, actual 200): expected item[0] = com.amazonaws.us-west-2.xray and item[1] = com.amazonaws.us-west-2.xray-fips (supportedIpAddressTypeSet item 'ipv4'); both reported `got None`, and the emulator's own rows come back as `extra`.

Smithy citations: DescribeVpcEndpointServicesRequest.ServiceNames (ValueStringList, xmlName ServiceName); DescribeVpcEndpointServicesResult.{ServiceNames -> serviceNameSet, ServiceDetails -> serviceDetailSet}; ServiceDetail.{BaseEndpointDnsNames -> baseEndpointDnsNameSet, PrivateDnsNames -> privateDnsNameSet, ServiceId, SupportedIpAddressTypes (length 0..2)}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-catalog-describe-vpc-endpoint-services, step task#15: an unfiltered DescribeVpcEndpointServices page. Compare serviceNameSet and serviceDetailSet row-by-row with the recording, and inspect each row's baseEndpointDnsNameSet suffix.
3. Replay step task#16 and look for the com.amazonaws.<region>.b2bi row and for four-entry privateDnsNameSet values including the -fips names.
4. Replay step task#35 (same comparison at the tail of the catalog) and step task#36 (a name-scoped DescribeVpcEndpointServices): task#36 answers 200 with an empty serviceDetailSet instead of the service and its -fips variant.
5. All four steps answer 200 with diverging catalog contents.
