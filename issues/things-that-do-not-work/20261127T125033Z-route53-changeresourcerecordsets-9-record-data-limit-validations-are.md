Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.rrset-mx-priority-above-65535-rejected route53.rrset-mx-value-without-priority-rejected route53.rrset-srv-value-not-four-fields-rejected route53.rrset-txt-string-over-255-characters-rejected route53.rrset-with-401-values-rejected route53.rrset-name-with-64-octet-label-rejected route53.rrset-cname-with-two-values-rejected route53.rrset-alias-to-missing-or-other-type-record-rejected route53.change-batch-upsert-counts-twice-against-the-element-limit
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.rrset-mx-priority-above-65535-rejected|_expect_error#9|AssertionError|raise AssertionError(f"expected {(code, message)!r}, but the call succeeded: {result!r}")
Issue-Key: route53.rrset-mx-value-without-priority-rejected|_expect_error#9|AssertionError|raise AssertionError(f"expected {(code, message)!r}, but the call succeeded: {result!r}")
Issue-Key: route53.rrset-srv-value-not-four-fields-rejected|_expect_error#9|AssertionError|raise AssertionError(f"expected {(code, message)!r}, but the call succeeded: {result!r}")
Issue-Key: route53.rrset-txt-string-over-255-characters-rejected|expect_error#9|AssertionError|raise AssertionError(f"expected ({code!r}, {message!r}) but the call succeeded: {result!r}")
Issue-Key: route53.rrset-with-401-values-rejected|expect_error#8|AssertionError|raise AssertionError(f"expected ({code!r}, {message!r}) but the call succeeded: {result!r}")
Issue-Key: route53.rrset-name-with-64-octet-label-rejected|_expect_error#9|AssertionError|raise AssertionError(f"expected {(code, message)!r}, but the call succeeded: {result!r}")
Issue-Key: route53.rrset-cname-with-two-values-rejected|expect_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: route53.rrset-alias-to-missing-or-other-type-record-rejected|expect_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: route53.change-batch-upsert-counts-twice-against-the-element-limit|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# route53 ChangeResourceRecordSets: 9 record-data/limit validations are not enforced — malformed MX/SRV/TXT/CNAME/alias record sets and over-limit batches are accepted with a change id

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
The emulator's ChangeResourceRecordSets accepts change batches that AWS Route 53 refuses, returning 200 with a ChangeInfo.Id and storing the record set. Nine independent validation rules are missing. Smithy (services/service-com-amazonaws-route53-awsdnsv20130401): ChangeResourceRecordSets declares the client errors InvalidChangeBatch (HTTP 400) and InvalidInput (HTTP 400); the response shape has only ChangeInfo, so a refusal is the only way AWS signals these. In every message below, the bracketed sentence text is AWS's fixed text for that error class and the names/types/zone id/values interpolated into it come from the request and the zone being changed — never from one recording.

Cause 1 of 9: an MX value whose priority exceeds 65535 is accepted.
AWS: refuses the batch with InvalidChangeBatch, HTTP 400, message "[Invalid Resource Record: 'FATAL problem: MXRRDATAPriorityOutOfBounds (MX record priority value is out of bounds) encountered with '<offending value>'']" (fixed text plus the rejected record value from the request).
Emulator: returns 200 with ChangeInfo.Id and stores the record set.
Why: a client that relies on Route 53 rejecting an out-of-range MX priority publishes a zone the real service would never hold, and sees no error to surface to its user.
Fix: the record-data validation of the ChangeResourceRecordSets handler (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs).
Traces: route53.rrset-mx-priority-above-65535-rejected.

Cause 2 of 9: an MX value with only one field (no priority) is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[Invalid Resource Record: 'FATAL problem: MXRRDATANotTwoFields (MX record doesn't have 2 fields) encountered with '<offending value>'']".
Emulator: 200 with a change id; the one-field MX is stored.
Why: a client cannot distinguish a well-formed MX RRSet from a malformed one, and mail routing data that AWS would reject is published.
Fix: same handler's record-data validation.
Traces: route53.rrset-mx-value-without-priority-rejected.

Cause 3 of 9: an SRV value that does not carry four fields is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[Invalid Resource Record: 'FATAL problem: SRVRRDATANotFourFields (SRV record doesn't have 4 fields) encountered with '<offending value>'']".
Emulator: 200 with a change id.
Why: service-discovery clients get an SRV RRSet real Route 53 would refuse, so the malformed form is never caught at write time.
Fix: same handler's record-data validation.
Traces: route53.rrset-srv-value-not-four-fields-rejected.

Cause 4 of 9: a TXT character-string longer than 255 characters is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[Invalid Resource Record: 'FATAL problem: CharacterStringTooLong (Value is too long) encountered with '<offending quoted string>'']". The 255-character ceiling is the DNS character-string limit Route 53 enforces per quoted string.
Emulator: 200 with a change id.
Why: a client splitting long TXT payloads (SPF/DKIM) never learns that a single string exceeded the limit, and the emulator holds a value the wire format cannot carry.
Fix: same handler's record-data validation.
Traces: route53.rrset-txt-string-over-255-characters-rejected.

Cause 5 of 9: an RRSet with more than 400 ResourceRecords is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[Cannot have more than 400 records in a RRSet.]" (fixed text; the limit is Route 53's documented per-RRSet maximum).
Emulator: 200 with a change id for a 401-value RRSet.
Why: a client sizing its batches against the documented per-RRSet maximum sees no refusal, so the limit is untestable against the emulator.
Fix: same handler's change-batch limit checks.
Traces: route53.rrset-with-401-values-rejected.

