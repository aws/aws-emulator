Issue-Source: trace-sweep-divergence
Issue-Trace: location-place-index-crud-and-conflict
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-place-index-crud-and-conflict
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|DescribePlaceIndex|4xx->2xx|$.message
Issue-Key: location|DescribePlaceIndex|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: location|DescribePlaceIndex|4xx->2xx|status
Issue-Commit: 89891cf26090dcd00165ccf096d4c55bb51500e1

# Location DescribePlaceIndex answers 200 for a deleted place index instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `89891cf26090dcd00165ccf096d4c55bb51500e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePlaceIndex

## Description
Cause 1 of 1: a place index that has been deleted is still described successfully, so deletion is not observable through the control-plane read path.

AWS: Smithy (com-amazonaws-location-locationservice): DeletePlaceIndex DELETE /places/v0/indexes/{IndexName} is @idempotent and removes the index resource; DescribePlaceIndex GET /places/v0/indexes/{IndexName} is @readonly and models ResourceNotFoundException (errorKind client, httpError 404) among its errors. Once the index named by the required IndexName httpLabel no longer exists, Amazon Location answers that read with HTTP 404 ResourceNotFoundException (header x-amzn-errortype: ResourceNotFoundException) and a body whose message is of the fixed form "Place index not found: <IndexName>." — the fixed text is "Place index not found: " plus the terminating period, and the only variable part is <IndexName>, taken verbatim from the request's IndexName path label. The recorded response for this step is exactly that: status 404 with message "Place index not found: sim2real-probe-location-index-930b4bd2." (that name is this run's probe value filling the template, not part of the requirement).

Emulator: the same GET after the index was deleted answers HTTP 200 with a full DescribePlaceIndexResponse body — no $.message and no x-amzn-errortype header are present (replay detail: "status: expected 404, got 200"; "$.message: missing (expected 'Place index not found: <IndexName>.', got None)"; "$headers.x-amzn-errortype: missing"). The place-index record therefore survives DeletePlaceIndex, or the describe path does not consult the deletion.

Why: a client that relies on AWS's behavior cannot confirm a delete. Teardown/cleanup loops that poll DescribePlaceIndex until it raises ResourceNotFoundException never terminate; code that treats a successful describe as "the resource exists" will skip re-creating an index that is gone, and a subsequent CreatePlaceIndex of the same name may be refused ConflictException against a record that should have been removed. Deleted-resource 404s are also how SDK waiters and CloudFormation-style delete verification decide success, so the whole delete half of the place-index lifecycle is unverifiable.

Fix: the place-index state store and the DeletePlaceIndex handler's state mutation — the delete must remove (or mark absent) the index record keyed by IndexName — together with the DescribePlaceIndex handler's lookup, which must raise the modeled ResourceNotFoundException (HTTP 404, x-amzn-errortype: ResourceNotFoundException) with the message template above when the lookup misses, instead of serving a record.

Traces: location-place-index-crud-and-conflict#task#6 (DescribePlaceIndex after the fixture's DeletePlaceIndex step).

Note: no open issue covers this path; the open Location issues 20261111T061520Z-location-resource-tagging-plane-diverges-tagresource-untagresource-answer.md (tagging plane), 20261118T080214Z-location-putgeofence-ring-orientation-validationexception-names-the-indexed-ring.md, 20261115T202735Z-location-getmapstyledescriptor-serves-an-emulator-synthesized-style-document.md and 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit.md concern other operations.

## Evidence
Fixture location-place-index-crud-and-conflict, step task#6, operation DescribePlaceIndex (GET /places/v0/indexes/{IndexName}).
Expected (recorded AWS): HTTP 404; body {"message": "Place index not found: <IndexName>."}; header x-amzn-errortype: ResourceNotFoundException (shaped, not asserted on value).
Actual (emulator): HTTP 200 with a DescribePlaceIndexResponse body; $.message absent; x-amzn-errortype header absent.
Replay detail verbatim: "status: expected 404, got 200\n$.message: missing (expected 'Place index not found: sim2real-probe-location-index-930b4bd2.', got None)\n$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)".
Divergence key: service=location|DescribePlaceIndex|status=404->200|keys=$.message,$headers.x-amzn-errortype,status. Smithy: DescribePlaceIndex errors include ResourceNotFoundException (httpError 404); DeletePlaceIndex is idempotent DELETE on the same URI.

## Steps to Reproduce
1. CreatePlaceIndex with a fresh IndexName (POST /places/v0/indexes) and a DataSource, as fixture location-place-index-crud-and-conflict does.
2. DeletePlaceIndex on that IndexName (DELETE /places/v0/indexes/{IndexName}).
3. Call DescribePlaceIndex on the same IndexName (GET /places/v0/indexes/{IndexName}).
4. Observe HTTP 200 with the index's full description. AWS answers HTTP 404 ResourceNotFoundException with message "Place index not found: <IndexName>." and header x-amzn-errortype: ResourceNotFoundException.
Replay: replay_wire_corpus, fixture location-place-index-crud-and-conflict, step task#6.
