Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-identity.register-endpoint-invalid-pinpoint-arn-inactive
Issue-Shape-Id: com.amazonaws.chimesdkidentity#ChimeIdentityService
Issue-Key: chime-sdk-identity.register-endpoint-invalid-pinpoint-arn-inactive|run#15|AssertionError|raise AssertionError(
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# RegisterAppInstanceUserEndpoint never resolves the Pinpoint application its ResourceArn names, so a ResourceArn naming an app that does not exist registers an endpoint instead of being refused 403 ForbiddenException

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterAppInstanceUserEndpoint

## Description
One net-new root cause.

Cause 1 of 1: `RegisterAppInstanceUserEndpoint` decides what to do with `ResourceArn` purely from the ARN's SYNTAX and never asks whether the Amazon Pinpoint application that ARN names actually exists for the caller. A register whose `ResourceArn` is correctly scoped (right partition, right `mobiletargeting` vendor, right region, the caller's own account id, the `apps/<id>` resource space) but names an application that was never created is therefore ACCEPTED and an endpoint is minted, where AWS refuses the call outright and creates nothing. Failing check: chime-sdk-identity.register-endpoint-invalid-pinpoint-arn-inactive.

AWS: the register resolves the Pinpoint application named by `ResourceArn` against the caller's account before it registers anything, and a `ResourceArn` it cannot resolve to an application the caller may use is refused, not downgraded. Smithy (services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/smithy-model/com-amazonaws-chimesdkidentity-chimeidentityservice.json): `RegisterAppInstanceUserEndpoint` POST /app-instance-users/{AppInstanceUserArn}/endpoints, success code 201, with `ForbiddenException` @httpError 403 among its modelled errors; `ResourceArn` targets `ChimeArn` (@length 5..1600, @pattern ^arn:...$) and the model constrains only that ARN SHAPE, never which application it may name. The refusal itself is recorded behavior only: the recording shows HTTP 403, error code `ForbiddenException`, message `You are not authorized to perform this action.` — that whole sentence is FIXED text with no variable part (no ARN, id, account or member name is interpolated into it) — and an independent `ListAppInstanceUserEndpoints` read afterwards shows the refused register materialized no endpoint. The same call shape against an application that DOES exist succeeds and the endpoint lands `EndpointState.Status == "ACTIVE"`, so the only thing that changes the answer is whether the application resolves.

Emulator: the handler calls `crate::state::endpoint_state_of(&input.resource_arn)` and then unconditionally registers. `endpoint_state_of` returns ACTIVE when the ARN merely LOOKS like a Pinpoint app ARN (vendor `mobiletargeting`, resource type `apps`, id exactly 32 lowercase-hex characters) and otherwise returns `{Status: INACTIVE, StatusReason: INVALID_PINPOINT_ARN}`; neither branch can refuse, and neither branch reads any Pinpoint application state. Replayed: a `ResourceArn` of the form arn:aws:mobiletargeting:<region>:<caller-account>:apps/<id-never-created> returned HTTP 201 with a populated `AppInstanceUserArn` and a freshly minted `EndpointId` instead of 403 `ForbiddenException`, and the endpoint persisted under the user. Because the test is purely lexical, the converse hole is open as well — Hypothesis: a correctly scoped `apps/<32 lowercase-hex>` ARN that names no application at all registers an ACTIVE endpoint, since nothing distinguishes it from the real application the positive control created (the check only exercised the non-32-character form, so only that leg is replay-grounded).

Why: a client that mistypes or stale-copies a Pinpoint application ARN is told its endpoint was created. It gets a 201 and an `EndpointId` it can describe and list, so it never discovers the misconfiguration; against AWS the call is refused and the client's error path (fix the ARN / fix the permission) runs. Worse, the effect is materialized: the emulator leaves a durable endpoint behind for a call AWS treats as unauthorized, so cleanup, quota accounting (`MAX_APP_INSTANCE_USER_ENDPOINTS`) and any later listing all diverge from the account state AWS would hold. This is the "overly-permissive" class: an authorization decision the service is supposed to make is simply absent.

