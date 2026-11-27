Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.route53-aaaa-query-serves-each-ipv6-value-as-written route53.route53-absent-name-query-rejected-with-nxdomain-carrying-the-apex-soa route53.route53-apex-ns-query-is-answered-with-the-delegation-set-name-servers route53.route53-caa-query-for-a-name-without-a-caa-record-rejected-with-nodata-beside-a-served-caa-set route53.route53-dnskey-query-rejected-with-nodata-before-a-key-signing-key-then-answered-with-signed-keys route53.route53-failover-query-is-answered-with-the-secondary-while-the-primarys-health-check-is-unhealthy route53.route53-geolocation-by-edns-client-subnet-unmatched-without-default-rejected-with-nodata route53.route53-geoproximity-records-answer-with-the-region-nearest-the-querier route53.route53-mx-query-serves-each-priority-and-mail-server-as-written route53.route53-query-for-a-type-absent-at-an-existing-name-rejected-with-nodata-and-the-zone-soa route53.route53-record-ttl-chosen-is-served-by-the-name-server-and-alias-answers-carry-the-targets route53.route53-srv-query-for-unpublished-service-label-rejected-with-nxdomain route53.route53-txt-record-with-several-strings-is-answered-with-every-string-in-order route53.route53-wildcard-expansion-rejected-for-other-types-and-the-apex
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.route53-aaaa-query-serves-each-ipv6-value-as-written|_ask#8|AssertionError|assert "reply" in body, f"relay could not complete the {qtype} query for {name}: {body!r}"
Issue-Key: route53.route53-absent-name-query-rejected-with-nxdomain-carrying-the-apex-soa|_ask#17|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: route53.route53-apex-ns-query-is-answered-with-the-delegation-set-name-servers|_ask#16|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: route53.route53-caa-query-for-a-name-without-a-caa-record-rejected-with-nodata-beside-a-served-caa-set|_ask#11|AssertionError|assert "reply" in out, f"the relay could not complete the query: {out!r}"
Issue-Key: route53.route53-dnskey-query-rejected-with-nodata-before-a-key-signing-key-then-answered-with-signed-keys|_query#11|AssertionError|assert "answer" in body, f"{server} sent no DNS reply: {body.get('failure')!r}"
Issue-Key: route53.route53-failover-query-is-answered-with-the-secondary-while-the-primarys-health-check-is-unhealthy|_ask#14|AssertionError|assert "reply" in out, f"the relay could not complete the query: {out!r}"
Issue-Key: route53.route53-geolocation-by-edns-client-subnet-unmatched-without-default-rejected-with-nodata|send#15|AssertionError|raise AssertionError(f"the query to the zone's name server got no reply: {body!r}")
Issue-Key: route53.route53-geoproximity-records-answer-with-the-region-nearest-the-querier|_ask#22|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: route53.route53-mx-query-serves-each-priority-and-mail-server-as-written|_ask#8|AssertionError|assert "reply" in body, f"relay could not complete the {qtype} query for {name}: {body!r}"
Issue-Key: route53.route53-query-for-a-type-absent-at-an-existing-name-rejected-with-nodata-and-the-zone-soa|_ask#11|AssertionError|assert "reply" in res, f"the relay could not complete the {rtype} query: {res!r}"
Issue-Key: route53.route53-record-ttl-chosen-is-served-by-the-name-server-and-alias-answers-carry-the-targets|_ask#7|AssertionError|assert body.get("ok"), f"query {rtype} {name} to {server} not completed: {body!r}"
Issue-Key: route53.route53-srv-query-for-unpublished-service-label-rejected-with-nxdomain|_ask#8|AssertionError|assert "reply" in body, f"relay could not complete the {qtype} query for {name}: {body!r}"
Issue-Key: route53.route53-txt-record-with-several-strings-is-answered-with-every-string-in-order|_ask#11|AssertionError|assert "reply" in res, f"the relay could not complete the {rtype} query: {res!r}"
Issue-Key: route53.route53-wildcard-expansion-rejected-for-other-types-and-the-apex|_ask#7|AssertionError|assert body.get("ok"), f"query {rtype} {name} to {server} not completed: {body!r}"
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# route53: 14 more DNS data-plane checks fail because the hosts in DelegationSet.NameServers resolve nowhere — AAAA/MX/TXT/SRV/CAA/DNSKEY/NS, NXDOMAIN/NODATA, TTL, failover, geolocation and geoproximity answers are all unobservable

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
Cause 1 of 1: CreateHostedZone publishes DelegationSet.NameServers as AWS-style hostnames (ns-<n>.awsdns-<n>.com/.net/.org/.co.uk) that resolve to no address in the environment the emulator serves, and no authoritative DNS listener is reachable for the zone, so every query a client would send to the zone's own name servers fails at address resolution rather than being answered.
AWS: CreateHostedZone returns a delegation set of four name servers that are reachable and authoritative for the zone; a query to any of them for a record the zone holds is answered from that zone — an AAAA set answers every IPv6 value as written, an MX set every priority/host pair, a TXT set every character-string in order, an SRV set its targets, a CAA set its values, the apex NS query the delegation set itself, DNSKEY NODATA before a KSK and the signed keys after one, a missing name NXDOMAIN with the apex SOA in the authority section, a missing type at an existing name NODATA with the zone SOA, the chosen TTL on the answer, the secondary record while the primary's health check is unhealthy, NODATA for an unmatched EDNS client subnet with no default geolocation record, and the geoproximity record nearest the querier.
Emulator: resolving any published name server fails — 'gaierror: [Errno -2] Name or service not known' — both when the check queries the name server directly and when it relays the query from a function inside the emulated environment. The emulator announces addresses (these hostnames) other than the one it is reachable on, so nothing about the 14 scenarios below can be served or graded.
Why: every DNS-serving behaviour of the service is unusable: a client that provisions records and then resolves them (mail routing, SPF/DKIM TXT, service discovery, CAA pinning, DNSSEC, health-check failover, geolocation/geoproximity routing, TTL and negative-answer semantics) gets a name-resolution failure instead of an answer, and cannot distinguish a correct zone from a broken one.
Fix: the DNS data plane — the authoritative name-server listener and the delegation-set hostnames the CreateHostedZone/GetHostedZone handlers publish must name an address the emulator actually answers on (services/service-com-amazonaws-route53-awsdnsv20130401: the delegation-set assignment in state plus the DNS listener).
Traces: the 14 checks in trace_ids. These are the checks the replay did NOT mark as owned; the same root cause is already open as 20261118T090000Z-route53-the-authoritative-dns-plane-is-unreachable-no-dns-wire-is-served.md and 20261122T001500Z-route53-a-hosted-zone-s-delegated-nameservers-resolve-nowhere.md, neither of which carries these replay keys, so they are filed here to be re-replayed with any fix.

