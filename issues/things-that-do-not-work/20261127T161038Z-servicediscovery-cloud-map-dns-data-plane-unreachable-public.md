Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicediscovery.public-dns-namespace-service-name-answered-authoritatively-with-the-registered-addresses servicediscovery.public-dns-namespace-unregistered-service-name-rejected-with-authoritative-nxdomain servicediscovery.weighted-routing-service-answers-each-dns-query-with-one-registered-address servicediscovery.cloudmap-a-query-in-the-vpc-answers-each-registered-instance-ipv4-and-follows-deregistration servicediscovery.cloudmap-srv-query-answers-each-instance-then-is-rejected-with-nxdomain-once-every-instance-is-deregistered
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery.public-dns-namespace-service-name-answered-authoritatively-with-the-registered-addresses|_ask#18|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: servicediscovery.public-dns-namespace-unregistered-service-name-rejected-with-authoritative-nxdomain|_ask#18|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: servicediscovery.weighted-routing-service-answers-each-dns-query-with-one-registered-address|_ask#21|AssertionError|assert "reply" in body, f"the query to {server} got no reply: {body.get('error')!r}"
Issue-Key: servicediscovery.cloudmap-a-query-in-the-vpc-answers-each-registered-instance-ipv4-and-follows-deregistration|_poll#11|Timeout|last = probe()
Issue-Key: servicediscovery.cloudmap-srv-query-answers-each-instance-then-is-rejected-with-nxdomain-once-every-instance-is-deregistered|_poll#11|Timeout|last = probe()
Issue-Commit: e107582060aad87cd54e5006cb4bcdcf31998cd8

# servicediscovery: Cloud Map DNS data plane unreachable — public-namespace delegation-set name servers do not resolve, and VPC-resolver queries for a private namespace never converge

