Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected
Issue-Shape-Id: com.amazonaws.partnercentralrevenuemeasurement#PartnerCentralRevenueMeasurement
Issue-Key: partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected|<lambda>#6|MemoryError|("GetRevenueAttribution", lambda: pc.get_revenue_attribution(Catalog=_CATALOG, Identifier=missing),
Issue-Commit: a069a9a0ca52a06ec1b3b6edb1946a214c0d1090

# partnercentral-revenue-measurement: 2 net-new causes — the response to an rpcv2Cbor request is not decodable CBOR (client dies with MemoryError, no status/code reaches it), and the whole revenue-attribution read/update surface is a 501 scaffold stub instead of answering ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `a069a9a0ca52a06ec1b3b6edb1946a214c0d1090`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRevenueAttribution

## Description
Two net-new root causes, both grounded in the single failing behavioral check for this service (partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected). The check never got past its FIRST call, so everything below is anchored on what that first call did; nothing is claimed about the later assertions in it. Neither cause is covered by an existing open issue — the open partnercentral issues are all for partnercentral-channel, partnercentral-account and partnercentral-benefits, and the two open rpc-v2-cbor issues (20261125T220211Z-cloudwatch-..., 20261126T041316Z-cloudwatch-...) are cloudwatch ValidationException wording, not this.

=== Cause 1 of 4 is not a thing; there are exactly TWO causes. ===

Cause 1 of 2: the response this service sends to an rpcv2Cbor request is not decodable CBOR, so NO status, error code or message ever reaches the client.
Failing check ids: partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected

AWS: Smithy — service com.amazonaws.partnercentralrevenuemeasurement#PartnerCentralRevenueMeasurement declares exactly one protocol, smithy.protocols#rpcv2Cbor (sdkId "PartnerCentral Revenue Measurement", endpointPrefix partnercentral-prm, sigv4Name partnercentral). On that protocol EVERY response — success or error — is a CBOR map body served with `content-type: application/cbor` and the `smithy-protocol: rpc-v2-cbor` response header, and an error body is the CBOR map `{"__type": <error code>, "message": <message>}` (or the modeled error struct plus `__type`). There is no response shape on this service that is JSON, XML or text. For the specific call the check makes, AWS answers GetRevenueAttribution on a well-formed id that does not exist with HTTP 404 and error code ResourceNotFoundException (Smithy: ResourceNotFoundException @error("client") @httpError(404), required members Message and Reason); recorded behavior for the message is the fixed text `We couldn't find revenue attribution '` + the Identifier value taken from the request + `'. Verify the ID and try again.` — the only variable part is the id from the request. The sibling list operation words the same miss differently: ListRevenueAttributions with an unknown member in Identifiers answers ResourceNotFoundException with the fixed text `Revenue attribution ` + the id from the request + ` doesn't exist. Verify the ID and try again.` (no quotes around the id).

Emulator: the call does not return and does not raise ClientError. botocore's CBOR decoder dies while parsing the response body:
  File "parsers.py", line 851, in _parse_byte_string / return self._read_from_stream(stream, length)
  File "parsers.py", line 965, in _read_from_stream / value = stream.read(num_bytes)
  MemoryError
A MemoryError at that exact point means the decoder read a string-item length that is astronomically large, which only happens when the bytes at a string position are not a CBOR string header at all — i.e. the body is not CBOR. (Hypothesis, stated as such: a JSON body is the shape that produces precisely this; its leading `{` is 0x7B, which CBOR reads as "text string, 8-byte length follows", so the next 8 body bytes are consumed as the length and the decoder tries to allocate it. A `<`-leading XML body or an empty body would fail differently.) The net effect is protocol-level, not fidelity-level: the client sees neither the HTTP status nor an error code nor a message, so no SDK or CLI can talk to this service at all.

Why: a client relying on AWS's behavior expects every answer from this endpoint to be a decodable rpcv2Cbor response, and expects a missing resource to surface as a typed ResourceNotFoundException it can branch on. Instead the SDK's parser raises a MemoryError out of the response-parsing layer — not a ClientError, not a modeled exception — so error handling, retry classification and even "did the call fail?" are all unreachable. A caller cannot distinguish a not-found from a throttle from a server fault; worse, MemoryError escapes normal `except ClientError` handling and can take the calling process down.

Fix: the response-rendering path for this service's declared envelope. The per-protocol envelope must be used for EVERY response this endpoint can emit, including the ones rendered before or outside a modeled error. Note that the router's two own refusals are rendered in awsJson1.1 unconditionally — `unknown_service_response` (404 UnknownService) and `ambiguous_service_response` (500 AmbiguousService) both call `aws_json_error_response(.., AwsJsonProtocol::V1_1)` with no regard for the protocol the targeted service declares — while every other refusal path in the same dispatch correctly renders per protocol (the authentication denial goes through `definition().refusal_response(..)`, the routing failure through `definition().routing_error_response(..)`, and `pre_dispatch::rpc_v2_cbor` renders a proper CBOR envelope via `cbor_synthetic_error_body`). Those two router-level renderers are the one place in the dispatch that can hand an rpcv2Cbor client a JSON body, so they are the first thing to check and, either way, must render in the request's protocol. If instead the body comes out of the operation's own serializer, the fix belongs in that serializer's error path.