Fix: the `ResourceArn` resolution step of this operation's handler, plus the state function it delegates to. It must become a real lookup of the Pinpoint application in the caller's account (the register runs under the same emulator that serves Pinpoint's `CreateApplication`/`DeleteApp`, so the application registry is available to it) rather than a shape test, and an unresolvable application must return the modelled `ForbiddenException` with code `ForbiddenException`/403 and the fixed sentence above, before any record is inserted, so nothing is written. Note the crate reached this state deliberately: the resolved issue 20261120T110326Z-chime-sdk-identity-registerappinstanceuserendpoint-wrong-behavior.md replaced the previous 403 for an `apps/<non-32-char>` id with an INACTIVE/INVALID_PINPOINT_ARN registration, and the open issue 20261125T041927Z-chime-sdk-identity-registerappinstanceuserendpoint-error-response.md (member-resolution ORDER) describes the code as it was before that change. Neither covers this cause: the recorded AWS answer for a correctly scoped ARN naming an application that does not exist is a 403 refusal with no endpoint, so `INVALID_PINPOINT_ARN` INACTIVE registration belongs to `ResourceArn`s that are not a Pinpoint application ARN at all, and the deciding input is application EXISTENCE, not id length. Whichever way the two legs are split, the existence lookup is the missing piece, and the tests that pin the current split (tests/all/user_endpoint_registration.rs) will need to move with it.

Traces: chime-sdk-identity.register-endpoint-invalid-pinpoint-arn-inactive.

## Evidence
Failing check chime-sdk-identity.register-endpoint-invalid-pinpoint-arn-inactive (group overly-permissive, axis:rejection, plane:control).

Expected (real AWS, per the recording the check encodes): RegisterAppInstanceUserEndpoint with a correctly scoped ResourceArn naming a Pinpoint application that does not exist is refused with HTTP 403, Error.Code == "ForbiddenException", Error.Message == "You are not authorized to perform this action." (entire sentence is fixed text; nothing from the request is interpolated), and a follow-up ListAppInstanceUserEndpoints on the same AppInstanceUserArn returns only the endpoint the positive control created.

Actual (emulator): the call SUCCEEDED. Replay output:
  AssertionError: real AWS refuses this call, but it SUCCEEDED and returned
  {'ResponseMetadata': {'HTTPStatusCode': 201, ...},
   'AppInstanceUserArn': 'arn:aws:chime:us-east-1:<account>:app-instance/<instance-id>/user/<user-id>',
   'EndpointId': '<minted-uuid>'}
(the ARN, account, ids and timestamps above are this run's values, not requirements — the requirement is 201-with-an-EndpointId where AWS answers 403.)

Positive control in the same check passed: against an application created via pinpoint CreateApplication, the identical call shape registered an endpoint whose EndpointState.Status == "ACTIVE". Only one thing differed between the two calls — whether the application the ResourceArn names exists.

Emulator code the behavior comes from:
- services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/register_app_instance_user_endpoint.rs:220-229 — comment states the design ("the `ResourceArn` decides the state the endpoint is registered in, and never whether it is registered at all") and line 229 `let endpoint_state = crate::state::endpoint_state_of(&input.resource_arn);`. There is no refusal branch for ResourceArn anywhere in the handler; lines 233-299 build the record and insert it, and the only refusals left are user-absent / deleted-instance / at-capacity.
- services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/state.rs:967-997 — `endpoint_state_of` returns ACTIVE if `names_pinpoint_application(resource_arn)` else INACTIVE + "INVALID_PINPOINT_ARN"; src/state.rs:963-965 `PINPOINT_APPLICATION_ID_LENGTH: usize = 32` and src/state.rs:999-1005 document the test as purely lexical (vendor `mobiletargeting`, resource_type `apps`, id an id "Pinpoint mints" by shape). No Pinpoint application state is consulted.

Smithy: RegisterAppInstanceUserEndpoint errors include ForbiddenException (errorKind client, httpError 403); http {method: POST, uri: /app-instance-users/{AppInstanceUserArn}/endpoints, code: 201}; ResourceArn -> ChimeArn, required, @length 5..1600, @pattern ^arn:[a-z0-9-\.]{1,63}:... — i.e. the model bounds the ARN's shape only.

## Steps to Reproduce
1. CreateAppInstance {"Name":"i","ClientRequestToken":"t1"} -> 201, AppInstanceArn A.
2. CreateAppInstanceUser {"AppInstanceArn":A,"AppInstanceUserId":"owner","Name":"endpoint-owner","ClientRequestToken":"t2"} -> 201, AppInstanceUserArn U.
3. Against the same emulator's Pinpoint service, CreateApp {"CreateApplicationRequest":{"Name":"pp"}} -> ApplicationResponse.Id = P (positive control).
4. POST /app-instance-users/{U}/endpoints with {"ClientRequestToken":"t3","Type":"GCM","ResourceArn":"arn:aws:mobiletargeting:us-east-1:<caller-account>:apps/<P>","EndpointAttributes":{"DeviceToken":"d"}} -> 201; DescribeAppInstanceUserEndpoint reports EndpointState.Status "ACTIVE". (Passes today; must keep passing.)
5. Break exactly one thing — same account, same region, same call shape, an application id that was never created: POST /app-instance-users/{U}/endpoints with {"ClientRequestToken":"t4","Type":"GCM","ResourceArn":"arn:aws:mobiletargeting:us-east-1:<caller-account>:apps/<id-never-created>","EndpointAttributes":{"DeviceToken":"d"}}.
6. Observed: HTTP 201 with AppInstanceUserArn + a minted EndpointId.
   Expected: HTTP 403, Error.Code "ForbiddenException", Error.Message "You are not authorized to perform this action." (fixed text, no interpolation).
7. ListAppInstanceUserEndpoints {"AppInstanceUserArn":U}. Observed: two endpoints (the step-4 control plus the one step 5 should never have created). Expected: exactly one, the step-4 control's EndpointId.
8. Repeat step 5 with an id of exactly 32 lowercase-hex characters that was never created to see the converse of the same missing lookup: the register is accepted and the endpoint lands ACTIVE (not replay-grounded; hypothesis from the lexical-only test at src/state.rs:967-997).
