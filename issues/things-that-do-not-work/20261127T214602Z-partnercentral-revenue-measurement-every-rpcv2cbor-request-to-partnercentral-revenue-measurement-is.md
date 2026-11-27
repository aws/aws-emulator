Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-revenue-measurement-create-revenue-attribution-validation-refusals
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-revenue-measurement-create-revenue-attribution-validation-refusals partnercentral-revenue-measurement-listings-and-tag-reads partnercentral-revenue-measurement-start-allocations-task-validation-refusals
Issue-Shape-Id: com.amazonaws.partnercentralrevenuemeasurement#PartnerCentralRevenueMeasurement
Issue-Key: partnercentral-revenue-measurement|CreateRevenueAttribution|4xx->4xx|$.FieldList
Issue-Key: partnercentral-revenue-measurement|CreateRevenueAttribution|4xx->4xx|$.Message
Issue-Key: partnercentral-revenue-measurement|CreateRevenueAttribution|4xx->4xx|$.Reason
Issue-Key: partnercentral-revenue-measurement|CreateRevenueAttribution|4xx->4xx|$.__type
Issue-Key: partnercentral-revenue-measurement|CreateRevenueAttribution|4xx->4xx|$headers.content-type
Issue-Key: partnercentral-revenue-measurement|CreateRevenueAttribution|4xx->4xx|status
Issue-Key: partnercentral-revenue-measurement|ListRevenueAttributions|2xx->4xx|$.RevenueAttributionSummaries
Issue-Key: partnercentral-revenue-measurement|ListRevenueAttributions|2xx->4xx|$headers.content-type
Issue-Key: partnercentral-revenue-measurement|ListRevenueAttributions|2xx->4xx|status
Issue-Key: partnercentral-revenue-measurement|ListMarketplaceRevenueShares|2xx->4xx|$.MarketplaceRevenueShareSummaries
Issue-Key: partnercentral-revenue-measurement|ListMarketplaceRevenueShares|2xx->4xx|$headers.content-type
Issue-Key: partnercentral-revenue-measurement|ListMarketplaceRevenueShares|2xx->4xx|status
Issue-Key: partnercentral-revenue-measurement|ListRevenueAttributions|4xx->4xx|$.Message
Issue-Key: partnercentral-revenue-measurement|ListRevenueAttributions|4xx->4xx|$.Reason
Issue-Key: partnercentral-revenue-measurement|ListRevenueAttributions|4xx->4xx|$.__type
Issue-Key: partnercentral-revenue-measurement|ListRevenueAttributions|4xx->4xx|$headers.content-type
Issue-Key: partnercentral-revenue-measurement|ListRevenueAttributions|4xx->4xx|status
Issue-Key: partnercentral-revenue-measurement|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: partnercentral-revenue-measurement|ListTagsForResource|2xx->4xx|$headers.content-type
Issue-Key: partnercentral-revenue-measurement|ListTagsForResource|2xx->4xx|status
Issue-Key: partnercentral-revenue-measurement|UntagResource|2xx->4xx|$
Issue-Key: partnercentral-revenue-measurement|UntagResource|2xx->4xx|status
Issue-Key: partnercentral-revenue-measurement|StartRevenueAttributionAllocationsTask|4xx->4xx|$.FieldList
Issue-Key: partnercentral-revenue-measurement|StartRevenueAttributionAllocationsTask|4xx->4xx|$.Message
Issue-Key: partnercentral-revenue-measurement|StartRevenueAttributionAllocationsTask|4xx->4xx|$.Reason
Issue-Key: partnercentral-revenue-measurement|StartRevenueAttributionAllocationsTask|4xx->4xx|$.__type
Issue-Key: partnercentral-revenue-measurement|StartRevenueAttributionAllocationsTask|4xx->4xx|$headers.content-type
Issue-Key: partnercentral-revenue-measurement|StartRevenueAttributionAllocationsTask|4xx->4xx|status
Issue-Key: partnercentral-revenue-measurement|GetRevenueAttributionAllocationsTask|4xx->4xx|$.Message
Issue-Key: partnercentral-revenue-measurement|GetRevenueAttributionAllocationsTask|4xx->4xx|$.Reason
Issue-Key: partnercentral-revenue-measurement|GetRevenueAttributionAllocationsTask|4xx->4xx|$.__type
Issue-Key: partnercentral-revenue-measurement|GetRevenueAttributionAllocationsTask|4xx->4xx|$headers.content-type
Issue-Commit: 6562e94ecc647d05dc33ee3ab0dc03fac5644997

