Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-trajwire-lia-state-filter-value-mistakes-and-tagging-route
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-trajwire-lia-state-filter-value-mistakes-and-tagging-route
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: resourcegroupstaggingapi|GetResources|2xx->2xx|$.ResourceTagMappingList
Issue-Key: resourcegroupstaggingapi|GetResources|2xx->2xx|$.ResourceTagMappingList[*]
Issue-Commit: 588eab0496abf0a5bc1b8cec7adc2c989702bf8c

# resourcegroupstaggingapi GetResources never returns tagged EC2 resources (1 cause)

> _Note: this issue was identified against the code base at commit `588eab0496abf0a5bc1b8cec7adc2c989702bf8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Cause 1 of 1: the Resource Groups Tagging API serves an empty resource index, so resources the account tagged in EC2 are invisible to tag-based discovery.

AWS: Smithy: GetResources (POST /GetResources, awsJson RPC) takes TagFilters, ResourceTypeFilters, ResourceARNList and pagination inputs and answers HTTP 200 with ResourceTagMappingList; each entry carries ResourceARN and Tags. AWS indexes every taggable resource in the Region, so a GetResources constrained by a TagFilter (recorded: a Name tag) returns one entry per matching resource - the EC2 instance (arn:aws:ec2:<region>:<account>:instance/<instance-id>), the VPC (…:vpc/<vpc-id>) and the subnet (…:subnet/<subnet-id>) that were tagged earlier in the fixture - with their Tags echoed. The specific arns and tag values are environment data; the requirement is that every tagged resource appears, and that ResourceTypeFilters narrow that same set.
Emulator: HTTP 200 with an EMPTY ResourceTagMappingList on all three queries; the replay polled 15 times and the list stayed at 0 items, so this is not a propagation delay.
Why: tag-based discovery is how clients enumerate what they own (cost tooling, cleanup/reaper jobs, ops consoles). Against this emulator every such caller concludes the account has no tagged resources, and a reaper that deletes "untagged" resources would do the opposite of what the tags say.
Fix: the cross-service resource/tag index the GetResources handler reads - EC2 CreateTags/create-with-TagSpecifications must publish into it - plus the ResourceTypeFilters/TagFilters evaluation in that handler.
Traces: ec2-trajwire-lia-state-filter-value-mistakes-and-tagging-route#task#11 (single instance by tag), #task#12 (vpc, subnet, instance and one more resource by tag), #task#13 (type-filtered query).

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 divergence clusters, service=resourcegroupstaggingapi, verdict=mismatch, stub_path empty, owned_by empty):
1. #task#11 GetResources 200->200: $.ResourceTagMappingList[0] missing (expected {'ResourceARN': 'arn:aws:ec2:us-west-2:000000000000:instance/i-...', 'Tags': [{'Key': 'Name', 'Value': 'sim2real-probe-lia-…'}]}, got None); $.ResourceTagMappingList poll: 0 item(s), not 1 after 15 attempt(s) (got []).
2. #task#12 GetResources 200->200: entries [0]..[3] missing - expected vpc/vpc-…, subnet/subnet-…, instance/i-… and one further arn, each with the Name tag; got None.
3. #task#13 GetResources 200->200: entries [0]..[1] missing - expected vpc/vpc-… and subnet/subnet-…; got None.
Model evidence: smithy_query(query=operation, operation=GetResources, model_path=services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126.json) - input TagFilters/ResourceTypeFilters/ResourceARNList, output ResourceTagMappingList, http POST /GetResources code 200, paginated on ResourceTagMappingList.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture ec2-trajwire-lia-state-filter-value-mistakes-and-tagging-route up to step task#11: the fixture creates a VPC, subnet and instance and tags them (Key 'Name'), then calls GetResources with that TagFilter.
3. Inspect $.ResourceTagMappingList in the 200 response - it is empty and stays empty across the replay's 15 polls; repeat for steps task#12 and task#13 (multi-resource and type-filtered queries).
