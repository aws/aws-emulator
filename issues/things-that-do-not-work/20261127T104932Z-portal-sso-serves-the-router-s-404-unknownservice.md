Issue-Source: trace-sweep-divergence
Issue-Trace: sso-portal-input-validation-before-token
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-portal-input-validation-before-token
Issue-Shape-Id: com.amazonaws.sso#SWBPortalService
Issue-Key: portal.sso|ListAccounts|4xx->4xx|$.__type
Issue-Key: portal.sso|ListAccounts|4xx->4xx|$.message
Issue-Key: portal.sso|ListAccounts|4xx->4xx|$headers.content-type
Issue-Key: portal.sso|GetRoleCredentials|4xx->4xx|$.__type
Issue-Key: portal.sso|GetRoleCredentials|4xx->4xx|$.message
Issue-Key: portal.sso|GetRoleCredentials|4xx->4xx|$headers.content-type
Issue-Commit: e0dbb6af3802ab132c756881984b5362646a383d

# portal.sso serves the router's 404 UnknownService envelope instead of the portal's own 404 ResourceNotFoundException for a path the model does not bind (2 causes)

> _Note: this issue was identified against the code base at commit `e0dbb6af3802ab132c756881984b5362646a383d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccounts

## Description
The SSO portal data plane (sdkId `SSO`, endpoint prefix `portal.sso`, protocol `restJson1`) does not own requests that fall under its own path namespace but match none of its four `@http` bindings. Such a request is never attributed to the service at all, so the multi-service router answers it with its own cross-service "no target service" envelope instead of the portal's modelled not-found error. Two causes below, one per path family the corpus exercises; both are the same missing behavior and both must be closed.

Cause 1 of 2 — unbound path under /assignment (ListAccounts family). Fixture `sso-portal-input-validation-before-token`, step `task#2`: `GET /assignment/accountz` (an unbound sibling of the modelled `GET /assignment/accounts`).
AWS: 404 with a restJson1 error document. Smithy: `SWBPortalService` is `restJson1`; `ListAccounts` is bound to `GET /assignment/accounts` and declares `ResourceNotFoundException`, whose shape `com.amazonaws.sso#ResourceNotFoundException` carries `@error("client")`, `@httpError(404)` and a single `message` member documented as "The specified resource doesn't exist." The recorded response body is `{"message": "Could not find resource for full path: <request URL>", "__type": "com.amazonaws.switchboard.portal#ResourceNotFoundException"}` with `content-type: application/json`. Message template: the fixed text is `Could not find resource for full path: ` and the one variable part is the request's own full URL (scheme + the host the request was addressed to + the unmatched path, query included if present) — in the recording that value happened to be `https://portal.sso.us-west-2.amazonaws.com/assignment/accountz`, which is this environment's host and NOT part of the requirement; the emulator must echo the URL of the request it actually received. The `__type` is the namespace-qualified id of the same not-found shape (recorded as `com.amazonaws.switchboard.portal#ResourceNotFoundException`; SDKs resolve on the `ResourceNotFoundException` name after the `#`).
Emulator: 404, but with the router's cross-service envelope — `__type` = `UnknownService`, `message` = `could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)`, and `content-type: application/x-amz-json-1.1`. The status coincides; the error code, the `__type`, the message and the content type all diverge.
Why: a restJson1 SDK/CLI pointed at the emulator cannot resolve `UnknownService` onto any error the portal models, and the `application/x-amz-json-1.1` content type is not the protocol the client is parsing, so a simple client-side path or API-version mistake surfaces as an unparseable/unknown transport error instead of the modelled `ResourceNotFoundException` that client error handling is written against. Any client that distinguishes "wrong path" from "wrong service" gets the wrong answer.
Fix: the service's request-attribution seam and its routing-error seam — `can_handle` must claim a request that addresses this service's path namespace even when the endpoint host names no service (today the `_ => serves_route(parts)` arm requires an EXACT match against the four literal `ROUTES` rows, so an unbound path is left unclaimed), and `override_routing_error` must render the modelled 404 `ResourceNotFoundException` with the "Could not find resource for full path: <url>" message for a claimed request that matches no route (today it returns `None` for any path not in `ROUTES`, letting the router's `UnknownService` stand).
Traces: `sso-portal-input-validation-before-token#task#2`.

Cause 2 of 2 — unbound path under /federation (GetRoleCredentials family). Fixture `sso-portal-input-validation-before-token`, step `task#3`: `GET /federation/credentialz` (an unbound sibling of the modelled `GET /federation/credentials`).
AWS: identical error class and envelope as cause 1 — 404, `content-type: application/json`, body `{"message": "Could not find resource for full path: <request URL>", "__type": "com.amazonaws.switchboard.portal#ResourceNotFoundException"}`. Smithy: `GetRoleCredentials` is bound to `GET /federation/credentials` and declares the same `ResourceNotFoundException` (`@httpError(404)`). Same message template: fixed text `Could not find resource for full path: ` plus the request's own full URL as the variable part (the recording's `https://portal.sso.us-west-2.amazonaws.com/federation/credentialz` is a per-environment value, not a requirement).
Emulator: 404 with `__type` = `UnknownService`, message `could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)`, `content-type: application/x-amz-json-1.1`.
Why: same as cause 1 — this is the credential-vending path family, so a client that mistypes or uses an older spelling of the federation path cannot tell a not-found resource from "this emulator serves no such service", and must parse a protocol envelope the portal never speaks.
Fix: same two seams as cause 1; the behavior is per-path-namespace, not per-operation, so one not-found renderer must cover every unbound path the portal claims (both `/assignment/*` and `/federation/*`, and any other path the request addresses this service with).
Traces: `sso-portal-input-validation-before-token#task#3`.

