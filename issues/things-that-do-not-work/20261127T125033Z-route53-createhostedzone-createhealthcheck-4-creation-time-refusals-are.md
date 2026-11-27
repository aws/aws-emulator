Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.private-zone-same-name-same-vpc-rejected route53.private-zone-with-unknown-vpc-rejected route53.same-name-zone-on-the-same-delegation-set-rejected route53.health-check-on-private-or-loopback-address-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.private-zone-same-name-same-vpc-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: route53.private-zone-with-unknown-vpc-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: route53.same-name-zone-on-the-same-delegation-set-rejected|expect_error#10|AssertionError|raise AssertionError(f"expected ({code!r}, {message!r}) but the call succeeded: {result!r}")
Issue-Key: route53.health-check-on-private-or-loopback-address-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# route53 CreateHostedZone/CreateHealthCheck: 4 creation-time refusals are missing — duplicate private zones, unknown VPCs, delegation-set name conflicts and private/loopback health-check targets are all accepted

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedZone

## Description
Creation-time validation on the two Route 53 create operations is absent: each call AWS refuses with a modeled 400 returns a created resource here. Smithy (services/service-com-amazonaws-route53-awsdnsv20130401): CreateHostedZone declares ConflictingDomainExists (400), InvalidVPCId (400) and InvalidInput (400) among its errors and answers HTTP 201 on success; CreateHealthCheck declares InvalidInput (400) and answers HTTP 201. In the messages below the sentence text is AWS's fixed text for the class; the VPC id, Region, zone id, domain name, delegation-set id and IP address are values taken from the request and the emulator's own state.

Cause 1 of 4: a second private hosted zone with the same name on a VPC already associated with such a zone is created.
AWS: ConflictingDomainExists, HTTP 400, "The VPC <vpc id> in region <region> has already been associated with the hosted zone <hosted zone id> with the same domain name." (fixed text; vpc id and region from the request, hosted zone id of the existing conflicting zone from state). The same name on a *different* VPC is accepted.
Emulator: HTTP 201 with a new hosted zone, and the VPC then carries two zones of that name (ListHostedZonesByVPC).
Why: a client relying on the one-zone-per-name-per-VPC rule creates an ambiguous private namespace and never sees the conflict error it handles.
Fix: the CreateHostedZone handler's private-zone conflict check against VPC associations in state (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/create_hosted_zone.rs).
Traces: route53.private-zone-same-name-same-vpc-rejected.

Cause 2 of 4: a private hosted zone is created for a VPC id that does not exist, and for a real VPC named under the wrong Region.
AWS: InvalidVPCId, HTTP 400, "The VPC: <vpc id> in region <vpc region> that you provided is not authorized to make the association." (fixed text; both values echo the request).
Emulator: HTTP 201 with a zone whose VPC member echoes the bogus id (e.g. a never-created vpc-id), and the zone is then listable by name.
Why: a client's VPC-id typo or cross-region mistake silently produces a private zone associated with nothing, so DNS never resolves and the cause is invisible.
Fix: the same handler must resolve the VPC id and Region against EC2 state before associating.
Traces: route53.private-zone-with-unknown-vpc-rejected.

Cause 3 of 4: a second public hosted zone with the same domain name on the same reusable delegation set is created.
AWS: ConflictingDomainExists, HTTP 400, "Cannot create hosted zone with DelegationSetId DelegationSetId:<delegation set id> as the DNSName <domain name> conflicts with existing ones sharing the delegation set" (fixed text, including the literal 'DelegationSetId:' prefix; the delegation-set id and DNS name come from the request).
Emulator: HTTP 201 with a second zone of the same name on that delegation set.
Whip/Why: two zones of one name share one delegation set, so the name servers would be authoritative for two conflicting versions of the zone and the client's conflict handling is never exercised.
Fix: the same handler's delegation-set name-conflict check.
Traces: route53.same-name-zone-on-the-same-delegation-set-rejected.