# Every rpcv2Cbor request to partnercentral-revenue-measurement is refused 404 UnknownService in awsJson, so no modeled response is served

> _Note: this issue was identified against the code base at commit `6562e94ecc647d05dc33ee3ab0dc03fac5644997`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRevenueAttribution

## Description
This issue bundles TEN net-new causes for the service `partnercentral-revenue-measurement`, all 10 divergence clusters the wire corpus reports for it. The whole corpus scored 0% (18 of 18 steps mismatched): every single request, whatever operation it names and whatever it asks for, is answered with HTTP 404, an awsJson-1.1 body `{"__type": "UnknownService", "message": "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` and `content-type: application/x-amz-json-1.1`.

SHARED FAULT (applies to every cause below): the multi-service router's service-selection step claims NO registered service for an rpcv2Cbor request addressed to this service, and then renders that refusal in the wrong protocol. Two distinct defects stack:
  (a) Selection. `MultiServiceRouter::select_service` collects `can_handle(parts)` claimants and, finding none, returns `unknown_service_response(resolve_route_key(parts))`. The message the emulator actually sent is the `key == None` arm, so the route key did not resolve at all: this service's requests are not recognized by any of the identifiers the claim test consults. The service IS registered and IS bootstrapped (crate `services/service-com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement`), so the request never reaches it. Note the identifiers that must resolve to it: Smithy `aws.api#service` gives endpointPrefix `partnercentral-prm` while `aws.auth#sigv4` gives sigv4Name `partnercentral` (these differ, and the credential scope carries the sigv4Name, not the endpointPrefix), and rpcv2Cbor's routing key is the URI path `/service/PartnerCentralRevenueMeasurement/operation/<OperationName>` together with the `smithy-protocol: rpc-v2-cbor` header — NOT `X-Amz-Target`.
  (b) Envelope. Even as a refusal, the body is wrong: `unknown_service_response` calls `aws_json_error_response(404, "UnknownService", .., AwsJsonProtocol::V1_1)` unconditionally, so an rpcv2Cbor client receives a JSON body with `content-type: application/x-amz-json-1.1` where every step of this corpus expects `content-type: application/cbor`. This service declares exactly ONE protocol, `smithy.protocols#rpcv2Cbor` (Smithy: service com.amazonaws.partnercentralrevenuemeasurement#PartnerCentralRevenueMeasurement, protocols [rpcv2Cbor], version 2022-07-26), so a JSON body is not decodable by any client of it.

Related open issue, which does NOT cover these: `20261127T161104Z-partnercentral-revenue-measurement-2-net-new-causes-the.md` is a BEHAVIORAL issue whose two causes are (1) the undecodable response body on GetRevenueAttribution and (2) the revenue-attribution read/update handlers being 501 scaffold stubs. It carries no replay key for any fixture step here (the replay reports `owned_by: []` for all ten clusters), and none of the ten operations below is one of the five it names. Fixing its Cause 1 in the router is likely to be part of the fix for the shared fault above, but each cause below additionally requires its own operation to be routed to and to answer the response recorded for it.

Because the shared fault blocks routing, NONE of the ten per-operation behaviors below has ever been exercised; each must be verified after the router is fixed, and each is its own cause to close.

=== Cause 1 of 10: CreateRevenueAttribution with ClientToken omitted must be refused 400 ValidationException with a FieldList entry naming it. ===
AWS: Smithy — CreateRevenueAttribution declares ValidationException (@error("client") @httpError(400), required Message + Reason:ValidationExceptionReason, optional FieldList:ValidationExceptionFieldList of ValidationExceptionField{required Name, Message, Code:FieldValidationCode}); ValidationExceptionReason values are [REQUEST_VALIDATION_FAILED, BUSINESS_VALIDATION_FAILED]; CreateRevenueAttributionInput.ClientToken is @idempotencyToken, @length 1..64, @pattern ^[!-~]{1,64}$. Recorded behavior for the message: HTTP 400, `__type` = `com.amazonaws.partnercentralrevenuemeasurement#ValidationException`, `Reason` = `REQUEST_VALIDATION_FAILED`, `Message` = the fixed text `Request validation failed` (no variable part for this form), and `FieldList` = one entry with `Code` = `REQUIRED_FIELD_MISSING`, `Name` = the omitted member's name as the wire spells it (here `ClientToken`) and `Message` = the fixed text `<Name> is required for mutating operations` — the member name is the only variable part. Served as `content-type: application/cbor`.
Emulator: HTTP 404, `__type` = `UnknownService`, no `Message`, no `Reason`, no `FieldList`, `content-type: application/x-amz-json-1.1`.
Why: a client that omits the idempotency token gets neither the modeled 400 nor the field-level diagnosis telling it WHICH member it must supply; it sees a 404 naming no service, which an SDK surfaces as an unmodeled error, so the retry/validation branch it would take against AWS is unreachable.
Fix: the router's service selection and refusal envelope (shared fault), then CreateRevenueAttribution's input validation and its error serializer.
Traces: partnercentral-revenue-measurement-create-revenue-attribution-validation-refusals#task#1.

=== Cause 2 of 10: CreateRevenueAttribution with an out-of-set TenancyModel must be refused 400 ValidationException naming the enum value set. ===
AWS: Smithy — CreateRevenueAttributionInput.TenancyModel is required and targets enum TenancyModel with values [MULTI_TENANT, SINGLE_TENANT]. Recorded behavior: HTTP 400, `__type` = `...#ValidationException`, `Reason` = `REQUEST_VALIDATION_FAILED`, `Message` = the constraint-violation template `<n> validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [SINGLE_TENANT, MULTI_TENANT]`. Fixed text: `validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy enum value set: [...]`. Variable parts: the count of failed constraints, the rejected value taken from the request, and the member name rendered in lowerCamel as the wire spells it (`tenancyModel`); the bracketed set is the modeled enum's values. No FieldList on this form. `content-type: application/cbor`.
Emulator: HTTP 404 `UnknownService`, no Message/Reason, JSON content-type (3 steps in the cluster).
Why: an invalid enum is silently indistinguishable from a routing failure, so a client cannot tell a bad parameter it must fix from an endpoint problem it should retry.
Fix: shared fault, then CreateRevenueAttribution's enum/constraint validation and the ValidationException message renderer.
Traces: partnercentral-revenue-measurement-create-revenue-attribution-validation-refusals#task#2 (cluster also covers task#3, task#4).

=== Cause 3 of 10: ListRevenueAttributions on an empty catalog must answer 200 with an empty RevenueAttributionSummaries list. ===
AWS: Smithy — ListRevenueAttributions is @readonly, @paginated(items: RevenueAttributionSummaries), output ListRevenueAttributionsOutput{RevenueAttributionSummaries: list, NextToken}. Recorded behavior: HTTP 200 with `RevenueAttributionSummaries` PRESENT and empty (`[]`) and no NextToken, `content-type: application/cbor`. An empty result is a success, not an error, and the list member is serialized rather than omitted.
Emulator: HTTP 404 `UnknownService`; `RevenueAttributionSummaries` missing entirely; JSON content-type.
Why: a paginating client iterating an empty catalog gets an error instead of a terminal empty page, so any list-then-act flow fails at its first call.
Fix: shared fault, then ListRevenueAttributions' handler/state read and its output serializer (the empty list must be emitted, not elided).
Traces: partnercentral-revenue-measurement-listings-and-tag-reads#task#1 (cluster also covers task#3).

=== Cause 4 of 10: ListMarketplaceRevenueShares on an empty catalog must answer 200 with an empty MarketplaceRevenueShareSummaries list. ===
AWS: Smithy — ListMarketplaceRevenueShares is @readonly, @paginated(items: MarketplaceRevenueShareSummaries); the output member MarketplaceRevenueShareSummaries is REQUIRED, so it is always present on the wire. Recorded behavior: HTTP 200, `MarketplaceRevenueShareSummaries` = `[]`, `content-type: application/cbor`.
Emulator: HTTP 404 `UnknownService`; the required member is absent; JSON content-type.
Why: the response omits a member the model marks required, so even once routing works a generated SDK can fail to deserialize it; today the client just sees a 404.
Fix: shared fault, then ListMarketplaceRevenueShares' handler and output serializer.
Traces: partnercentral-revenue-measurement-listings-and-tag-reads#task#2.

=== Cause 5 of 10: ListRevenueAttributions with MaxResults above the modeled range must be refused 400 ValidationException naming the ceiling. ===
AWS: Smithy — ListRevenueAttributionsInput.MaxResults targets Integer with @range(min: 1, max: 100). Recorded behavior: HTTP 400, `__type` = `...#ValidationException`, `Reason` = `REQUEST_VALIDATION_FAILED`, `Message` = `<n> validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>`. Fixed text is the template; variable parts are the count, the rejected value from the request, the lowerCamel member name (`maxResults`) and the modeled range maximum (100). `content-type: application/cbor`.
Emulator: HTTP 404 `UnknownService`, no Message/Reason, JSON content-type.
Why: a client probing page size gets no indication of the limit, so it cannot correct the request; AWS tells it the exact ceiling.
Fix: shared fault, then ListRevenueAttributions' range validation and the ValidationException message renderer.
Traces: partnercentral-revenue-measurement-listings-and-tag-reads#task#4.

=== Cause 6 of 10: ListTagsForResource on an untagged resource must answer 200 with an empty tags list. ===
AWS: Smithy — ListTagsForResource is @readonly; input member `resourceArn` is required (@length 1..1011, @pattern ^arn:aws:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/(revenue-attribution/[a-zA-Z0-9-]+|marketplace-revenue-share/mrs-[a-z0-9]{13})$); output member is the lowerCamel `tags`. Recorded behavior: HTTP 200 with `tags` present and empty (`[]`), `content-type: application/cbor`. An existing resource with no tags is a success with an empty list, not a ResourceNotFoundException.
Emulator: HTTP 404 `UnknownService`; `tags` missing; JSON content-type.
Why: tag read-modify-write flows break at the read, and the emulator's 404 is indistinguishable from the modeled ResourceNotFoundException for a resource that really is absent.
Fix: shared fault, then ListTagsForResource's handler/state read and output serializer.
Traces: partnercentral-revenue-measurement-listings-and-tag-reads#task#5 (cluster also covers task#7).

=== Cause 7 of 10: UntagResource must answer 200 with an empty body. ===
AWS: Smithy — UntagResource has output `Unit` (no members); required input members `resourceArn` and `tagKeys`. Recorded behavior: HTTP 200 with an EMPTY map body (`{}` decoded) and `content-type: application/cbor`. Removing tag keys that are not present is still a success.
Emulator: HTTP 404 with the body `{"__type": "UnknownService", "message": "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` where an empty map is required, and the wrong content-type.
Why: an idempotent cleanup call fails, so teardown paths in client code abort and leave state behind.
Fix: shared fault, then UntagResource's handler and its empty-output serializer.
Traces: partnercentral-revenue-measurement-listings-and-tag-reads#task#6.

=== Cause 8 of 10: StartRevenueAttributionAllocationsTask with ClientToken omitted must be refused 400 ValidationException with a field-count message and a FieldList. ===
AWS: Smithy — StartRevenueAttributionAllocationsTask is @idempotent and declares ValidationException (@httpError 400); ClientToken is @idempotencyToken, @length 1..64, @pattern ^[!-~]{1,64}$. Recorded behavior: HTTP 400, `__type` = `...#ValidationException`, `Reason` = `REQUEST_VALIDATION_FAILED`, `Message` = the fixed text `Request validation failed: <n> field error(s)` where the only variable part is the number of FieldList entries, and `FieldList` = one entry per offending member with `Code` = `REQUIRED_FIELD_MISSING`, `Name` = the member name (`ClientToken`) and `Message` = `<Name> is required for mutating operations`. Note this operation words the summary differently from CreateRevenueAttribution (Cause 1), which uses the bare `Request validation failed`; both are requirements as recorded. `content-type: application/cbor`.
Emulator: HTTP 404 `UnknownService`, no Message/Reason/FieldList, JSON content-type (3 steps in the cluster).
Why: a caller cannot learn which required member it omitted, and an allocations task that should have been refused outright returns an error class the SDK cannot branch on.
Fix: shared fault, then StartRevenueAttributionAllocationsTask's required-member validation and its error serializer.
Traces: partnercentral-revenue-measurement-start-allocations-task-validation-refusals#task#1 (cluster also covers task#2, task#6).

=== Cause 9 of 10: StartRevenueAttributionAllocationsTask with a RevenueAttributionRevision that fails the revision pattern must be refused 400 ValidationException quoting the pattern. ===
AWS: Smithy — StartRevenueAttributionAllocationsTaskInput.RevenueAttributionRevision is required and targets RevisionToken (@length 1..19, @pattern ^[1-9][0-9]*$). Recorded behavior: HTTP 400, `__type` = `...#ValidationException`, `Reason` = `REQUEST_VALIDATION_FAILED`, `Message` = `<n> validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. Fixed text is the template; variable parts are the count, the rejected value from the request, the lowerCamel member name (`revenueAttributionRevision`) and the modeled @pattern with its leading ^ and trailing $ stripped (so `[1-9][0-9]*`). `content-type: application/cbor`.
Emulator: HTTP 404 `UnknownService`, no Message/Reason, JSON content-type (3 steps in the cluster).
Why: an optimistic-concurrency caller passing a bad revision token cannot distinguish a malformed token from a stale one or from an endpoint fault, and never sees the pattern it must satisfy.
Fix: shared fault, then StartRevenueAttributionAllocationsTask's pattern validation and the ValidationException message renderer (pattern anchors stripped, member name lowerCamel).
Traces: partnercentral-revenue-measurement-start-allocations-task-validation-refusals#task#3 (cluster also covers task#4, task#5).

=== Cause 10 of 10: GetRevenueAttributionAllocationsTask for a well-formed but unknown revenue-attribution id must answer 404 ResourceNotFoundException with Message and Reason, not UnknownService. ===
AWS: Smithy — GetRevenueAttributionAllocationsTask is @readonly and declares ResourceNotFoundException (@error("client") @httpError(404), required Message + Reason:ResourceNotFoundExceptionReason whose only value is RESOURCE_NOT_FOUND); its required input member RevenueAttributionIdentifier targets RevenueAttributionIdentifier (@length 16..1011, @pattern ^(arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/revenue-attribution/ra-[a-z0-9]{13}|ra-[a-z0-9]{13})$). Recorded behavior: HTTP 404, `__type` = `com.amazonaws.partnercentralrevenuemeasurement#ResourceNotFoundException`, `Reason` = `RESOURCE_NOT_FOUND`, `Message` = the fixed text `Revenue attribution ` + the identifier value taken from the request (unquoted) + ` doesn't exist. Verify the ID and try again.` — the id is the only variable part. `content-type: application/cbor`.
Emulator: the status happens to be 404, but the body is the router refusal: `__type` = `UnknownService`, both required members (`Message`, `Reason`) absent, and `content-type: application/x-amz-json-1.1`. This is the one cluster whose status matches by coincidence, which makes it the most dangerous: a status-only check passes while the error type and both required members are wrong.
Why: a client branching on ResourceNotFoundException to decide "this attribution has no allocations task" instead receives an unmodeled error code with no Message or Reason, so it cannot distinguish a missing resource from a misconfigured endpoint; a generated SDK also cannot deserialize the modeled exception because its required members are missing.
Fix: shared fault, then GetRevenueAttributionAllocationsTask's handler lookup (return the modeled ResourceNotFoundException with Message worded as above and Reason = RESOURCE_NOT_FOUND) and its error serializer.
Traces: partnercentral-revenue-measurement-start-allocations-task-validation-refusals#task#7.

## Evidence
Replay summary (trace set simparity-wire-aws-v1): emulator_up true, bootstrap_signal false, fixtures_replayed 3, steps_total 18, steps_passed 0, steps_mismatched 18, score_pct 0.0, divergence_count 10. All 10 clusters carry service `partnercentral-revenue-measurement`, `stub_path` empty and `owned_by` empty — none is already filed.

Actual, identical for all 18 steps: HTTP 404, `content-type: application/x-amz-json-1.1`, body `{"__type": "UnknownService", "message": "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` (quoted verbatim from cluster 7's diff, which compares whole bodies).

Per-cluster expected vs actual, verbatim from the replay detail:
1. CreateRevenueAttribution, create-...-validation-refusals#task#1 — status: expected 400, got 404; $.__type expected 'com.amazonaws.partnercentralrevenuemeasurement#ValidationException', got 'UnknownService'; $.FieldList expected [{'Code': 'REQUIRED_FIELD_MISSING', 'Message': 'ClientToken is required for mutating operations', 'Name': 'ClientToken'}], got None; $.Message expected 'Request validation failed', got None; $.Reason expected 'REQUEST_VALIDATION_FAILED', got None; $headers.content-type expected 'application/cbor', got 'application/x-amz-json-1.1'.
2. CreateRevenueAttribution, create-...-validation-refusals#task#2 (+task#3, task#4) — status: expected 400, got 404; $.Message expected "1 validation error detected: Value 'SHARED_TENANT' at 'tenancyModel' failed to satisfy constraint: Member must satisfy enum value set: [SINGLE_TENANT, MULTI_TENANT]", got None; $.Reason expected 'REQUEST_VALIDATION_FAILED'; $.__type expected ValidationException, got 'UnknownService'; content-type expected 'application/cbor'.
3. ListRevenueAttributions, listings-and-tag-reads#task#1 (+task#3) — status: expected 200, got 404; $.RevenueAttributionSummaries expected [], got None; content-type expected 'application/cbor'.
4. ListMarketplaceRevenueShares, listings-and-tag-reads#task#2 — status: expected 200, got 404; $.MarketplaceRevenueShareSummaries expected [], got None; content-type expected 'application/cbor'.
5. ListRevenueAttributions, listings-and-tag-reads#task#4 — status: expected 400, got 404; $.Message expected "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100", got None; $.Reason expected 'REQUEST_VALIDATION_FAILED'; $.__type expected ValidationException.
6. ListTagsForResource, listings-and-tag-reads#task#5 (+task#7) — status: expected 200, got 404; $.tags expected [], got None; content-type expected 'application/cbor'.
7. UntagResource, listings-and-tag-reads#task#6 — status: expected 200, got 404; $ expected {}, got {'__type': 'UnknownService', 'message': 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'}.
8. StartRevenueAttributionAllocationsTask, start-allocations-task-validation-refusals#task#1 (+task#2, task#6) — status: expected 400, got 404; $.FieldList expected [{'Code': 'REQUIRED_FIELD_MISSING', 'Message': 'ClientToken is required for mutating operations', 'Name': 'ClientToken'}], got None; $.Message expected 'Request validation failed: 1 field error(s)', got None; $.Reason expected 'REQUEST_VALIDATION_FAILED'; $.__type expected ValidationException.
9. StartRevenueAttributionAllocationsTask, start-allocations-task-validation-refusals#task#3 (+task#4, task#5) — status: expected 400, got 404; $.Message expected "1 validation error detected: Value '0' at 'revenueAttributionRevision' failed to satisfy constraint: Member must satisfy regular expression pattern: [1-9][0-9]*", got None; $.Reason expected 'REQUEST_VALIDATION_FAILED'; $.__type expected ValidationException.
10. GetRevenueAttributionAllocationsTask, start-allocations-task-validation-refusals#task#7 — status 404 matches; $.__type expected 'com.amazonaws.partnercentralrevenuemeasurement#ResourceNotFoundException', got 'UnknownService'; $.Message expected "Revenue attribution ra-3ae34b60be6c3 doesn't exist. Verify the ID and try again." (the id is the request's value, the rest is fixed text), got None; $.Reason expected 'RESOURCE_NOT_FOUND', got None; content-type expected 'application/cbor'.

Model facts cited (smithy_query on services/service-com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement/smithy-model/com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement.json):
  service PartnerCentralRevenueMeasurement, version 2022-07-26, protocols [rpcv2Cbor], sdkId "PartnerCentral Revenue Measurement", endpointPrefix `partnercentral-prm`, arnNamespace `partnercentral`, sigv4Name `partnercentral`, authSchemes [sigv4, sigv4a]. Note endpointPrefix != sigv4Name.
  ValidationException: errorKind client, httpError 400, required Message + Reason (enum [REQUEST_VALIDATION_FAILED, BUSINESS_VALIDATION_FAILED]), optional FieldList of ValidationExceptionField{required Name, Message, Code}.
  ResourceNotFoundException: errorKind client, httpError 404, required Message + Reason (enum [RESOURCE_NOT_FOUND]).
  TenancyModel enum values [MULTI_TENANT, SINGLE_TENANT]; ListRevenueAttributionsInput.MaxResults @range 1..100; RevisionToken @length 1..19 @pattern ^[1-9][0-9]*$; RevenueAttributionIdentifier @length 16..1011 @pattern ^(arn:[a-z-]+:partnercentral:[a-z0-9-]+:[0-9]{12}:catalog/[a-zA-Z]+/revenue-attribution/ra-[a-z0-9]{13}|ra-[a-z0-9]{13})$; ListMarketplaceRevenueSharesOutput.MarketplaceRevenueShareSummaries is required; UntagResource output is Unit; ListTagsForResource output member is `tags`.
  Every operation above declares ValidationException and ResourceNotFoundException among its errors; all @http bindings are inert (rpcv2Cbor dispatches by the rpc-v2-cbor URI path, not by @http).

Router code evidence for the shared fault (vendor/smithy-server-runtime/src/multi_service_router.rs):
  lines 296-336 `select_service` — claimants are `services.filter(|s| s.can_handle(parts))`; the empty arm returns `unknown_service_response(emulator_server::resolve_route_key(parts).as_deref())`.
  lines 483-491 `unknown_service_response` — the message the emulator sent is the `None` arm, i.e. `resolve_route_key` resolved no key at all; the body is rendered with `aws_json_error_response(404, "UnknownService", &message, AwsJsonProtocol::V1_1)` regardless of the targeted service's protocol.
  lines 496-509 `ambiguous_service_response` has the same unconditional awsJson rendering.
  Contrast the protocol-correct paths in the same dispatch: lines 199-205 (routing_error_response), 246-257 (routing_error_response), 261-268 (definition().refusal_response).

## Steps to Reproduce
1. Clone this branch and build/boot the emulator, then replay the wire corpus `simparity-wire-aws-v1`. Observe score_pct 0.0 with all 18 steps of the three partnercentral-revenue-measurement fixtures mismatched, every one answered 404 `UnknownService` with `content-type: application/x-amz-json-1.1`.
2. Reproduce the shared fault with a single request (the SDK is unusable here because the body is not CBOR):
   curl -sS -D- -X POST "<emulator endpoint>/service/PartnerCentralRevenueMeasurement/operation/ListRevenueAttributions" -H 'smithy-protocol: rpc-v2-cbor' -H 'content-type: application/cbor' -H 'accept: application/cbor' -H 'authorization: AWS4-HMAC-SHA256 Credential=<key>/<date>/us-east-1/partnercentral/aws4_request, SignedHeaders=..., Signature=...' --data-binary @<cbor body for {"Catalog":"Sandbox"}>
   Expected: HTTP 200, `content-type: application/cbor`, CBOR map containing `RevenueAttributionSummaries: []`. Observed: HTTP 404, `content-type: application/x-amz-json-1.1`, `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`.
3. Fix the shared fault: make the service claim its own requests (the rpc-v2-cbor URI path `/service/PartnerCentralRevenueMeasurement/operation/<Op>` plus `smithy-protocol: rpc-v2-cbor`, and the SigV4 credential scope `partnercentral` which is the sigv4Name, not the endpointPrefix `partnercentral-prm`), and render `unknown_service_response`/`ambiguous_service_response` in the request's protocol instead of awsJson1.1. Re-run step 2 and confirm the response is decodable CBOR.
4. Then close each of the ten causes by driving its operation and comparing against the recorded response: CreateRevenueAttribution without ClientToken; CreateRevenueAttribution with TenancyModel='SHARED_TENANT'; ListRevenueAttributions on an empty catalog; ListMarketplaceRevenueShares on an empty catalog; ListRevenueAttributions with MaxResults=101; ListTagsForResource on an untagged resource ARN; UntagResource; StartRevenueAttributionAllocationsTask without ClientToken; StartRevenueAttributionAllocationsTask with RevenueAttributionRevision='0'; GetRevenueAttributionAllocationsTask on a well-formed unknown `ra-<13 chars>` id.
5. Re-replay all three fixtures to confirm all 18 steps pass: partnercentral-revenue-measurement-create-revenue-attribution-validation-refusals (task#1-task#4), partnercentral-revenue-measurement-listings-and-tag-reads (task#1-task#7) and partnercentral-revenue-measurement-start-allocations-task-validation-refusals (task#1-task#7).