Traces: partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected (first call in the check: GetRevenueAttribution, Catalog=Sandbox, Identifier=a well-formed but unknown `ra-<13 hex chars>`). File/line citations for the awsJson-rendered router refusals: vendor/smithy-server-runtime/src/multi_service_router.rs lines 483-491 (unknown_service_response) and 496-509 (ambiguous_service_response); the protocol-correct CBOR renderer that should be used instead is vendor/smithy-server-runtime/src/pre_dispatch.rs lines 396-413, over vendor/smithy-schema-server/src/rpc.rs lines 571-580.

Cause 2 of 2: every revenue-attribution read and update operation is an unimplemented scaffold stub answering 501 NotImplemented, so none of them can answer the modeled ResourceNotFoundException for an id that does not exist.
Failing check ids: partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected

AWS: Smithy — GetRevenueAttribution, ListRevenueAttributions, ListRevenueAttributionAllocations, GetRevenueAttributionAllocation and UpdateRevenueAttribution all declare ResourceNotFoundException (@httpError(404), required Message and Reason:ResourceNotFoundExceptionReason); UpdateRevenueAttribution additionally declares ConflictException. A read or update addressed to a well-formed RevenueAttributionIdentifier (string, @length 16..1011, @pattern ^(arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/revenue-attribution/ra-[a-z0-9]{13}|ra-[a-z0-9]{13})$) that names no stored attribution is refused 404 ResourceNotFoundException naming that id, with the per-operation wording given under Cause 1. 501 NotImplemented is not a modeled answer for any of them.

Emulator: each of the five handlers still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))`, which renders HTTP 501 with error code `NotImplemented` and the message `operation <Op> is scaffolded but not implemented in this emulator`. So the whole revenue-attribution plane — get, list, list-allocations, get-allocation, update — asserts nothing about this service's real behavior. (The check could not observe this directly because Cause 1 destroys the response before any code reaches the client; the handler bodies in the checkout are the evidence.)

Why: a client branching on ResourceNotFoundException to tell "this attribution is gone" from "the service is broken" gets a 501 with a non-modeled error code instead. 501 is also outside the modeled error set, so a generated SDK surfaces it as an unmodeled/unknown error rather than the typed exception the caller handles, and nothing in the emulator ever stores or looks up a revenue attribution.

Fix: implement the five handlers, each in its own file, and return the modeled error rather than the scaffold signal: `Err(<Op>ErrorModeled::ResourceNotFoundException(ResourceNotFoundException { .. }).into())` with Message worded as above and Reason set, after a lookup against the service state. Belongs in services/service-com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement/src/ops/ (get_revenue_attribution.rs, list_revenue_attributions.rs, list_revenue_attribution_allocations.rs, get_revenue_attribution_allocation.rs, update_revenue_attribution.rs) plus the crate's state module for the store they read; delete the SCAFFOLDED marker line in each file as it is implemented.

Traces: partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected. Handler citations: src/ops/get_revenue_attribution.rs lines 3-6 (marker) and 196 (`Scaffolded::not_implemented`), src/ops/list_revenue_attributions.rs lines 3-6, src/ops/update_revenue_attribution.rs lines 3-6; the 501/NotImplemented rendering itself is src/scaffolded.rs lines 93-99 and 273-291.

Note for the fixer: Cause 1 gates Cause 2. While the response body is undecodable the check cannot observe any error code at all, so fix Cause 1 first, re-replay the check, then implement Cause 2. The check also asserts, after the five not-found calls, that a malformed id (`ra-NOT-VALID-ID`, sent with botocore parameter validation off) is refused 400 ValidationException with the message `2 validation errors detected: ` followed by the two constraint sentences in either order — `Value 'ra-NOT-VALID-ID' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/revenue-attribution/ra-[a-z0-9]{13}|ra-[a-z0-9]{13})` and `Value 'ra-NOT-VALID-ID' at 'identifier' failed to satisfy constraint: Member must have length greater than or equal to 16`. Note the reported pattern is the modeled @pattern with its ^ and $ anchors stripped, the member is named in lowerCamel (`identifier`), and the count prefix is the number of failed constraints. That assertion was never reached, so it is NOT filed as a cause here — but it is what the check will assert next once the two causes above are fixed.

## Evidence
Suite report (behavioral), service heading `### partnercentral-revenue-measurement`:

#### [error] partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected
Error: File "parsers.py", line 851, in _parse_byte_string
    return self._read_from_stream(stream, length)
  File "parsers.py", line 965, in _read_from_stream
    value = stream.read(num_bytes)
MemoryError

