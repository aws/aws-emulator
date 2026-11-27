Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-images-amazon-linux-2
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-images-amazon-linux-2 ec2-catalog-describe-images-amazon-linux-2023 ec2-catalog-describe-images-deep-learning ec2-catalog-describe-images-ecs-optimized ec2-catalog-describe-images-eks-optimized ec2-catalog-describe-images-windows-server-2025-base ec2-catalog-describe-images-windows-server-base ec2-catalog-get-image-ancestry ec2-catalog-describe-snapshots ec2-catalog-describe-managed-prefix-lists ec2-catalog-describe-prefix-lists ec2-vpc-attributes-and-account-settings ec2-catalog-get-managed-prefix-list-entries
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeImages|2xx->4xx|$.DescribeImagesResponse
Issue-Key: ec2|DescribeImages|2xx->4xx|status
Issue-Key: ec2|GetImageAncestry|2xx->4xx|$.GetImageAncestryResponse
Issue-Key: ec2|GetImageAncestry|2xx->4xx|status
Issue-Key: ec2|DescribeSnapshots|2xx->2xx|$.DescribeSnapshotsResponse.snapshotSet.item[*]
Issue-Key: ec2|DescribeSnapshots|2xx->2xx|$.DescribeSnapshotsResponse.snapshotSet
Issue-Key: ec2|DescribeManagedPrefixLists|2xx->2xx|$.DescribeManagedPrefixListsResponse.prefixListSet.item[*]
Issue-Key: ec2|DescribePrefixLists|2xx->2xx|$.DescribePrefixListsResponse.prefixListSet.item[*]
Issue-Key: ec2|DescribePrefixLists|2xx->2xx|$.DescribePrefixListsResponse.prefixListSet.item.cidrSet.item
Issue-Key: ec2|GetManagedPrefixListEntries|2xx->2xx|$.GetManagedPrefixListEntriesResponse.entrySet.item[*]
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ec2 AWS-owned AMI, snapshot and prefix-list catalogs are locally minted or refused

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
The read-only, AWS-owned catalogs that ec2 serves (public AMIs, public snapshots, AWS-managed prefix lists) are either refused outright or synthesised locally instead of being served from the recorded AWS catalog. The shared fault: these describe/get handlers have no authoritative catalog behind them, so they answer with invented rows, invented ids, or a parameter refusal. This is a BUNDLE OF 8 CAUSES; all eight must be closed.

Cause 1 of 8: DescribeImages for public AMIs is refused 400.
AWS: recorded behavior only (DescribeImages models no errors). A DescribeImages call that selects public AMIs (owner alias 'amazon' plus name/architecture/root-device filters) answers 200 with an imagesSet whose items carry the full modeled image shape — imageId, imageLocation, imageState, imageOwnerId, creationDate, isPublic, architecture, imageType, sriovNetSupport, imageOwnerAlias, name, description, rootDeviceType, rootDeviceName, blockDeviceMapping (with ebs snapshotId/volumeSize/volumeType/encrypted/deleteOnTermination), virtualizationType, hypervisor, enaSupport, platformDetails, usageOperation, deprecationTime, freeTierEligible, publicSsmParameterName.
Emulator: 400 with no DescribeImagesResponse at all.
Why: AMI lookup is the first step of nearly every launch workflow (Amazon Linux 2/2023, ECS- and EKS-optimized, Deep Learning); refusing it makes those workflows unrunnable. This single cluster covers 33 recorded steps.
Fix: the DescribeImages handler and the public-AMI catalog it reads (owner-alias/filter resolution over the recorded catalog) in the ec2 crate's image state.
Traces: ec2-catalog-describe-images-amazon-linux-2#task#1 (cluster spans 33 steps across ec2-catalog-describe-images-amazon-linux-2, -amazon-linux-2023, -deep-learning, -ecs-optimized, -eks-optimized).

Cause 2 of 8: GetImageAncestry is refused 400.
AWS: Smithy: GetImageAncestry(ImageId required) -> GetImageAncestryResult.ImageAncestryEntries (xmlName imageAncestryEntrySet); no modeled errors. For a public AMI AWS answers 200 with imageAncestryEntrySet items carrying imageId, imageOwnerAlias and creationDate.
Emulator: 400 with no GetImageAncestryResponse.
Why: image-provenance queries fail entirely; 11 recorded steps are blocked by this one cluster.
Fix: the GetImageAncestry handler — resolve the ancestry from the same AMI catalog as cause 1.
Traces: ec2-catalog-get-image-ancestry#task#1 (cluster spans 11 steps of that fixture).

