Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance license-manager-user-subscriptions-deregister-identity-provider-not-found license-manager-user-subscriptions-license-server-endpoint-lifecycle-errors license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp license-manager-user-subscriptions-update-identity-provider-settings-unregistered
Issue-Shape-Id: com.amazonaws.licensemanagerusersubscriptions#LicenseManagerUserSubscriptions
Issue-Key: license-manager-user-subscriptions|AssociateUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DisassociateUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DeregisterIdentityProvider|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DeleteLicenseServerEndpoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|ListUserAssociations|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|ListProductSubscriptions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|StartProductSubscription|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|StopProductSubscription|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|UpdateIdentityProviderSettings|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: bdcf8cd5a68bd8230d17264742078b45c7947bd8

# UnauthorizedException refusals emit x-amzn-errortype once instead of the recorded repeated header

> _Note: this issue was identified against the code base at commit `bdcf8cd5a68bd8230d17264742078b45c7947bd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateUser

## Description
SHARED FAULT (9 causes, one per gated operation): every fail-closed `UnauthorizedException` refusal this service answers leaves with the `x-amzn-errortype` error-type header present ONCE, where the recorded wire for the same refusal carries that header REPEATED (the replay harness joins repeated header values, so the recorded value reads 'UnauthorizedException, UnauthorizedException', while clusters whose fixture asserts the header's shape rather than a literal report it as shape_mismatch '<shaped-not-asserted>' vs the emulator's single scalar 'UnauthorizedException'). Status (401), wire code (`UnauthorizedException`) and the body sentence all MATCH on every one of these steps; only the error-type header set diverges, so this is one envelope defect in the refusal renderer, reproduced on nine operations.

AWS: recorded behavior only for the header multiplicity. The service model (Smithy: LicenseManagerUserSubscriptions, protocol restJson1, authSchemes sigv4; smithy_query digest) declares NO `UnauthorizedException` shape on any operation — AssociateUser/DisassociateUser/ListUserAssociations/ListProductSubscriptions/StartProductSubscription/StopProductSubscription/DeregisterIdentityProvider/DeleteLicenseServerEndpoint list only AccessDeniedException, ConflictException, ResourceNotFoundException, ServiceQuotaExceededException, ValidationException, ThrottlingException, plus the service-common InternalServerException, and UpdateIdentityProviderSettings only AccessDeniedException/ThrottlingException/ValidationException. So nothing in the model constrains this refusal and the recording is the only authority: HTTP 401, error code `UnauthorizedException`, the `x-amzn-errortype` header carried as a REPEATED header value, and a message whose text is entirely fixed ('Unauthorized. Please check the request and access policy and try again.') with NO variable parts taken from the request or state — a fail-closed refusal discloses nothing about the identifier it was asked about.

Emulator: answers the same 401 / `UnauthorizedException` / same sentence, but writes the `x-amzn-errortype` header exactly once. The single occurrence is deliberate policy in this service, not an accident: services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/service/refusal_headers.rs states in its module doc (lines 6-17) that every refusal 'leaves with exactly one occurrence of it: a second occurrence would fold, in any client that joins repeated header values, into a value naming no error shape at all'. That reasoning contradicts the recorded wire, which is exactly what a client joining repeated values sees from AWS.

Why: on restJson1 the error-type header is how a client resolves the failure's error SHAPE. A client, SDK error-classifier, proxy or contract test that reads the full (joined) `x-amzn-errortype` value, or that asserts the header set of an AWS error response, gets a different value from the emulator than from AWS for the same refusal, so authorization-error handling that is exercised against the emulator is not the handling AWS exercises. Any response-normalizing middleware that de-duplicates or splits that header is likewise never exercised.

Fix: the service's refusal envelope renderer — `refusal_response` in services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/service/refusal_headers.rs, which serializes `crate::scaffolded::ServiceRefusal` (scaffolded.rs: UNAUTHORIZED_EXCEPTION_CODE/_STATUS/_MESSAGE) through `definition().protocol().serialize_error(..)`, plus that module's single-occurrence policy. The fail-closed refusal must leave with the error-type header set the recording shows for it; the status, code and sentence it already carries must not change. Both the IAM gate and the instance gate route through this one renderer, so fixing it there covers all nine causes.

Cause 1 of 9: AssociateUser — fixture license-manager-user-subscriptions-associate-disassociate-user-unknown-instance, step task#1. Expected 401 with `x-amzn-errortype` joined as 'UnauthorizedException, UnauthorizedException'; got 401 with 'UnauthorizedException'.

Cause 2 of 9: DisassociateUser — fixture license-manager-user-subscriptions-associate-disassociate-user-unknown-instance, step task#2 (cluster also covers task#3 of the same fixture). Same expected/actual header divergence at 401.

Cause 3 of 9: DeregisterIdentityProvider — fixture license-manager-user-subscriptions-deregister-identity-provider-not-found, step task#2. Reported as shape_mismatch: expected '<shaped-not-asserted>' (the recorded repeated-value header), got the single scalar 'UnauthorizedException'; status 401 on both sides.

Cause 4 of 9: DeleteLicenseServerEndpoint — fixture license-manager-user-subscriptions-license-server-endpoint-lifecycle-errors, step task#5. Reported as shape_mismatch: expected '<shaped-not-asserted>', got 'UnauthorizedException'; status 401 on both sides.

Cause 5 of 9: ListUserAssociations — fixture license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp, step task#1. Expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException'.

Cause 6 of 9: ListProductSubscriptions — fixture license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp, step task#2. Expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException'.

Cause 7 of 9: StartProductSubscription — fixture license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp, step task#1. Expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException'.

Cause 8 of 9: StopProductSubscription — fixture license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp, step task#2. Expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException'.

Cause 9 of 9: UpdateIdentityProviderSettings — fixture license-manager-user-subscriptions-update-identity-provider-settings-unregistered, step task#1. Expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException'.

No open issue covers this service; list_existing_issues(query='license-manager-user-subscriptions') returned none. All nine causes must be closed (the renderer fix closes them together) and every listed fixture#step re-replayed before submitting.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 37 steps, 26 passed, 11 mismatched. Nine of the eleven mismatches are this service's 401 refusal header.

Expected (recorded AWS wire), every cause:
  HTTP/1.1 401
  x-amzn-errortype: UnauthorizedException   (header carried REPEATED; harness joins to 'UnauthorizedException, UnauthorizedException')
  body: {"__type":"UnauthorizedException","message":"Unauthorized. Please check the request and access policy and try again."}

Actual (emulator), every cause:
  HTTP/1.1 401
  x-amzn-errortype: UnauthorizedException   (ONE occurrence)
  same code, same message

Per-cluster detail strings as returned by the replay:
  AssociateUser (associate-disassociate-user-unknown-instance#task#1): "$headers.x-amzn-errortype: value_mismatch (expected 'UnauthorizedException, UnauthorizedException', got 'UnauthorizedException')"
  DisassociateUser (same fixture#task#2, affected also task#3): same value_mismatch
  DeregisterIdentityProvider (deregister-identity-provider-not-found#task#2): "$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'UnauthorizedException')"
  DeleteLicenseServerEndpoint (license-server-endpoint-lifecycle-errors#task#5): same shape_mismatch
  ListUserAssociations / ListProductSubscriptions (list-associations-and-subscriptions-unregistered-idp#task#1, #task#2): same value_mismatch
  StartProductSubscription / StopProductSubscription (start-stop-product-subscription-unregistered-idp#task#1, #task#2): same value_mismatch
  UpdateIdentityProviderSettings (update-identity-provider-settings-unregistered#task#1): same value_mismatch

Message-text accounting: 'Unauthorized. Please check the request and access policy and try again.' is entirely FIXED text — no part of it is interpolated from the request or from emulator state — and it matched on every step, as did the 401 status and the `UnauthorizedException` code. Only the header occurrence count diverges. No per-run value (request id, timestamp, generated id) is involved in any of these assertions.

Emulator source stating the diverging policy: services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/service/refusal_headers.rs lines 6-17 and 29-42 (one occurrence of x-amzn-errortype per refusal); the refusal values come from src/scaffolded.rs lines 180-206 (UNAUTHORIZED_EXCEPTION_CODE/_MESSAGE/_STATUS) and 308-317 (ServiceRefusal::unauthorized).

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm /__admin/health answers 200.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the nine license-manager-user-subscriptions clusters.
3. Or by hand, for any gated operation of this service, send a signed restJson1 request that the authorization/instance gate refuses, e.g.:
   POST /user/AssociateUser  (Content-Type: application/json) with a body naming an EC2 instance the calling account does not own — the gate in src/service/authorization.rs refuses it fail-closed.
   Repeat for /user/DisassociateUser, /user/ListUserAssociations, /user/ListProductSubscriptions, /user/StartProductSubscription, /user/StopProductSubscription, /identity-provider/DeregisterIdentityProvider, /identity-provider/UpdateIdentityProviderSettings and /license-server/DeleteLicenseServerEndpoint.
4. Dump the RAW response headers (curl -i / curl -D -, not a client that collapses duplicates) and count the occurrences of `x-amzn-errortype`.
5. Observe exactly one occurrence; the recorded wire for the same refusal carries it repeated, which a header-joining client reads as 'UnauthorizedException, UnauthorizedException'.
6. After fixing, re-replay the representative fixture step (associate-disassociate-user-unknown-instance#task#1) AND every fixture#step listed in trace_ids.