Cause 6 of 9: a record-set Name containing a DNS label longer than 63 octets is accepted.
AWS: InvalidChangeBatch, HTTP 400, "FATAL problem: DomainLabelTooLong (Domain label is too long) encountered with '<offending label>'" (fixed text plus the offending label taken from the requested Name).
Emulator: 200 with a change id.
Why: a name that cannot be encoded on the DNS wire is published, and the client's own name-length validation is never exercised.
Fix: the name validation of the same handler.
Traces: route53.rrset-name-with-64-octet-label-rejected.

Cause 7 of 9: a CNAME RRSet with two ResourceRecords is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[RRSet of type CNAME with DNS name <name> does not contain exactly one resource record.]" (fixed text; <name> is the requested record name).
Emulator: 200 with a change id; the two-valued CNAME is stored.
Why: a CNAME set that is unresolvable by definition is accepted, so a client never learns its write was wrong until resolution fails.
Fix: the per-type arity validation of the same handler.
Traces: route53.rrset-cname-with-two-values-rejected.

Cause 8 of 9: an in-zone alias whose target record does not exist (or exists with another type) is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[Tried to create an alias that targets <target dns name>, type <type> in zone <hosted zone id>, but that target was not found]" (fixed text; the target name, the alias record's type and the zone id come from the request).
Emulator: 200 with a change id for both an A alias to a missing name and an AAAA alias to an A-only name.
Why: dangling aliases are published that AWS refuses at write time, so a client's dependency ordering bug is silently absorbed.
Fix: the alias-target resolution/validation of the same handler against zone state.
Traces: route53.rrset-alias-to-missing-or-other-type-record-rejected.

Cause 9 of 9: the 1,000-element change-batch limit is not enforced, and an UPSERT is not counted as two elements.
AWS: a change batch may carry at most 1,000 ResourceRecord elements and each UPSERT counts twice, so 500 UPSERTs are accepted and 501 are refused with InvalidChangeBatch, HTTP 400, "[Number of records limit of 1000 exceeded.]" (fixed text), changing nothing.
Emulator: accepts 501 UPSERTs with 200 and a change id.
Why: a client that batches changes against the documented element budget cannot validate its chunking, and a batch AWS rejects atomically is applied here.
Fix: the element accounting in the same handler's change-batch limit checks.
Traces: route53.change-batch-upsert-counts-twice-against-the-element-limit.

## Evidence
All nine checks fail the same way: the call that AWS refuses returns success. Examples from the replay:
- route53.rrset-mx-priority-above-65535-rejected: AssertionError: expected ('InvalidChangeBatch', "[Invalid Resource Record: 'FATAL problem: MXRRDATAPriorityOutOfBounds (MX record priority value is out of bounds) encountered with '70000 mail.example.net.'']"), but the call succeeded: '/change/...'
- route53.rrset-mx-value-without-priority-rejected: expected ('InvalidChangeBatch', "[Invalid Resource Record: 'FATAL problem: MXRRDATANotTwoFields (MX record doesn't have 2 fields) encountered with 'mail.example.net.'']"), but the call succeeded.
- route53.rrset-srv-value-not-four-fields-rejected: expected ('InvalidChangeBatch', "[Invalid Resource Record: 'FATAL problem: SRVRRDATANotFourFields (SRV record doesn't have 4 fields) encountered with '10 sip.example.net.'']"), but the call succeeded.
- route53.rrset-txt-string-over-255-characters-rejected: expected ('InvalidChangeBatch', "[Invalid Resource Record: 'FATAL problem: CharacterStringTooLong (Value is too long) encountered with '\"aaa…\"'']"), but the call succeeded.
- route53.rrset-with-401-values-rejected: expected ('InvalidChangeBatch', '[Cannot have more than 400 records in a RRSet.]'), but the call succeeded.
- route53.rrset-name-with-64-octet-label-rejected: expected ('InvalidChangeBatch', "FATAL problem: DomainLabelTooLong (Domain label is too long) encountered with 'bbbb…(64 b's)'"), but the call succeeded.
- route53.rrset-cname-with-two-values-rejected: expected ('InvalidChangeBatch', '[RRSet of type CNAME with DNS name <name> does not contain exactly one resource record.]'), but the call succeeded.
- route53.rrset-alias-to-missing-or-other-type-record-rejected: expected ('InvalidChangeBatch', '[Tried to create an alias that targets <name>, type <type> in zone <zone id>, but that target was not found]'), but the call succeeded.
- route53.change-batch-upsert-counts-twice-against-the-element-limit: 500 UPSERTs accepted (correct), then 501 UPSERTs also accepted — expected ('InvalidChangeBatch', '[Number of records limit of 1000 exceeded.]').
In each case the emulator additionally leaves the refused record set in the zone, which the checks observe through ListResourceRecordSets.

## Steps to Reproduce
1. CreateHostedZone for a public zone (e.g. <name>.example.test.).
2. Call ChangeResourceRecordSets with a single CREATE carrying, one case per call: an MX value '70000 mail.example.net.'; an MX value 'mail.example.net.'; an SRV value '10 sip.example.net.'; a TXT value with one quoted string of 256+ characters; an RRSet with 401 ResourceRecords; a Name whose first label is 64 octets; a CNAME RRSet with two ResourceRecords; an A AliasTarget pointing at a name that holds no A record.
3. Observe HTTP 200 with ChangeInfo.Id instead of HTTP 400 InvalidChangeBatch, and the record set present in ListResourceRecordSets.
4. Then submit a batch of 501 UPSERTs of one-value A record sets and observe it is accepted instead of refused with '[Number of records limit of 1000 exceeded.]'.