Cause 3 of 8: DescribeSnapshots over the public (amazon-owned) snapshot catalog returns different rows.
AWS: recorded behavior only. The public snapshot catalog rows carry snapshotId, volumeId, status 'completed', startTime, progress '100%', ownerId, volumeSize, description, ownerAlias 'amazon', encrypted, storageTier 'standard', transferType 'standard', completionTime and fullSnapshotSizeInBytes.
Emulator: the expected snapshot rows are absent from snapshotSet (reported 'missing ... got None'), i.e. the emulator serves a different (locally minted) set.
Why: any workflow that locates a public snapshot by description or owner alias finds nothing.
Fix: the DescribeSnapshots handler's handling of AWS-owned snapshots and the snapshot catalog it reads.
Traces: ec2-catalog-describe-snapshots#task#1.

Cause 4 of 8: DescribeSnapshots with a restricting filter returns the wrong shape for the result set.
AWS: recorded behavior only. The filtered query answers a snapshotSet with an 'item' LIST of the matching catalog rows.
Emulator: snapshotSet is a different type than the expected list-of-items (type_mismatch), so the filtered projection over the catalog is wrong as well as its contents.
Why: an SDK deserialising snapshotSet.item as a list of structures mis-parses or sees one element; filters over the public catalog are unusable.
Fix: DescribeSnapshots filter evaluation and the XML list serialisation of snapshotSet for the catalog path.
Traces: ec2-catalog-describe-snapshots#task#2.

Cause 5 of 8: DescribeManagedPrefixLists serves invented AWS-managed prefix lists.
AWS: recorded behavior only. Each AWS-managed row carries prefixListId in the long form ('pl-' + 17 hex chars), prefixListName 'com.amazonaws.<region>[.ipv6].<service>', prefixListArn 'arn:aws:ec2:<region>:aws:prefix-list/<prefixListId>', addressFamily, state 'create-complete', ownerId 'AWS', empty tagSet — and the set covers the services AWS publishes in that Region (route53-healthchecks, ec2-instance-connect, ...).
Emulator: rows for different services with short legacy ids (e.g. 'pl-00a54069'), and the expected rows are missing entirely; address families are attached to the wrong names (an IPv4 row expected, an IPv6 row served).
Why: a client that resolves a managed prefix list by name (the documented way to reference AWS service ranges in a security group) gets no match or the wrong list, and the prefixListArn it records is wrong.
Fix: the AWS-managed prefix-list catalog behind DescribeManagedPrefixLists (names, long-form ids, ARNs, address families).
Traces: ec2-catalog-describe-managed-prefix-lists#task#1.

Cause 6 of 8: DescribePrefixLists serves invented ids and CIDR sets.
AWS: recorded behavior only. Each row carries prefixListId, prefixListName and a cidrSet whose item list is the service's published ranges (several real public CIDRs per service).
Emulator: different prefixListIds for the same prefixListName and a single synthetic CIDR per row (documentation/test ranges such as 198.18.x.x and 2001:db8::/48).
Why: a client that resolves a service's ranges to build routes or security-group rules gets non-routable placeholder CIDRs.
Fix: the prefix-list catalog behind DescribePrefixLists — id assignment per service name and the published CIDR lists.
Traces: ec2-catalog-describe-prefix-lists#task#1.

Cause 7 of 8: DescribePrefixLists cidrSet is a single value where AWS returns a multi-entry list (second, independently recorded occurrence outside the catalog fixtures).
AWS: recorded behavior only. cidrSet.item is a LIST of the service's CIDRs (for com.amazonaws.<region>.s3 the recording shows ten entries).
Emulator: cidrSet.item is one scalar CIDR (type_mismatch), so even the XML shape differs from AWS's repeated-element list.
Why: an SDK deserialising cidrSet.item as a list of strings receives one element (or mis-parses), so range enumeration silently truncates.
Fix: the cidrSet serialisation for DescribePrefixLists (always a repeated item list) plus the catalog content from cause 6.
Traces: ec2-vpc-attributes-and-account-settings#task#8.

