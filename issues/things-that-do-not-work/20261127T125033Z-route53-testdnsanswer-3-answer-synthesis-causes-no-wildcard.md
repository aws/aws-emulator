Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.test-dns-answer-expands-a-wildcard-record route53.test-dns-answer-serves-a-traffic-policy-instance-record route53.test-dns-answer-ns-record-delegation-boundary
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.test-dns-answer-expands-a-wildcard-record|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: route53.test-dns-answer-serves-a-traffic-policy-instance-record|run#19|AssertionError|assert answer["RecordData"] == ["203.0.113.30"], (
Issue-Key: route53.test-dns-answer-ns-record-delegation-boundary|run#16|AssertionError|assert sub_answer["RecordData"] == [], (
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# route53 TestDNSAnswer: 3 answer-synthesis causes — no wildcard expansion, no traffic-policy-instance record data, and NS data returned at a delegation boundary

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestDNSAnswer

## Description
TestDNSAnswer is served (it answers 200 with the modeled shape), but the RecordData it synthesises does not follow the zone's own resolution rules. Smithy (services/service-com-amazonaws-route53-awsdnsv20130401): TestDNSAnswer output requires Nameserver, RecordName, RecordType, RecordData (list) and ResponseCode (DNSRCode), Protocol; it is the API-level view of what the zone's name servers would answer for the queried name and type.

Cause 1 of 3: a name covered only by a wildcard record set is not answered from the wildcard.
AWS: a wildcard record set *.<zone> answers a query for any name below the zone that holds no record of its own, so TestDNSAnswer for anything.<zone> and deep.below.<zone>, type A, answers ResponseCode NOERROR with the wildcard's RecordData, while an explicit record still wins at its own name.
Emulator: the covered names never answer with the wildcard's value — the check polls TestDNSAnswer until its 60 s budget is exhausted and never observes the wildcard RecordData (the sibling cause below shows the emulator's RecordData for a name it cannot resolve is the empty list, so a poll for the expected data never completes).
Why: wildcard zones are untestable against the emulator: every name a client expects the wildcard to cover answers with no data, so catch-all DNS configurations appear broken.
Fix: the wildcard-matching step of the TestDNSAnswer handler's answer synthesis over zone state (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/test_dns_answer.rs).
Traces: route53.test-dns-answer-expands-a-wildcard-record.

Cause 2 of 3: a record created by a traffic policy instance answers with empty RecordData.
AWS: after CreateTrafficPolicyInstance, the instance's record set is served, so TestDNSAnswer for that name and type answers NOERROR with the policy endpoint's value in RecordData.
Emulator: answers HTTP 200, ResponseCode NOERROR, RecordData: [] for the policy record's name.
Why: a client that provisions DNS through traffic policies sees an existing-but-empty answer, so it cannot tell a published policy from an unpublished one.
Fix: the same handler must resolve record sets materialised by traffic policy instances, not only directly created ones.
Traces: route53.test-dns-answer-serves-a-traffic-policy-instance-record.

Cause 3 of 3: an NS query at a delegation boundary inside the zone answers with the delegation's name servers instead of no data.
AWS: TestDNSAnswer for a delegated subdomain's own NS record set answers NOERROR with an empty RecordData (the delegation NS set is authoritative below the cut, not answered here), while the same NS query at the zone apex answers NOERROR with the zone's own delegation-set name servers.
Emulator: returns the four delegation name-server hostnames as RecordData for the subdomain NS query.
Why: a client probing where a zone's authority ends sees the sub-delegation answered as if it were apex data, so delegation-boundary logic validated against the emulator is wrong against AWS.
Fix: the delegation-cut handling of the same handler's answer synthesis.
Traces: route53.test-dns-answer-ns-record-delegation-boundary.

(Related but distinct, and already open: 20261124T071615Z-route53-route-53-testdnsanswer-reports-the-zone-s-first.md covers the Nameserver field only; none of the three causes here is that one.)

## Evidence
From the replay output:
- route53.test-dns-answer-expands-a-wildcard-record: 'Error: Check timed out after 60.0s'. The check creates *.<zone> A 203.0.113.9 and www.<zone> A 203.0.113.1, then polls TestDNSAnswer until anything.<zone> and deep.below.<zone> answer RecordData == ['203.0.113.9']; that condition is never met inside the budget.
- route53.test-dns-answer-serves-a-traffic-policy-instance-record: AssertionError: the policy record must answer with the policy endpoint value 203.0.113.30: {'Nameserver': 'ns-...awsdns-...com', 'RecordName': 'tp.<zone>.', 'RecordType': 'A', 'RecordData': [], 'ResponseCode': 'NOERROR', 'Protocol': 'UDP'} (HTTP 200).
- route53.test-dns-answer-ns-record-delegation-boundary: AssertionError: expected no RecordData at the delegation boundary sub.<zone>., got ['ns-1707.awsdns-07.com', 'ns-542.awsdns-09.net', 'ns-1998.awsdns-18.org', 'ns-1422.awsdns-54.co.uk']. The check first confirms the sub-delegation NS record set is stored correctly via ListResourceRecordSets, so only the TestDNSAnswer synthesis diverges.

## Steps to Reproduce
1. Cause 1: CreateHostedZone; ChangeResourceRecordSets CREATE '*.<zone>' A 203.0.113.9 TTL 60 and 'www.<zone>' A 203.0.113.1 TTL 60; call TestDNSAnswer(HostedZoneId=<id>, RecordName='anything.<zone>', RecordType='A') — RecordData never becomes ['203.0.113.9'].
2. Cause 2: CreateTrafficPolicy + CreateTrafficPolicyInstance for 'tp.<zone>' with endpoint value 203.0.113.30; call TestDNSAnswer for that name and type A — RecordData is [] with ResponseCode NOERROR.
3. Cause 3: CreateHostedZone; CREATE an NS record set at 'sub.<zone>' holding the zone's four delegation-set name servers; call TestDNSAnswer(RecordName='sub.<zone>', RecordType='NS') — RecordData carries the four name servers where AWS returns [] (the apex NS query must still return them).
