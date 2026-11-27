Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__route53.acm-caa-records-authorizing-the-amazon-ca-domains-are-answered-for-the-certificate-domain acm__route53.acm-dns-validation-cname-in-the-delegated-zone-is-answered-with-the-published-target acm__route53.acm-wildcard-and-apex-validation-name-is-answered-and-an-unpublished-sibling-is-rejected-with-nxdomain
Issue-Key: acm__route53.acm-caa-records-authorizing-the-amazon-ca-domains-are-answered-for-the-certificate-domain|_ask#11|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: acm__route53.acm-dns-validation-cname-in-the-delegated-zone-is-answered-with-the-published-target|_ask#13|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: acm__route53.acm-wildcard-and-apex-validation-name-is-answered-and-an-unpublished-sibling-is-rejected-with-nxdomain|run#8|AssertionError|assert (_norm(records[wildcard]["Name"]), _norm(records[wildcard]["Value"])) == shared, (
Issue-Commit: 0e14ec34ba796ebb423716588b97f90ad61f6019

# acm__route53: 2 net-new causes — a hosted zone's DelegationSet.NameServers are AWS-style hostnames that resolve nowhere, and a wildcard SAN gets its own DNS validation record instead of sharing the apex's

> _Note: this issue was identified against the code base at commit `0e14ec34ba796ebb423716588b97f90ad61f6019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedZone

## Description
Two distinct defects behind ACM's DNS-validation data plane. Both must be closed before this issue is submitted.

Cause 1 of 2: the name servers a hosted zone publishes are not addresses the emulator can be reached on, so they fail at name resolution.
AWS: route53 CreateHostedZone/GetHostedZone return DelegationSet.NameServers, the authoritative servers for the zone. Those names resolve, and a DNS query for a record in the zone sent to any of them is answered authoritatively; a name outside the zone comes back NXDOMAIN. (Smithy: route53 CreateHostedZone/GetHostedZone output member DelegationSet -> DelegationSet.NameServers; the DNS wire behavior itself is not modeled, so: recorded behavior only.)
Emulator: the delegation set is populated with literal AWS production hostnames in the awsdns family (e.g. `ns-108.awsdns-27.com`, `ns-1071.awsdns-06.com`). Those names do not resolve where the emulator is reached, so a query addressed to a published name server never even reaches a socket: it fails with `gaierror: [Errno -2] Name or service not known`. The emulator is announcing an address other than the one it serves DNS on. Every record the control plane accepted — the ACM DNS-validation CNAME, the CAA records authorizing the Amazon CAs for a certificate domain — is therefore unreadable, although ChangeResourceRecordSets reported success.
Why: a client that follows the delegation set it was handed, exactly as AWS instructs for delegating a zone, cannot resolve anything the emulator claims to be authoritative for. ACM DNS validation, CAA pre-checks, and every alias/CNAME layered on a hosted zone are unreachable, and the failure mode is a name-resolution error rather than a DNS response, so a caller cannot even distinguish NXDOMAIN from an unserved zone.
Related open issues that describe this surface but do not carry these checks' replay keys: 20261118T090000Z-route53-the-authoritative-dns-plane-is-unreachable-no-dns-wire-is-served.md, 20261122T001500Z-route53-a-hosted-zone-s-delegated-nameservers-resolve-nowhere.md, 20261117T134237Z-acm-cloudfront-iam-lambda-route53-s3-a-route-53-public-hosted-zone-s-delegated.md.
Fix: route53 CreateHostedZone's delegation-set allocation together with the DNS responder that serves the zones held in route53 state — the hostnames published in DelegationSet.NameServers must resolve to, and be answered by, the responder that actually serves those zones.
Traces: acm__route53.acm-caa-records-authorizing-the-amazon-ca-domains-are-answered-for-the-certificate-domain, acm__route53.acm-dns-validation-cname-in-the-delegated-zone-is-answered-with-the-published-target

Cause 2 of 2: a wildcard SAN is given its own validation record instead of sharing the record of the name it wildcards.
AWS: for an ACM certificate requested with ValidationMethod DNS whose DomainName is `d` and whose SubjectAlternativeNames include `*.d`, DNS validation collapses the two onto ONE record: the DomainValidationOptions entries for `d` and for `*.d` carry an IDENTICAL ResourceRecord Name and Value, so publishing that single CNAME validates both. A genuinely different SAN (e.g. `www.d`) gets its own record sharing neither name nor value. AWS documents this (https://docs.aws.amazon.com/acm/latest/userguide/dns-validation.html); the Smithy model does not state it, so the rule is the documented one.
Emulator: the `*.d` entry is minted with its own independent Name and Value, unrelated to the entry for `d` — two separate records where AWS publishes one shared record.
Why: every client that publishes the deduplicated set of validation records — the console/CLI flow, CDK, and Terraform's aws_acm_certificate/aws_route53_record pattern which keys records by their unique name — publishes the apex's record and considers the certificate fully pending; the wildcard's extra record is never published and the certificate never leaves PENDING_VALIDATION. The certificate also demands one more DNS record than AWS does.
Related: open issue 20261022T143245Z-acm-certificate-representation-dns-validation-and-error-message.md carries this behavior for the `acm` check `acm.dns-validation-cname-collapses-wildcard-onto-its-apex-and-is-unique-per-san`, but not this check's replay key.
Fix: ACM's DNS-validation record derivation in the RequestCertificate handler / certificate state — derive the validation record from the wildcarded apex so a `*.X` SAN reuses the record generated for `X`, while unrelated SANs keep distinct records.
Traces: acm__route53.acm-wildcard-and-apex-validation-name-is-answered-and-an-unpublished-sibling-is-rejected-with-nxdomain

## Evidence
Cause 1 — after route53 CreateHostedZone + ChangeResourceRecordSets, a DNS query is sent to DelegationSet.NameServers[0]:
  Expected: an authoritative reply carrying the record (NXDOMAIN for a name outside the zone).
  Actual:   AssertionError: the query to ns-108.awsdns-27.com got no reply: 'gaierror: [Errno -2] Name or service not known'
            AssertionError: the query to ns-1071.awsdns-06.com got no reply: 'gaierror: [Errno -2] Name or service not known'
  The published names are production awsdns hostnames; the specific labels differ per zone, the requirement is that whatever names are published resolve to the responder serving the zone.

Cause 2 — DescribeCertificate on a certificate with DomainName=<d>, SubjectAlternativeNames=['*.<d>', 'www.<d>']:
  Expected: the DomainValidationOptions entries for <d> and for *.<d> have the same ResourceRecord Name and the same Value (one shared CNAME), and the entry for www.<d> shares neither.
  Actual:   AssertionError: the wildcard's record {'Name': '_<hex32>.<d>.', 'Type': 'CNAME', 'Value': '_<hex32>.<token>.acm-validations.<domain>.'} differs from the domain's {'Name': '_<other-hex32>.<d>.', 'Type': 'CNAME', 'Value': '_<other-hex32>.<other-token>.acm-validations.<domain>.'}
  (the hex labels and tokens are per-run values; the requirement is that the wildcard's pair equals the apex's pair.)

## Steps to Reproduce
Cause 1:
1. route53 CreateHostedZone for a public zone and read DelegationSet.NameServers.
2. acm RequestCertificate with ValidationMethod=DNS for a name in that zone; ChangeResourceRecordSets to publish the validation CNAME (and the CAA records authorizing the Amazon CAs).
3. Send a DNS query for that record to the first published name server.
4. Observe the query fails to resolve the name server itself: 'gaierror: [Errno -2] Name or service not known'. AWS answers authoritatively.

Cause 2:
1. acm RequestCertificate(DomainName=<d>, SubjectAlternativeNames=['*.<d>', 'www.<d>'], ValidationMethod='DNS').
2. DescribeCertificate and read DomainValidationOptions[*].ResourceRecord.
3. Observe the entries for <d> and '*.<d>' carry different Name and Value; AWS gives them the identical record.