Cause 8 of 8: GetManagedPrefixListEntries returns no entries for an AWS-managed prefix list.
AWS: recorded behavior only. The response's entrySet.item list carries one {cidr} entry per published range of the named prefix list.
Emulator: the expected entries are absent (entrySet items 'missing ... got None'), so the entry list is empty or minted differently.
Why: resolving an AWS-managed prefix list to its concrete CIDRs — the supported way to pin AWS service ranges — returns nothing.
Fix: GetManagedPrefixListEntries over the same managed prefix-list catalog as cause 5.
Traces: ec2-catalog-get-managed-prefix-list-entries#task#2.

The concrete ids, CIDRs, AMI ids and timestamps quoted in the recordings are the catalog's recorded content for that Region; the requirement is that these read-only catalogs are served from the recorded AWS data with AWS's member set and list shapes, not minted locally.

## Evidence
Cause 1 — DescribeImages: status expected 200, got 400; $.DescribeImagesResponse missing (expected an imagesSet item with imageId 'ami-09179e36e2b518604', imageLocation 'amazon/amzn2-ami-kernel-5.10-hvm-2.0.20260930.0-x86_64-gp2', imageOwnerId '137112412989', imageOwnerAlias 'amazon', blockDeviceMapping, platformDetails 'Linux/UNIX', publicSsmParameterName ..., got None); affected_count 33.
Cause 2 — GetImageAncestry: status expected 200, got 400; $.GetImageAncestryResponse missing (expected imageAncestryEntrySet item {creationDate, imageId 'ami-00e236d9d38e7c625', imageOwnerAlias 'amazon'}, got None); affected_count 11.
Cause 3 — DescribeSnapshots: 200/200, snapshotSet.item[0..41] missing (expected rows such as {snapshotId 'snap-001a9a1d8e527da00', volumeId 'vol-ffffffff', status 'completed', ownerAlias 'amazon', storageTier 'standard', transferType 'standard', fullSnapshotSizeInBytes '1048576'}, got None).
Cause 4 — DescribeSnapshots: 200/200, $.DescribeSnapshotsResponse.snapshotSet type_mismatch (expected {'item': [ ... list of catalog rows ... ]}).
Cause 5 — DescribeManagedPrefixLists: 200/200, prefixListSet.item[0..15] missing (expected e.g. {prefixListId 'pl-0068613c321dee54b', prefixListName 'com.amazonaws.us-west-2.route53-healthchecks', prefixListArn 'arn:aws:ec2:us-west-2:aws:prefix-list/pl-0068613c321dee54b', addressFamily 'IPv4', ownerId 'AWS'}, got {prefixListId 'pl-00a54069', prefixListName 'com.amazonaws.us-west-2.dynamodb'}).
Cause 6 — DescribePrefixLists: 200/200, prefixListSet.item[0..5] missing (expected e.g. {prefixListId 'pl-68a54001', prefixListName 'com.amazonaws.us-west-2.s3', cidrSet.item [ten public CIDRs]}, got the same name with cidrSet.item '198.18.96.0/21').
Cause 7 — DescribePrefixLists: 200/200, $.DescribePrefixListsResponse.prefixListSet.item.cidrSet.item type_mismatch (expected a 10-element list, got the single string '198.18.96.0/21').
Cause 8 — GetManagedPrefixListEntries: 200/200, entrySet.item[0..2] missing (expected {'cidr': '35.71.64.0/22'}, {'cidr': '52.94.10.0/24'}, {'cidr': '52.94.28.0/23'}, got None).

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: ec2-catalog-describe-images-amazon-linux-2 step task#1 — DescribeImages filtered to the public Amazon Linux 2 AMI.
3. Cause 2: ec2-catalog-get-image-ancestry step task#1 — GetImageAncestry for a public AMI id.
4. Causes 3-4: ec2-catalog-describe-snapshots steps task#1 and task#2 — DescribeSnapshots over the public snapshot catalog, unfiltered and filtered.
5. Cause 5: ec2-catalog-describe-managed-prefix-lists step task#1 — DescribeManagedPrefixLists.
6. Causes 6-7: ec2-catalog-describe-prefix-lists step task#1 and ec2-vpc-attributes-and-account-settings step task#8 — DescribePrefixLists.
7. Cause 8: ec2-catalog-get-managed-prefix-list-entries step task#2 — GetManagedPrefixListEntries for an AWS-managed list.
8. Compare ids, names, ARNs, CIDR lists and list shapes with the fixture expectations.