Related but NOT covering this: open issue `20261030T212213Z-sso-listaccounts-wrong-behavior.md` reports that the same hand-rolled exact `ROUTES` match mis-handles SLASH-PADDED spellings of paths the model DOES bind, and it explicitly asserts the opposite design for this case ("a path this model does not bind matches no route (so it stays the router's own `UnknownService`)", `src/service.rs:221-224`). The recordings show that assertion is itself the divergence: AWS answers an unbound portal path with the portal's own 404 `ResourceNotFoundException`, not with a cross-service routing error. Fixing the slash-padding issue alone does not change either step here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (24 steps, 22 passed, 2 mismatched) reported both steps as `verdict: mismatch`, `stub_path: ""`, `owned_by: []`.

Cause 1 — fixture `sso-portal-input-validation-before-token`, step `task#2`, operation ListAccounts, service portal.sso, expected_status 404 / actual_status 404:
  $.message: expected 'Could not find resource for full path: https://portal.sso.us-west-2.amazonaws.com/assignment/accountz', got 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'
  $.__type: expected 'com.amazonaws.switchboard.portal#ResourceNotFoundException', got 'UnknownService'
  $headers.content-type: expected 'application/json', got 'application/x-amz-json-1.1'

Cause 2 — fixture `sso-portal-input-validation-before-token`, step `task#3`, operation GetRoleCredentials, service portal.sso, expected_status 404 / actual_status 404:
  $.message: expected 'Could not find resource for full path: https://portal.sso.us-west-2.amazonaws.com/federation/credentialz', got 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'
  $.__type: expected 'com.amazonaws.switchboard.portal#ResourceNotFoundException', got 'UnknownService'
  $headers.content-type: expected 'application/json', got 'application/x-amz-json-1.1'

Model evidence (services/service-com-amazonaws-sso-swbportalservice/smithy-model/com-amazonaws-sso-swbportalservice.json): service SWBPortalService, sdkId SSO, protocols [restJson1]; ListAccounts GET /assignment/accounts errors [ResourceNotFoundException]; GetRoleCredentials GET /federation/credentials errors [ResourceNotFoundException]; shape com.amazonaws.sso#ResourceNotFoundException { errorKind: client, httpError: 404, members: { message: ErrorDescription } }.

Code evidence: services/service-com-amazonaws-sso-swbportalservice/src/service.rs:225-230 (`matched_route` matches the four literal `ROUTES` rows exactly), :259-261 (`serves_route`), :368-372 (`can_handle`'s fallback `_ => serves_route(parts)`, which declines an unbound path when the endpoint host names no service), :464 (`override_routing_error` returns `None` via `absent_required_members(req)?` for any path not in `ROUTES`, so the router's `UnknownService` stands). Expected content type is the protocol's own: restJson1 renders errors as `application/json`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `sso-portal-input-validation-before-token`.
2. Cause 1 (step `task#2`): send `GET /assignment/accountz` to the emulator's data listener addressed as the SSO portal (no SigV4 credential scope and no `X-Amz-Target`, as every portal operation is `@optionalAuth` with empty `@auth`). Observed: 404 with `content-type: application/x-amz-json-1.1` and body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`. Expected: 404 with `content-type: application/json` and body `{"__type":"com.amazonaws.switchboard.portal#ResourceNotFoundException","message":"Could not find resource for full path: <the full URL of the request as received>"}`.
3. Cause 2 (step `task#3`): send `GET /federation/credentialz` the same way. Observed: the same `UnknownService` / `application/x-amz-json-1.1` envelope. Expected: the same 404 `ResourceNotFoundException` / `application/json` envelope, with the request's own full URL in the message.
4. Confirm the modelled routes are unaffected: `GET /assignment/accounts` and `GET /federation/credentials` with a usable `x-amz-sso_bearer_token` still answer their modelled results (the other 22 steps of this corpus pass).
5. Re-replay both `sso-portal-input-validation-before-token#task#2` and `sso-portal-input-validation-before-token#task#3` after the fix; closing only one leaves the other path family diverging.