Expected (AWS), first call of the check — GetRevenueAttribution(Catalog="Sandbox", Identifier="ra-<13 hex>" that does not exist):
  HTTP 404, decodable rpcv2Cbor body (`content-type: application/cbor`, `smithy-protocol: rpc-v2-cbor`), error code `ResourceNotFoundException`, Message = "We couldn't find revenue attribution '<Identifier>'. Verify the ID and try again." (fixed text; <Identifier> is the request's value), Reason set.
  ListRevenueAttributions(Identifiers=[that id]) -> same code, Message = "Revenue attribution <Identifier> doesn't exist. Verify the ID and try again."
  ListRevenueAttributionAllocations / GetRevenueAttributionAllocation / UpdateRevenueAttribution on that id -> 404 ResourceNotFoundException with the first wording.

Actual (emulator):
  Cause 1 — no HTTP status, no error code and no message are observable: botocore's CBOR decoder raises MemoryError out of `_parse_byte_string`/`_read_from_stream` because the body at a string position is not a CBOR string header (it read a length too large to allocate). The call neither succeeds nor raises ClientError.
  Cause 2 — the five handlers in the checkout are scaffold stubs: `Err(crate::scaffolded::Scaffolded::not_implemented("GetRevenueAttribution"))` (src/ops/get_revenue_attribution.rs:196), which renders HTTP 501, error code `NotImplemented`, message "operation GetRevenueAttribution is scaffolded but not implemented in this emulator" (src/scaffolded.rs:93-99, 273-291) — neither the status nor the code AWS answers.

Model facts cited (smithy_query on services/service-com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement/smithy-model/com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement.json):
  protocols: ["rpcv2Cbor"]; sigv4Name "partnercentral"; endpointPrefix "partnercentral-prm".
  ResourceNotFoundException: errorKind client, httpError 404, required Message + Reason.
  ValidationException: errorKind client, httpError 400, required Message + Reason, optional FieldList.
  RevenueAttributionIdentifier: @length 16..1011, @pattern ^(arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/revenue-attribution/ra-[a-z0-9]{13}|ra-[a-z0-9]{13})$.
  GetRevenueAttribution / GetRevenueAttributionAllocation / ListRevenueAttributions / ListRevenueAttributionAllocations declare ResourceNotFoundException; UpdateRevenueAttribution declares ConflictException + ResourceNotFoundException.

Router evidence for the non-CBOR body (hypothesis for Cause 1's mechanism):
  vendor/smithy-server-runtime/src/multi_service_router.rs:483-491 `unknown_service_response` -> `aws_json_error_response(404, "UnknownService", .., AwsJsonProtocol::V1_1)`
  vendor/smithy-server-runtime/src/multi_service_router.rs:496-509 `ambiguous_service_response` -> `aws_json_error_response(500, "AmbiguousService", .., AwsJsonProtocol::V1_1)`
  Both ignore the targeted service's declared protocol, unlike the auth-denial path (:261-268, `definition().refusal_response`) and the routing-failure path (:246-257, `definition().routing_error_response`). The protocol-correct CBOR envelope exists and is used elsewhere: vendor/smithy-server-runtime/src/pre_dispatch.rs:396-413 over vendor/smithy-schema-server/src/rpc.rs:571-580.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1 — reproduce directly with botocore against the emulator:
   client = session.client("partnercentral-revenue-measurement", region_name="us-east-1", endpoint_url=<emulator endpoint>)
   client.get_revenue_attribution(Catalog="Sandbox", Identifier="ra-" + uuid.uuid4().hex[:13])
   Observed: MemoryError raised from botocore/parsers.py `_parse_byte_string` -> `_read_from_stream`, not a ClientError.
   Confirm the wire body independently (the SDK cannot be used to see it): send the same request with curl —
   curl -sS -D- -X POST "<emulator endpoint>/service/PartnerCentralRevenueMeasurement/operation/GetRevenueAttribution" -H 'smithy-protocol: rpc-v2-cbor' -H 'content-type: application/cbor' --data-binary @<cbor body> | xxd | head
   Expected: `content-type: application/cbor`, `smithy-protocol: rpc-v2-cbor`, and a body whose first byte is a CBOR map header (0xa2 or 0xbf). Observed: a body that is not CBOR (a leading 0x7B `{` is what reproduces the MemoryError exactly).
3. Cause 2 — once step 2's body is decodable, the same call surfaces HTTP 501 with error code `NotImplemented` and message "operation GetRevenueAttribution is scaffolded but not implemented in this emulator" instead of HTTP 404 `ResourceNotFoundException`. Repeat for list_revenue_attributions(Catalog="Sandbox", Identifiers=[id]), list_revenue_attribution_allocations(Catalog="Sandbox", RevenueAttributionIdentifier=id), get_revenue_attribution_allocation(Catalog="Sandbox", RevenueAttributionIdentifier=id, RevenueAttributionAllocationId="alloc-<13 hex>") and update_revenue_attribution(Catalog="Sandbox", Identifier=id, Revision="1", ClientToken=<token>, Description="updated description") — all five answer the same 501.
4. Re-replay partnercentral-revenue-measurement.revenue-attribution-unknown-id-not-found-and-malformed-id-rejected to confirm both causes are closed.