> _Note: this issue was identified against the code base at commit `e107582060aad87cd54e5006cb4bcdcf31998cd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePublicDnsNamespace

## Description
Two net-new root causes in the Cloud Map (servicediscovery) DNS data plane. Both are about serving DNS for a namespace, not about the control-plane API responses, which answer normally.

Cause 1 of 2: a public DNS namespace's hosted zone advertises delegation-set name servers that cannot be reached, so no query against the namespace can be answered.
  AWS: CreatePublicDnsNamespace creates a Route 53 public hosted zone and reports it on GetNamespace (Smithy: servicediscovery DnsProperties.HostedZoneId — "The ID for the Route 53 hosted zone that Cloud Map creates when you create a namespace"; DnsProperties.SOA is the zone's SOA). Route 53 GetHostedZone on that zone returns DelegationSet.NameServers, four name server host names, and each of those names resolves and answers queries for the zone authoritatively: an A query for <service>.<namespace> returns the registered instances' AWS_INSTANCE_IPV4 values with the service's record TTL and the AA bit set, and a name with no registered instance is answered with an authoritative NXDOMAIN (rcode 3) plus the zone's SOA in the authority section. The model says nothing about the host names themselves — recorded behavior only: the recording shows the delegation-set names being resolved and queried successfully from the client vantage.
  Emulator: GetHostedZone on the namespace's zone does return four AWS-shaped name server host names (of the form ns-<n>.awsdns-<n>.com), but those names do not resolve from inside the emulated environment at all. Every query to servers[0] fails before any DNS message is exchanged, with the relay reporting `gaierror: [Errno -2] Name or service not known` for the announced server (observed as ns-730.awsdns-12.com, ns-307.awsdns-32.com, ns-365.awsdns-30.com — the digits are per-run values, the ns-*.awsdns-*.com shape is the fixed part). The emulator therefore announces an address other than the one it is actually reachable on: the names are literal copies of AWS's public delegation-set pattern rather than names that point at the emulator's own DNS endpoint. Consequence for each check: the authoritative-answer check, the authoritative-NXDOMAIN check and the weighted-routing single-address check all abort at the first query with no reply, so the answer section, AA bit, TTL, rcode and SOA behavior of a public namespace are entirely unserved.
  Why: a client that follows the documented Cloud Map flow — create a public DNS namespace, read HostedZoneId, read the zone's delegation set, then resolve <service>.<namespace> against those name servers — cannot resolve anything. Anything exercising DNS-based discovery (service meshes, registrator sidecars, DNS-based client load balancing, weighted/multivalue routing policies) sees a hard name-resolution failure instead of records, which is indistinguishable from a broken deployment.
  Fix: the public-DNS-namespace creation path and the delegation-set it hands to Route 53 — the namespace handler must publish name server names that resolve, inside the emulated environment, to the emulator's own authoritative DNS listener, and that listener must serve the namespace's zone (A/SRV answers from registered instances, AA bit, service TTL, and SOA-bearing NXDOMAIN for unregistered names). Announcing AWS's real ns-*.awsdns-*.com names is the defect.
  Traces: servicediscovery.public-dns-namespace-service-name-answered-authoritatively-with-the-registered-addresses; servicediscovery.public-dns-namespace-unregistered-service-name-rejected-with-authoritative-nxdomain; servicediscovery.weighted-routing-service-answers-each-dns-query-with-one-registered-address.

Cause 2 of 2: A and SRV queries asked of the VPC Resolver from inside a private DNS namespace's VPC never reach the expected answer, so the scenario reaches no verdict inside its budget.
  AWS: for a private DNS namespace, RegisterInstance with AWS_INSTANCE_IPV4 creates the A record (and with AWS_INSTANCE_PORT the SRV record) in the namespace's private hosted zone, and a recursive query sent to the VPC Resolver (the VPC CIDR's base address + 2) from inside that VPC answers <service>.<namespace> with exactly the currently registered instances' addresses at a TTL no greater than the service's configured TTL; DeregisterInstance removes that instance's record, and once every instance is deregistered the name is answered NXDOMAIN. Recorded behavior only for the resolver address convention; the record contents follow from the DnsConfig/DnsRecords the service was created with.
  Emulator: neither scenario produces a verdict — both end as `Check timed out after 60.0s`, i.e. the emulator never puts the scenario into the asserted state within the wall clock, while the equivalent control-plane-only Cloud Map checks in the same service pass. Hypothesis: the VPC-Resolver path for a private DNS namespace serves no records for registered instances, so the checks' retry loops (polling for the registered addresses, then for the post-deregistration set) spin until the clock expires rather than failing on a wrong answer; a slow-but-correct path is the alternative reading, and either way the scenario does not converge.
  Why: a client doing in-VPC service discovery — the primary Cloud Map use case — gets no records and no error, only hangs and retries. Deregistration-driven removal and the eventual NXDOMAIN are likewise unobservable, so failover and scale-in behavior cannot be exercised at all.
  Fix: the private-DNS-namespace record path — instance registration/deregistration state must be projected into the namespace's zone, and the emulator's in-VPC resolver endpoint must answer A and SRV queries for <service>.<namespace> from that state (service TTL honored, NXDOMAIN once the last instance is deregistered), promptly enough for a query made right after RegisterInstance to converge.
  Traces: servicediscovery.cloudmap-a-query-in-the-vpc-answers-each-registered-instance-ipv4-and-follows-deregistration; servicediscovery.cloudmap-srv-query-answers-each-instance-then-is-rejected-with-nxdomain-once-every-instance-is-deregistered.

Neither cause is covered by the open issue 20261127T111751Z-servicediscovery-discover-instances-data-plane-request-validation-and.md, which concerns the DiscoverInstances API, not DNS resolution.

## Evidence
Cause 1 — public namespace, query to the zone's own delegation-set name server:
  Expected (AWS): the A query for <service>.<namespace> sent to DelegationSet.NameServers[0] gets a reply — rcode 0, AA set, not truncated, one A record per registered instance carrying AWS_INSTANCE_IPV4 at the service's TTL; for a name with no instances, rcode 3 (NXDOMAIN) with the zone SOA in the authority section.
  Actual (emulator): no DNS message is exchanged at all. Relay result:
    AssertionError: the query to ns-730.awsdns-12.com got no reply: 'gaierror: [Errno -2] Name or service not known'
    AssertionError: the query to ns-307.awsdns-32.com got no reply: 'gaierror: [Errno -2] Name or service not known'
    AssertionError: the query to ns-365.awsdns-30.com got no reply: 'gaierror: [Errno -2] Name or service not known'
  The emulator did return four plausibly-shaped name servers from GetHostedZone (the delegation-set length assertion passed); they are simply not names that resolve to the emulator. The ns-*/awsdns-* digits and the namespace label are per-run values; the defect is the announced-name pattern, not any one name.

Cause 2 — private namespace, query to the VPC Resolver at <vpc-cidr-base>+2:
  Expected (AWS): the first poll iteration or a later one answers <service>.<namespace> with exactly the registered addresses (rcode 0, every answer an A record owned by the FQDN, ttl <= the service's configured TTL); after DeregisterInstance of the first instance the same query answers only with the remaining address, and ListInstances reports only the remaining instance id. For the SRV variant, each instance is answered as an SRV record and the name becomes NXDOMAIN after the last deregistration.
  Actual (emulator): Check timed out after 60.0s — for both the A and the SRV scenario. No answer assertion was ever reached, so the emulator produced no record set to compare.

## Steps to Reproduce
Cause 1 (public DNS namespace, 3 checks):
1. CreatePublicDnsNamespace(Name="<label>.example.test", CreatorRequestId=...) and poll GetOperation until SUCCESS; take NAMESPACE from Operation.Targets.
2. GetNamespace(Id=<ns>) -> Namespace.Properties.DnsProperties.HostedZoneId.
3. route53 GetHostedZone(Id=<zone>) -> DelegationSet.NameServers (four names are returned).
4. CreateService(Name="web", NamespaceId=<ns>, DnsConfig={DnsRecords:[{Type:"A",TTL:<ttl>}]}) and RegisterInstance twice with distinct AWS_INSTANCE_IPV4 values.
5. From a client vantage inside the environment, send a UDP A query for "web.<label>.example.test" to NameServers[0]. Observe that the name server host name does not resolve (gaierror / NXDOMAIN on the server name itself) so no DNS exchange occurs. Expected: an authoritative answer carrying both registered addresses. Repeat with a service name that has no registered instance and expect an authoritative NXDOMAIN with the zone SOA.

Cause 2 (private DNS namespace, 2 checks):
1. Create a VPC/subnets/security group, CreatePrivateDnsNamespace in that VPC, and CreateService with DnsConfig DnsRecords Type A (TTL 10) — and separately a service with Type SRV.
2. RegisterInstance twice: AWS_INSTANCE_IPV4 10.77.0.11 and 10.77.0.12 (for SRV, with AWS_INSTANCE_PORT).
3. From inside the VPC, send a recursive A (resp. SRV) query for "<service>.<namespace>" to the VPC Resolver at <vpc-cidr-network-address>+2 and poll. Expected: rcode 0 with exactly the registered addresses at ttl <= 10.
4. DeregisterInstance(the first instance) and re-query. Expected: only the remaining address; after deregistering all instances, NXDOMAIN. Observed: the scenario never reaches either state and the check is killed at its 60s wall clock.