Cause 4 of 4: CreateHealthCheck accepts endpoints Route 53 cannot reach — RFC 1918, loopback and link-local addresses.
AWS: refuses with InvalidInput, HTTP 400. For a private (RFC 1918) address: "Invalid parameter : PrivateIP supplied for a non private endpoint health check: <ip>". For loopback and link-local: "IPv4 address <ip> is forbidden". Both are fixed texts with the rejected IPAddress from the request; a health check on a public FQDN is accepted.
Emulator: HTTP 201 with a health-check id for 10.0.0.1, 192.168.1.10, 127.0.0.1 and 169.254.169.254, and ListHealthChecks then shows the new checks.
Why: a client validating that only publicly reachable endpoints can be health-checked gets an accepted check that can never report status, and its negative path is untested.
Fix: the IPAddress validation of the CreateHealthCheck handler (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/create_health_check.rs).
Traces: route53.health-check-on-private-or-loopback-address-rejected.

## Evidence
From the replay output:
- route53.private-zone-same-name-same-vpc-rejected: expected ('ConflictingDomainExists', 'The VPC <vpc id> in region <region> has already been associated with the hosted zone <zone id> with the same domain name.'); AssertionError: expected the call to be rejected, but it succeeded: {... 'HTTPStatusCode': 201, 'HostedZone': {'Id': '/hostedzone/...', 'Config': {'PrivateZone': True}}, 'VPC': {'VPCRegion': 'us-west-2', 'VPCId': 'vpc-...'}}
- route53.private-zone-with-unknown-vpc-rejected: expected ('InvalidVPCId', 'The VPC: <vpc id> in region <region> that you provided is not authorized to make the association.'); got HTTP 201 with 'VPC': {'VPCRegion': 'us-west-2', 'VPCId': 'vpc-0123456789abcdef0'} — an id no CreateVpc ever returned.
- route53.same-name-zone-on-the-same-delegation-set-rejected: AssertionError: expected ('ConflictingDomainExists', 'Cannot create hosted zone with DelegationSetId DelegationSetId:<id> as the DNSName <name> conflicts with existing ones sharing the delegation set') but the call succeeded: {... 'HTTPStatusCode': 201, 'HostedZone': {...}, 'DelegationSet': {'Id': '/delegationset/<id>', ...}}
- route53.health-check-on-private-or-loopback-address-rejected: AssertionError: expected the call to be rejected, but it succeeded: '<health check id>' — expected ('InvalidInput', 'Invalid parameter : PrivateIP supplied for a non private endpoint health check: 10.0.0.1') for RFC 1918 and ('InvalidInput', 'IPv4 address 127.0.0.1 is forbidden') for loopback/link-local.

## Steps to Reproduce
1. Cause 1: CreateVpc twice; CreateHostedZone <name> with VPC=vpc1, then the same <name> with VPC=vpc2 (accepted, correct), then <name> again with VPC=vpc1 — observe HTTP 201 instead of 400 ConflictingDomainExists, and ListHostedZonesByVPC(vpc1) returning two zones.
2. Cause 2: CreateHostedZone with VPC={VPCRegion: <current region>, VPCId: 'vpc-0123456789abcdef0'} and with a real VPC id under another Region — observe HTTP 201 instead of 400 InvalidVPCId.
3. Cause 3: CreateReusableDelegationSet, CreateHostedZone <name> with that DelegationSetId, then CreateHostedZone <name> again with the same DelegationSetId — observe HTTP 201 instead of 400 ConflictingDomainExists.
4. Cause 4: CreateHealthCheck with HealthCheckConfig {Type: HTTP, Port: 80, ResourcePath: '/', IPAddress: '10.0.0.1'} and again for '192.168.1.10', '127.0.0.1', '169.254.169.254' — observe HTTP 201 and a new id in ListHealthChecks instead of 400 InvalidInput.
