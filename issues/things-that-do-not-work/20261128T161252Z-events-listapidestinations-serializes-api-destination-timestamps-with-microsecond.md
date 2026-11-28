Issue-Source: trace-sweep-divergence
Issue-Trace: events-list-api-destinations-next-token-length
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-list-api-destinations-next-token-length
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-pgtok-a-d3d0b164"].CreationTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-pgtok-a-d3d0b164"].LastModifiedTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-pgtok-b-13ffa505"].CreationTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-pgtok-b-13ffa505"].LastModifiedTime
Issue-Commit: b325123fedfa77f0bbc54662ef6abf5ee95872ba

# events: ListApiDestinations serializes API-destination timestamps with microsecond precision

> _Note: this issue was identified against the code base at commit `b325123fedfa77f0bbc54662ef6abf5ee95872ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiDestinations

## Description
One root cause, 1 of 1.

Cause 1 of 1: the ListApiDestinations response serializes every API-destination timestamp as an epoch-seconds float carrying microsecond (6-7 digit) precision, for each element of the returned list.

AWS: EventBridge speaks awsJson1_1, whose default timestamp encoding is epoch-seconds (Smithy: service com.amazonaws.eventbridge#AWSEvents, protocols [awsJson1_1]). ListApiDestinations.output.ApiDestinations targets ApiDestinationResponseList of ApiDestination, whose CreationTime and LastModifiedTime members both target the Timestamp shape (Smithy: ApiDestination.CreationTime -> Timestamp, ApiDestination.LastModifiedTime -> Timestamp; ListApiDestinations has no modeled client error, only InternalException). The model states only the type; the fractional resolution AWS actually puts on the wire is at most milliseconds -- recorded behavior only for that limit. The recorded response for this step is HTTP 200 with epoch-seconds values whose fractional part does not exceed millisecond resolution; there is no error code or message involved, the status matches on both sides and only the value shape differs.

Emulator: answers HTTP 200 but emits the raw clock reading with 6-7 fractional digits on both members of every list element. Replay reports, per element keyed by Name: "$.ApiDestinations[Name="<destination-name>"].CreationTime: value_mismatch (expected '<shaped-not-asserted>', got <epoch float with 7 fractional digits>)" and the same for .LastModifiedTime. The fixture asserts the SHAPE of the value, not the instant, so the instant itself is not the finding -- the precision is. Concretely in this replay both listed destinations came back with CreationTime == LastModifiedTime at 7 fractional digits (e.g. 1795882201.9037566 and 1795882201.9102378); those particular numbers and the generated destination names are per-run values, not requirements.

Why: a client that re-serializes, hashes, caches or diffs the listing, or that validates the response against the API shape, sees a precision real EventBridge never emits. Equality against a value it previously stored from a Describe/Create call, or against an AWS-SDK- or CloudTrail-formatted timestamp, fails; a consumer that assumes millisecond-resolution epochs mis-parses the trailing digits; and paginating clients that key cached pages on these values see spurious change.

Fix: the timestamp serializer used by the events ListApiDestinations response serializer (the shared control-plane timestamp formatting path that the ApiDestination list element serializer inherits) -- truncate/round to at most millisecond resolution before emitting the epoch-seconds number.

Relation to an already-open issue: the open issue 20261127T145950Z-events-control-plane-responses-serialize-timestamps-with-microsecond.md describes the same class of fault and even lists ListApiDestinations keys, but its replay keys for this list are pinned to the per-run generated destination names from an earlier recording (sim2real-probe-events-pgtok-a-fb832fd9 / -b-3edc90f6), so this run's cluster (names ...-a-d3d0b164 / ...-b-13ffa505) is NOT owned by it and its replay pointer does not cover this step. That issue does not close this cluster; this issue carries its replay key.

Traces: events-list-api-destinations-next-token-length#task#8 (ListApiDestinations, HTTP 200 both sides).

## Evidence
Replay cluster (service=events|ListApiDestinations|status=200->200), fixture events-list-api-destinations-next-token-length, step task#8, expected_status 200, actual_status 200, owned_by [] (not covered by any open issue), stub_path "" (operation is implemented).

Detail as returned by the replay:
  $.ApiDestinations[Name="sim2real-probe-events-pgtok-a-d3d0b164"].CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1795882201.9037566)
  $.ApiDestinations[Name="sim2real-probe-events-pgtok-a-d3d0b164"].LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1795882201.9037566)
  $.ApiDestinations[Name="sim2real-probe-events-pgtok-b-13ffa505"].CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1795882201.9102378)
  $.ApiDestinations[Name="sim2real-probe-events-pgtok-b-13ffa505"].LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1795882201.9102378)

Expected: an awsJson1_1 epoch-seconds number for each ApiDestination.CreationTime / LastModifiedTime whose fractional part does not exceed millisecond resolution.
Actual: an epoch-seconds number with 7 fractional digits on every member of every list element.

The destination names, the account/region in the ARNs and the instants themselves are per-run values and are not part of the requirement; the requirement is the serialized fractional resolution of ApiDestination.CreationTime and ApiDestination.LastModifiedTime. No status, error code or error message diverges in this cluster -- both sides are HTTP 200.

## Steps to Reproduce
1. CreateConnection, then CreateApiDestination twice against that connection (two distinct names sharing a common NamePrefix).
2. Call ListApiDestinations with that NamePrefix (optionally with Limit/NextToken, as the fixture does while exercising the NextToken length bound).
3. Inspect $.ApiDestinations[*].CreationTime and $.ApiDestinations[*].LastModifiedTime in the HTTP 200 body: the emulator returns epoch-seconds floats with 6-7 fractional digits, where real EventBridge serializes at most millisecond resolution.
4. Re-replay fixture events-list-api-destinations-next-token-length, step task#8, to confirm the fix.