## Evidence
Every one of the 14 checks fails on name resolution of a published delegation-set host, never on the content of an answer. Representative lines from the replay:
- route53.route53-aaaa-query-serves-each-ipv6-value-as-written: AssertionError: relay could not complete the AAAA query for v6.<zone>.: {'error': 'gaierror: [Errno -2] Name or service not known'}
- route53.route53-apex-ns-query-is-answered-with-the-delegation-set-name-servers: AssertionError: the query to ns-1012.awsdns-52.com got no reply: 'gaierror: [Errno -2] Name or service not known'
- route53.route53-dnskey-query-rejected-with-nodata-before-a-key-signing-key-then-answered-with-signed-keys: AssertionError: ns-234.awsdns-56.com sent no DNS reply: 'gaierror: [Errno -2] Name or service not known'
- route53.route53-record-ttl-chosen-is-served-by-the-name-server-and-alias-answers-carry-the-targets: AssertionError: query A plain.<zone>. to ns-1137.awsdns-08.com not completed: {'ok': False, 'error': 'gaierror: [Errno -2] Name or service not known'}
- route53.route53-query-for-a-type-absent-at-an-existing-name-rejected-with-nodata-and-the-zone-soa, route53.route53-caa-query-..., route53.route53-txt-record-..., route53.route53-failover-query-...: AssertionError: the relay could not complete the query: {'error': 'gaierror: [Errno -2] Name or service not known'}
- route53.route53-geolocation-by-edns-client-subnet-unmatched-without-default-rejected-with-nodata: AssertionError: the query to the zone's name server got no reply: {'failure': 'gaierror: [Errno -2] Name or service not known'}
The control-plane calls in these checks succeed (zones and record sets are created and listed), so only the data plane diverges.

## Steps to Reproduce
1. CreateHostedZone for a public zone and read DelegationSet.NameServers from the response (four ns-<n>.awsdns-<n>.<tld> hostnames).
2. ChangeResourceRecordSets to publish the records under test (AAAA, MX, TXT with several strings, SRV, CAA, an apex NS query, a failover pair with health checks, geolocation/geoproximity siblings, a record with an explicit TTL).
3. Resolve any of the published name-server hostnames to an address, or send it a DNS query over UDP/TCP, from the runner or from a function inside the emulated environment.
4. Resolution fails with gaierror [Errno -2]; no query reaches an authoritative server, so none of the published records can be looked up.
