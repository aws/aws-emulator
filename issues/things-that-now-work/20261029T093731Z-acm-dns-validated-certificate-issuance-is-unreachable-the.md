Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.dns-validated-certificate-issues-under-a-delegated-zone
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.dns-validated-certificate-issues-under-a-delegated-zone|_domain_zone#3|AssertionError|raise AssertionError(
Issue-Commit: 6c0e2feb75d7e0fede15568cd8941f97f9b3d17f

# acm: DNS-validated certificate issuance is unreachable — the account's tagged public hosted zone is not discoverable through route53 ListHostedZones/ListTagsForResource, so ACM has no delegated zone to validate in

> _Note: this issue was identified against the code base at commit `6c0e2feb75d7e0fede15568cd8941f97f9b3d17f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostedZones

## Description
One root cause, grounded in one net-new failing behavioral check that aborts in its very first step, before any ACM call is made.

Cause 1 of 1: the account's tagged PUBLIC hosted zone — the DNS baseline in which an ACM DNS-validated certificate publishes its validation CNAME — cannot be found through route53's own listing and tagging reads, so the whole ACM DNS-validation state machine (RequestCertificate -> PENDING_VALIDATION -> publish CNAME -> ISSUED) is never exercised. Failing check: acm.dns-validated-certificate-issues-under-a-delegated-zone.

AWS: Smithy (com.amazonaws.route53): ListHostedZones — input {Marker (httpQuery marker, length 0..64), MaxItems (httpQuery maxitems), DelegationSetId, HostedZoneType}, output ListHostedZonesResponse{HostedZones (required, list), Marker (required), IsTruncated (required, default false), NextMarker, MaxItems (required)}, GET /2013-04-01/hostedzone, code 200, @paginated inputToken=Marker outputToken=NextMarker items=HostedZones pageSize=MaxItems; modeled refusals InvalidInput(400), NoSuchDelegationSet(400), DelegationSetNotReusable(400). ListTagsForResource — input {ResourceType (httpLabel, required, enum TagResourceType), ResourceId (httpLabel, required, length 0..64)}, output ListTagsForResourceResponse{ResourceTagSet (required)}, GET /2013-04-01/tags/{ResourceType}/{ResourceId}, code 200; modeled refusals InvalidInput(400), NoSuchHostedZone(404), NoSuchHealthCheck(404), PriorRequestNotComplete(400), ThrottlingException(400). Beyond those signatures this is recorded behavior only: in the recorded account a PUBLIC hosted zone exists (Config.PrivateZone false) whose tag set contains the baseline tag {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'}; ListHostedZones enumerates it across its Marker/NextMarker pages and ListTagsForResource(ResourceType='hostedzone', ResourceId=<the zone id after the last '/'>) returns that tag in ResourceTagSet.Tags — both HTTP 200, with no refusal anywhere at this point in the recording. The requirement is general: a hosted zone created and tagged in this account must be enumerated by ListHostedZones and must report, through ListTagsForResource, the tags that were written on it. The particular zone id, apex domain and tag values are account values that fill the template; they are not part of the contract.

Emulator: both reads succeed (no non-2xx, no error code is reported), but the pagination completes with no enumerated public hosted zone carrying the baseline tag, so the check raises before issuing a single ACM request: `AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no delegated zone to validate a certificate in` (raised from the check's _domain_zone helper at line 42, reached from run line 74). The fixed text of that message is the harness's; the variable parts (the tag key/value and the zone apex) come from the request/state, not from one run. Two readings are consistent with the replay and the recording does not distinguish them — Hypothesis: either ListHostedZones enumerates no (or not this) public hosted zone for the account (or reports Config.PrivateZone true for it), or ListTagsForResource('hostedzone', <id>) answers with a ResourceTagSet that omits the tags ChangeTagsForResource wrote, so the zone is enumerated but never recognised.

Why: a client cannot discover, enumerate or tag-select its own DNS zones, so nothing that depends on a delegated public zone can run. Concretely, the entire ACM DNS-validation happy path is unverifiable behind this one read: RequestCertificate(ValidationMethod='DNS') publishing a CNAME challenge in DomainValidationOptions[].ResourceRecord, the certificate starting at Status PENDING_VALIDATION, the transition to ISSUED once the CNAME resolves, per-domain ValidationStatus SUCCESS, Type AMAZON_ISSUED, InUseBy empty, NotBefore < NotAfter, and indexing under ListCertificates(CertificateStatuses=['ISSUED']). Any tag-driven discovery tooling (`aws route53 list-hosted-zones` + `list-tags-for-resource`, Terraform/CDK data sources, inventory and compliance scans) likewise sees an account with no zones.

Fix: route53's hosted-zone listing and hostedzone tag reads in the route53 service — the ListHostedZones enumeration over the account's zones (its Marker/NextMarker pagination and the Config.PrivateZone flag it reports) and the hostedzone tag store that ListTagsForResource projects after ChangeTagsForResource has written to it — so that a zone created and tagged in this account is enumerated and reports its tags. Once the lookup resolves, the ACM side (its RequestCertificate handler's DNS-validation state machine and DescribeCertificate representation) must carry the certificate from PENDING_VALIDATION to ISSUED when the published CNAME resolves in that zone.

Related open issues that do NOT cover this failure: 20261029T084614Z-ses-the-ses-sending-and-receiving-planes-are.md and 20261029T065537Z-sesv2-the-whole-ses-sending-surface-is-unreachable.md carry the same route53 root cause, but are keyed only to ses / sesv2 / s3__ses__sesv2 check ids; the acm check id listed here is on neither of them and neither mentions the ACM DNS-validation path.

Service attribution: the failing check id's leading token is 'acm' (its SERVICES list is ['acm']), so the issue is filed under 'acm'; the diverging route53 operations are named here and in the fix.

Traces: acm.dns-validated-certificate-issues-under-a-delegated-zone.

## Evidence
Expected (AWS, recorded): route53.get_paginator('list_hosted_zones').paginate() yields at least one HostedZone with Config.PrivateZone false; route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<zone id after the last '/'>)['ResourceTagSet']['Tags'] contains {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'}; both HTTP 200. The check then proceeds: acm.request_certificate(DomainName=<label>.<apex>, ValidationMethod='DNS') -> DescribeCertificate publishes DomainValidationOptions[0].ResourceRecord with Type 'CNAME' and Status 'PENDING_VALIDATION'; after the CNAME is UPSERTed and the change reaches INSYNC, Status becomes 'ISSUED', DomainValidationOptions[0].ValidationStatus 'SUCCESS', Type 'AMAZON_ISSUED', InUseBy [], NotBefore < NotAfter, and the ARN appears in ListCertificates(CertificateStatuses=['ISSUED']).

Actual (emulator): the hosted-zone pagination completes and no enumerated public zone's ResourceTagSet.Tags contains the baseline tag, so the check aborts at its first statement with
  [mismatch] acm.dns-validated-certificate-issues-under-a-delegated-zone — run line 74 -> _domain_zone line 42:
  `AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no delegated zone to validate a certificate in`
No non-2xx response and no error code is reported for either route53 read, so this is not a refusal-class divergence: the reads succeed and return content that omits the account's tagged zone (or its tags). The zone id and domain name are account values; the requirement is that a tagged public hosted zone in this account is enumerated and reports the tags written on it.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then run the behavioral suite (region us-west-2).
2. Ensure the account holds a PUBLIC hosted zone tagged {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'} (route53 CreateHostedZone + ChangeTagsForResource).
3. Page route53.list_hosted_zones() and, for each zone with Config.PrivateZone false, call route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<zone id after the last '/'>).
4. Observe that no enumerated public zone's ResourceTagSet.Tags contains the baseline tag — either the zone is not enumerated at all (or is reported as private), or its tag set comes back without the entry ChangeTagsForResource wrote. Expected: the zone is enumerated and reports its tags, both with HTTP 200.
5. With that zone resolved, call acm.request_certificate(DomainName='sim2real<hex>.<apex>', ValidationMethod='DNS'), poll DescribeCertificate for DomainValidationOptions[0].ResourceRecord (Type 'CNAME', Status 'PENDING_VALIDATION'), UPSERT that CNAME in the zone, wait for GetChange -> INSYNC, and poll DescribeCertificate until Status leaves PENDING_VALIDATION.
6. Re-replay acm.dns-validated-certificate-issues-under-a-delegated-zone to confirm the fix; it then proceeds to its own ACM assertions (ISSUED, ValidationStatus SUCCESS, Type AMAZON_ISSUED, InUseBy [], NotBefore < NotAfter, ListCertificates(ISSUED) indexing), which this lookup is currently masking.
