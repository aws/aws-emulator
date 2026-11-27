Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53__sesv2.sesv2-easy-dkim-cnames-in-a-delegated-zone-are-served-and-verify-the-identity route53__sesv2.sesv2-custom-mail-from-mx-and-spf-in-a-delegated-zone-are-served-and-verify route53__sesv2.sesv2-unpublished-dkim-token-names-in-a-delegated-zone-are-nxdomain
Issue-Key: route53__sesv2.sesv2-easy-dkim-cnames-in-a-delegated-zone-are-served-and-verify-the-identity|_ask#14|AssertionError|assert body["ok"], f"the relay could not exchange the query for {name!r}: {body['error']!r}"
Issue-Key: route53__sesv2.sesv2-custom-mail-from-mx-and-spf-in-a-delegated-zone-are-served-and-verify|_ask#16|AssertionError|assert body["ok"], f"the relay could not exchange the query for {name!r}: {body['error']!r}"
Issue-Key: route53__sesv2.sesv2-unpublished-dkim-token-names-in-a-delegated-zone-are-nxdomain|_ask#15|AssertionError|assert body["ok"], f"the relay could not exchange the query for {name!r}: {body['error']!r}"
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# route53__sesv2: SESv2 DNS verification records published in a delegated Route 53 zone can never be resolved — the zone's name servers resolve nowhere

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
Cause 1 of 1: the three SESv2-over-Route 53 scenarios all fail because the hosts the hosted zone publishes in DelegationSet.NameServers resolve to no address, so no query for the records SESv2 asks the client to publish ever reaches an authoritative server.
AWS: a delegated hosted zone's four name servers are reachable and authoritative, so the records SESv2 requires are resolvable from the zone and the identity verifies: the Easy DKIM CNAMEs (<token>._domainkey.<identity>) answer with their <token>.dkim.amazonses.com targets, the custom MAIL FROM domain's MX (feedback-smtp.<region>.amazonses.com with priority 10) and its SPF TXT answer as written, and a DKIM token name that was never published answers NXDOMAIN.
Emulator: every query — issued through a relay running inside the emulated environment — fails before it is sent: 'gaierror: [Errno -2] Name or service not known' when resolving the zone's published name-server hostnames. The emulator announces AWS-style ns-<n>.awsdns-<n>.<tld> hosts rather than an address it answers DNS on, so neither a positive answer nor the NXDOMAIN case can be observed and no identity can verify.
Why: a client automating email-identity setup (publish DKIM CNAMEs / MAIL FROM MX+SPF in Route 53, then wait for SESv2 verification) can never progress: the records appear in the control plane but resolve nowhere, so verification never completes and the failure gives no indication of which side is at fault.
Fix: the Route 53 DNS data plane for these zones — the authoritative listener plus the delegation-set hostnames CreateHostedZone/GetHostedZone publish must name an address the emulator serves DNS on (services/service-com-amazonaws-route53-awsdnsv20130401). The same underlying gap is open for the route53 service as 20261118T090000Z-route53-the-authoritative-dns-plane-is-unreachable-no-dns-wire-is-served.md and 20261122T001500Z-route53-a-hosted-zone-s-delegated-nameservers-resolve-nowhere.md; neither carries these route53__sesv2 replay keys, so they are recorded here.
Traces: the three checks in trace_ids.

## Evidence
From the replay output:
- route53__sesv2.sesv2-easy-dkim-cnames-in-a-delegated-zone-are-served-and-verify-the-identity: AssertionError: the relay could not exchange the query for '<token>._domainkey.<identity>.<baseline zone>.': 'gaierror: [Errno -2] Name or service not known'
- route53__sesv2.sesv2-custom-mail-from-mx-and-spf-in-a-delegated-zone-are-served-and-verify: AssertionError: the relay could not exchange the query for 'mail.<identity>.<baseline zone>.': 'gaierror: [Errno -2] Name or service not known'
- route53__sesv2.sesv2-unpublished-dkim-token-names-in-a-delegated-zone-are-nxdomain: AssertionError: the relay could not exchange the query for '<unpublished token>._domainkey.<identity>.<baseline zone>.': 'gaierror: [Errno -2] Name or service not known'
In all three the SESv2 and Route 53 control-plane calls succeed (identity created, records written); only the resolution step fails, and it fails at address resolution of the zone's own published name servers.

## Steps to Reproduce
1. CreateHostedZone for a delegated zone and note DelegationSet.NameServers.
2. SESv2 CreateEmailIdentity for a name in that zone (Easy DKIM), and PutEmailIdentityMailFromAttributes for a custom MAIL FROM subdomain.
3. ChangeResourceRecordSets to publish the DKIM CNAMEs and the MAIL FROM MX (10 feedback-smtp.<region>.amazonses.com) and SPF TXT.
4. From inside the emulated environment, resolve any of the zone's published name-server hostnames or send it a DNS query for one of those names: resolution fails with gaierror [Errno -2], and an unpublished <token>._domainkey name cannot be shown to be NXDOMAIN either.
