Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__route53.acm-caa-records-authorizing-the-amazon-ca-domains-are-answered-for-the-certificate-domain acm__route53.acm-dns-validation-cname-in-the-delegated-zone-is-answered-with-the-published-target acm__route53.acm-wildcard-and-apex-validation-name-is-answered-and-an-unpublished-sibling-is-rejected-with-nxdomain
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: acm__route53.acm-caa-records-authorizing-the-amazon-ca-domains-are-answered-for-the-certificate-domain|_ask#11|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: acm__route53.acm-dns-validation-cname-in-the-delegated-zone-is-answered-with-the-published-target|_ask#13|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: acm__route53.acm-wildcard-and-apex-validation-name-is-answered-and-an-unpublished-sibling-is-rejected-with-nxdomain|_ask#13|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Commit: 0e14ec34ba796ebb423716588b97f90ad61f6019

# acm__route53: the hosted zone's published DelegationSet.NameServers resolve nowhere, so no ACM DNS-validation or CAA query ever reaches a responder of this emulator

> _Split out of `issues/things-that-now-work/20261127T110935Z-acm-route53-2-net-new-causes-a-hosted.md`, whose cause 2 (a wildcard SAN sharing the validation record of the name it wildcards) is fixed. This report carries the three replay keys that do NOT pass, re-filed here so they are not recorded as working. The third key is the same unit as that fixed cause: it now gets PAST the record-sharing assertion (`run#8`) and fails at the first DNS query it sends (`_ask#13`), on this cause alone._

## Operation
CreateHostedZone

## Description
The name servers a hosted zone delegates to are AWS hostnames that resolve to nothing in the
environment the checks query from, so no query can reach an authoritative server of this
emulator — and there is nothing for one to reach: no socket in this repository speaks the DNS
wire.

AWS: `CreateHostedZone`/`GetHostedZone` report `DelegationSet.NameServers` (a list of
`DNSName`). The wire behaviour is not modelled, so this is recorded behaviour only: the names
Route 53 reports are the zone's authoritative servers, they RESOLVE to addresses, and those
addresses answer queries for names in the zone authoritatively (AA set) and `NXDOMAIN` for a
name the zone does not hold. Each check here first reads the delegation set, then sends its
query to the FIRST advertised name server — exactly what a client following the delegation
does — for the ACM DNS-validation CNAME it published, for the CAA records authorizing the
Amazon CAs, and for the shared wildcard/apex validation name beside an unpublished sibling it
expects `NXDOMAIN` for.

Emulator: the advertised hostnames do not resolve at all from that vantage point, so the query
is never sent. The control-plane calls before it (`CreateHostedZone`,
`ChangeResourceRecordSets`, `RequestCertificate`, `DescribeCertificate`) all succeed, so the
zone, its records and the certificate's challenge all exist; what is missing is that the
emulator announces a delegation set it is not reachable on. The generated labels are per-run
values; the requirement is that whatever `DelegationSet.NameServers` publishes must resolve
AND must be answered on, authoritatively, by a DNS responder of this emulator.

Why it is BLOCKED, and on what: a service crate may not open a socket of its own ("The service
never binds a socket, chooses a port, or configures TLS itself", `vendor/engine/src/lib.rs`
module docs), and the core listener API it must use offers no datagram socket and no way to be
served on a protocol's well-known port: `vendor/server/src/listeners.rs` exposes only
`Listeners::tcp`, `tcp_with_idle`, `tcp_plaintext` and `http` — all stream sockets — each bound
on an EPHEMERAL port, or on one from the operator's port range (`bind_std`/`bind_tokio`). A
delegated name server is reached by DATAGRAM on port 53, and an `NS` name carries no port at
all, so neither half of the requirement is reachable from a service crate today.

The required core change, stated so a maintainer can make it: a datagram entry point on
`emulator_server::Listeners` (a `udp(handler)` beside `tcp`, handing each received datagram to
a handler and answering from the same socket), plus a way for such a listener to take the
protocol's well-known port and a name the control plane can publish that resolves to it in the
environment the suite queries from. With that in place the responder itself is largely already
modelled in-process and needs wiring to the published delegation set:
`services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/test_dns_answer.rs` already
answers one simulated query from a zone's persisted record sets (routing policies, alias
chasing, `NOERROR`/`NXDOMAIN`), `services/common/src/public_dns_standard.rs` holds the
reserved-name readings every resolver here shares, and
`services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/vpc_network/`
shows the `simple_dns` packet parse/serialise already used in this tree.

What is ruled out, so it is not retried: publishing a name that RESOLVES (an address literal,
or a loopback name) while nothing answers on port 53 would turn these failures into timeouts
and announce an endpoint nothing protocol-capable serves, so the delegation set is
deliberately left exactly as it is until the responder exists.

Related, same missing capability, different replay keys:
`20261118T090000Z-route53-the-authoritative-dns-plane-is-unreachable-no-dns-wire-is-served.md`
(13 route53 keys), `20261122T001500Z-route53-a-hosted-zone-s-delegated-nameservers-resolve-nowhere.md`,
`20261117T134237Z-acm-cloudfront-iam-lambda-route53-s3-a-route-53-public-hosted-zone-s-delegated.md`.

## Evidence
Replay of the parent bundle's three units after its cause-2 fix: all three fail while resolving
the FIRST host listed in the zone's `DelegationSet.NameServers`, and `other_causes` was empty,
so the cause-2 fix caused none of them:

    AssertionError: the query to ns-690.awsdns-36.com got no reply:
    'gaierror: [Errno -2] Name or service not known'
    AssertionError: the query to ns-1283.awsdns-40.com got no reply:
    'gaierror: [Errno -2] Name or service not known'
    AssertionError: the query to ns-291.awsdns-00.com got no reply:
    'gaierror: [Errno -2] Name or service not known'

(the host labels are per-run values). Expected: every name published in
`DelegationSet.NameServers` resolves and answers authoritatively for names in the zone, and
`NXDOMAIN` for a name it does not hold.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. `CreateHostedZone` for a public zone, then `RequestCertificate` with `ValidationMethod=DNS`
   for a name in that zone and `ChangeResourceRecordSets` to publish the validation CNAME (and
   the CAA records authorizing the Amazon CAs).
3. Read `DelegationSet.NameServers` off the create (or `GetHostedZone`), and from a client in
   the environment send a DNS query for the published record to the first advertised host.
4. Observe `gaierror` (the name does not resolve) rather than a reply, so the authoritative
   answer the checks grade — the validation CNAME, the CAA set, `NXDOMAIN` for an unpublished
   sibling — is never observed.
5. Replay any of the three check ids in Issue-Trace-Ids.
