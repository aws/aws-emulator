Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.rrset-set-identifier-without-routing-policy-rejected route53.rrset-weight-and-multivalue-together-rejected route53.rrset-geolocation-continent-with-country-rejected route53.rrset-multivalue-alias-rejected route53.rrset-multivalue-cname-rejected route53.route53-geoproximity-location-naming-a-region-and-coordinates-rejected route53.route53-geoproximity-records-at-one-name-sharing-a-region-rejected
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.rrset-set-identifier-without-routing-policy-rejected|_expect_error#8|AssertionError|raise AssertionError(f"expected {(code, message)!r}, but the call succeeded: {result!r}")
Issue-Key: route53.rrset-weight-and-multivalue-together-rejected|expect_error#8|AssertionError|raise AssertionError(f"expected ({code!r}, {message!r}) but the call succeeded: {result!r}")
Issue-Key: route53.rrset-geolocation-continent-with-country-rejected|expect_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: route53.rrset-multivalue-alias-rejected|expect_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: route53.rrset-multivalue-cname-rejected|expect_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: route53.route53-geoproximity-location-naming-a-region-and-coordinates-rejected|_refusal#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: route53.route53-geoproximity-records-at-one-name-sharing-a-region-rejected|_refusal#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# route53 ChangeResourceRecordSets: 7 routing-policy validations are not enforced — illegal SetIdentifier/weighted/geolocation/multivalue/geoproximity record sets are accepted

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
A second, disjoint family of missing validation in the same operation: the emulator accepts routing-policy combinations that AWS Route 53 refuses. Smithy (services/service-com-amazonaws-route53-awsdnsv20130401): ChangeResourceRecordSets declares InvalidInput (HTTP 400) and InvalidChangeBatch (HTTP 400); its output carries only ChangeInfo. In the messages below the sentence text is AWS's fixed text for the class and the record name, type, SetIdentifier, location codes and Region come from the request.

Cause 1 of 7: a SetIdentifier with no routing policy at all is accepted.
AWS: InvalidInput, HTTP 400, "Invalid request: Expected exactly one of [Weight, Region, Failover, GeoLocation, MultiValueAnswer, GeoProximityLocation, CidrRoutingConfig, or AZAffinity], but found none in Change with [Action=<action>, Name=<name>, Type=<type>, SetIdentifier=<id>]" (fixed list and clause; the bracketed Change fields echo the request).
Emulator: 200 with ChangeInfo.Id.
Why: a client that forgot its routing policy gets an un-routable identified record set instead of the refusal AWS gives, so the bug surfaces only at resolution time.
Fix: the routing-policy exclusivity validation of the ChangeResourceRecordSets handler (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs).
Traces: route53.rrset-set-identifier-without-routing-policy-rejected.

Cause 2 of 7: two routing policies on one record set (Weight together with MultiValueAnswer) are accepted.
AWS: InvalidInput, HTTP 400, same fixed list with "but found more than one in Change with [Action=<action>, Name=<name>, Type=<type>, SetIdentifier=<id>]".
Emulator: 200 with ChangeInfo.Id.
Why: the exclusivity rule a client depends on is unenforced, so a record set with contradictory routing is published and answered by some arbitrary policy.
Fix: same exclusivity validation.
Traces: route53.rrset-weight-and-multivalue-together-rejected.

Cause 3 of 7: a GeoLocation naming a continent and a country at once is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[Cannot find location: ' continent = <continent code>country = <country code>'.  No changes made.]" (fixed text, including its internal spacing and the trailing 'No changes made.' clause; the two codes come from the request).
Emulator: 200 with ChangeInfo.Id.
Why: an unresolvable geolocation is stored, and the client never learns the pairing is not a location AWS knows.
Fix: the geolocation validation of the same handler.
Traces: route53.rrset-geolocation-continent-with-country-rejected.

Cause 4 of 7: an alias record set marked MultiValueAnswer is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[Multivalue answer rrset should not be an alias rrset. No changes made.]" (fixed text).
Emulator: 200 with ChangeInfo.Id.
Why: multivalue-answer routing cannot be combined with an alias target in AWS; accepting it lets a client ship a configuration real Route 53 rejects.
Fix: the multivalue-answer validation of the same handler.
Traces: route53.rrset-multivalue-alias-rejected.

Cause 5 of 7: a MultiValueAnswer CNAME record set is accepted.
AWS: InvalidChangeBatch, HTTP 400, "[RRSet with DNS name <name>, type <type>, SetIdentifier <id> cannot be created because this type of RRSet is not supported.]" (fixed text; the three fields echo the request).
Emulator: 200 with ChangeInfo.Id.
Why: multivalue answer supports every type except NS and CNAME; the unsupported type is accepted here, so the client's error path is never reached.
Fix: the multivalue-answer type check of the same handler.
Traces: route53.rrset-multivalue-cname-rejected.

Cause 6 of 7: a GeoProximityLocation naming both an AWSRegion and coordinates is accepted.
AWS: a geoproximity location names exactly one of AWSRegion, LocalZoneGroup or Coordinates (API_GeoProximityLocation); naming a Region and Coordinates together is refused with InvalidInput, HTTP 400, and nothing is stored.
Emulator: the CREATE succeeds (the check records 'expected the call to be refused, but it succeeded').
Why: a client cannot rely on the mutual exclusivity of the geoproximity location members; the record is stored with an ambiguous location.
Fix: the GeoProximityLocation validation of the same handler.
Traces: route53.route53-geoproximity-location-naming-a-region-and-coordinates-rejected.

Cause 7 of 7: two geoproximity record sets at one name sharing the same AWSRegion are accepted.
AWS: geoproximity siblings at one name must have distinct locations; a second record set with the same AWSRegion (whatever its Bias) is refused with InvalidChangeBatch, HTTP 400, and leaves nothing behind.
Emulator: the CREATE succeeds.
Why: duplicate-location siblings make the nearest-region choice undefined, and the client's uniqueness assumption is never checked.
Fix: the geoproximity sibling-uniqueness check of the same handler, against the sibling record sets in zone state.
Traces: route53.route53-geoproximity-records-at-one-name-sharing-a-region-rejected.

## Evidence
From the replay output:
- route53.rrset-set-identifier-without-routing-policy-rejected: AssertionError: expected ('InvalidInput', 'Invalid request: Expected exactly one of [Weight, Region, Failover, GeoLocation, MultiValueAnswer, GeoProximityLocation, CidrRoutingConfig, or AZAffinity], but found none in Change with [Action=CREATE, Name=s.<zone>., Type=A, SetIdentifier=a]'), but the call succeeded: '/change/...'
- route53.rrset-weight-and-multivalue-together-rejected: expected the same InvalidInput text with 'but found more than one in Change with [...]' but the call succeeded.
- route53.rrset-geolocation-continent-with-country-rejected: expected ('InvalidChangeBatch', "[Cannot find location: ' continent = <code>country = <code>'.  No changes made.]"), but the call succeeded.
- route53.rrset-multivalue-alias-rejected: expected ('InvalidChangeBatch', '[Multivalue answer rrset should not be an alias rrset. No changes made.]'), but the call succeeded.
- route53.rrset-multivalue-cname-rejected: expected ('InvalidChangeBatch', '[RRSet with DNS name <name>, type CNAME, SetIdentifier <id> cannot be created because this type of RRSet is not supported.]'), but the call succeeded.
- route53.route53-geoproximity-location-naming-a-region-and-coordinates-rejected: check declares _REFUSAL_CODE = 'InvalidInput'; AssertionError: expected the call to be refused, but it succeeded: None.
- route53.route53-geoproximity-records-at-one-name-sharing-a-region-rejected: check declares _REFUSAL_CODE = 'InvalidChangeBatch'; AssertionError: expected the call to be refused, but it succeeded: None.

## Steps to Reproduce
1. CreateHostedZone for a public zone.
2. Call ChangeResourceRecordSets with a single CREATE for each case: (a) an A record set with SetIdentifier and no routing member; (b) an A record set with both Weight and MultiValueAnswer=true; (c) an A record set whose GeoLocation sets ContinentCode and CountryCode together; (d) an alias record set with MultiValueAnswer=true; (e) a CNAME record set with SetIdentifier and MultiValueAnswer=true; (f) an A record set whose GeoProximityLocation sets AWSRegion and Coordinates together; (g) after storing one geoproximity record set for a Region, a second record set at the same name with the same AWSRegion.
3. Each call returns HTTP 200 with ChangeInfo.Id; AWS returns HTTP 400 InvalidInput (a, b, f) or InvalidChangeBatch (c, d, e, g) and stores nothing.
