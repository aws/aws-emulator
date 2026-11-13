Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner appfabric.delete-app-bundle-with-contents-rejected
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner|run#15|AssertionError|assert message == refusals[code], losers
Issue-Key: appfabric.delete-app-bundle-with-contents-rejected|run#12|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: 2d2373ac96719fb51b225d122133a9d558f5af51

# AppFabric duplicate-CreateAppAuthorization conflict message is the emulator's own wording, and DeleteAppBundle on a non-empty bundle still refuses as ConflictException (2 causes)

> _Note: this issue was identified against the code base at commit `2d2373ac96719fb51b225d122133a9d558f5af51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppAuthorization, DeleteAppBundle

## Description
Two independent root causes in the AppFabric crate, both found by replaying the behavioral suite against the booted emulator (appfabric 17/23 pass; these 2 checks are the ones the replay report did not already mark as owned/filed). A fixer must close BOTH before submitting.

Related open issues (named here for context, neither covers these two failing checks: 20261029T192800Z-...-connect-never-validates-credentials... lists appfabric.delete-app-bundle-with-contents-rejected but its replay key no longer matches the check as it now fails, and the wire issue 20261029T200017Z-appfabric-refusal-paths-use-the-wrong-error-class carries only wire fixture keys and pins a different duplicate-create wording for the SEQUENTIAL duplicate, not the concurrent-race refusal of cause 1): 20261029T192800Z-appfabric-connect-never-validates-credentials-app-authorization-not.md, 20261029T200017Z-appfabric-refusal-paths-use-the-wrong-error-class.md. Close them together with this one.

--------------------------------------------------
Cause 1 of 2: the losers of a concurrent duplicate CreateAppAuthorization get the emulator's own conflict message instead of AWS's
--------------------------------------------------
AWS: when N callers race to create an app authorization for the SAME (app bundle, app, authType, tenantIdentifier), exactly one succeeds and every other caller is refused with one of exactly two refusals: HTTP 409 ConflictException whose message is the fixed template "Request to create AppAuthorization for App <app> with authType <authType> and tenantIdentifier <tenantIdentifier> is already in progress." — the fixed text is "Request to create AppAuthorization for App ", " with authType ", " and tenantIdentifier ", " is already in progress.", and the three interpolated parts are the request's app, authType and tenant.tenantIdentifier values (never a literal from one run) — or HTTP 400 ValidationException with the wholly fixed text "ClientId is already being used for this AppBundle and App." (no interpolation). Smithy: CreateAppAuthorization (POST /appbundles/{appBundleIdentifier}/appauthorizations, success 201, @idempotent) models both ConflictException (httpError 409; required message/resourceId/resourceType) and ValidationException (httpError 400; required message/reason), so the model permits either shape and recorded behavior is what selects the wording. The message text itself is recorded behavior only.
Emulator: single-winner semantics are correct (1 winner, 4 losers, exactly one authorization persisted and listed), but all four losers are refused ConflictException with the message "An app authorization already exists for this application and tenant" — the emulator's own fixed text, with no app/authType/tenantIdentifier interpolated and no trailing period. It is the shared duplicate-authorization constructor (state.rs app_authorization_already_exists) raised by the pre-insert duplicate guard in CreateAppAuthorization's handler.
Why: for AppFabric the message is part of the error contract. A client that distinguishes "another create for this tenant is in progress" (retry/wait, which AWS's 409 wording states) from "this clientId is already used" (fix the request) cannot tell them apart, and any parity test or operator surface matching AWS's documented string diverges even though the code and single-winner outcome are right.
Fix: CreateAppAuthorization's duplicate guard and the shared duplicate-authorization error constructor in the appfabric crate's state module — emit AWS's templated conflict text (app/authType/tenantIdentifier taken from the request) for the in-progress/duplicate refusal, and make sure the refusal picked for the concurrent-race path matches one of AWS's two recorded refusals exactly.
Traces: appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner (AssertionError on `message == refusals[code]`; all four losers reported ('ConflictException', 'An app authorization already exists for this application and tenant')).

--------------------------------------------------
Cause 2 of 2: DeleteAppBundle on a bundle that still holds an app authorization is refused ConflictException (409) instead of ValidationException (400)
--------------------------------------------------
AWS: DeleteAppBundle against a bundle that still contains an app authorization is refused HTTP 400 ValidationException with the message "The specified app bundle cannot be deleted." — the whole string is fixed text, with no request or state values interpolated — and the call has no effect: GetAppBundle still reports the bundle. After the contents are deleted the same DeleteAppBundle call succeeds (204) and GetAppBundle then refuses 404 ResourceNotFoundException "The app bundle ID was not found." Smithy: DeleteAppBundle (DELETE /appbundles/{appBundleIdentifier}, success 204, @idempotent) models BOTH ConflictException (409) and ValidationException (400, required message + `reason`), so the model permits either and recorded behavior selects ValidationException.
Emulator: the refusal is classified ConflictException (HTTP 409), so the status, the error code and the error's member set (ConflictException's required resourceId/resourceType instead of ValidationException's required reason/fieldList) are all wrong; the replay asserted the code and never reached the message comparison.
Why: 409 ConflictException reads to an SDK retry policy and to human-written teardown loops as "transient, the state will settle, retry", while AWS's 400 ValidationException means "this request can never succeed as written until the caller changes something". A teardown or reconciliation loop keyed on that distinction spins or gives up wrongly, and a client branching on the error code takes the wrong branch.
Fix: DeleteAppBundle's non-empty-bundle guard in the appfabric crate's delete-app-bundle handler — raise ValidationException (400, with the required `reason`) carrying AWS's fixed message instead of ConflictException, leaving the bundle and its contents untouched.
Traces: appfabric.delete-app-bundle-with-contents-rejected (AssertionError: expected ValidationException, got 'ConflictException').

## Evidence
Both failures come from replaying the behavioral suite against the booted emulator (total 17/23 passed; appfabric 17/23, 6 mismatched, 0 errored; the other 4 failures are marked OWNED by 20261029T192800Z-appfabric-connect-never-validates-credentials-app-authorization-not.md and were skipped).

Cause 1 — appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner:
  check_appfabric_concurrent_create_app_authorization_duplicate_tenant_single_winner.py:131
  assert message == refusals[code], losers
  AssertionError: [('ConflictException', 'An app authorization already exists for this application and tenant') x4]
  Sequence: CreateAppBundle -> 5 threads concurrently CreateAppAuthorization(appBundleIdentifier=<bundle arn>, app="SLACK", authType="oauth2", credential.oauth2Credential={clientId, clientSecret}, tenant={tenantIdentifier=<unique>, tenantDisplayName="race-tenant"}).
  Expected for each loser, one of:
    409 ConflictException "Request to create AppAuthorization for App SLACK with authType oauth2 and tenantIdentifier <the request's tenantIdentifier> is already in progress."
    400 ValidationException "ClientId is already being used for this AppBundle and App."
  Actual: every loser got ConflictException with "An app authorization already exists for this application and tenant".
  The winner/loser counts and the persisted-state assertions (exactly one authorization listed for the tenant, matching the winner's ARN) were NOT reached but the counts asserted before the message check passed (1 winner, 4 losers), so only the refusal wording diverges.

Cause 2 — appfabric.delete-app-bundle-with-contents-rejected:
  check_appfabric_delete_app_bundle_with_contents_rejected.py:161
  assert code == "ValidationException", f"expected ValidationException, got {code!r}"
  AssertionError: expected ValidationException, got 'ConflictException'
  Sequence: CreateAppBundle -> CreateAppAuthorization(SLACK, oauth2) -> poll ListAppAuthorizations until the authorization is visible -> DeleteAppBundle(bundle).
  Expected: 400 ValidationException, message "The specified app bundle cannot be deleted." (all fixed text), bundle unaffected; then DeleteAppAuthorization followed by the same DeleteAppBundle succeeds and GetAppBundle refuses 404 ResourceNotFoundException "The app bundle ID was not found."
  Actual: refused as ConflictException (modeled httpError 409), so the assertion failed before message/reason could be compared.

Model context (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json):
  CreateAppAuthorization: POST /appbundles/{appBundleIdentifier}/appauthorizations, success 201, @idempotent; app = String255 required (no @enum); authType = AuthType enum [oauth2, apiKey]; errors AccessDenied(403), Conflict(409), InternalServer(500), ResourceNotFound(404), ServiceQuotaExceeded(402), Throttling(429), Validation(400).
  DeleteAppBundle: DELETE /appbundles/{appBundleIdentifier}, success 204, @idempotent; errors AccessDenied(403), Conflict(409), InternalServer(500), Throttling(429), Validation(400).
  ConflictException: httpError 409, required message/resourceId/resourceType.
  ValidationException: httpError 400, required message/reason (+ optional fieldList).

## Steps to Reproduce
Cause 1 (concurrent duplicate CreateAppAuthorization refusal wording):
1. CreateAppBundle (one bundle per account/region; clear any orphan bundle first).
2. From 5 threads simultaneously, POST /appbundles/{bundle}/appauthorizations with the identical body: app="SLACK", authType="oauth2", credential.oauth2Credential={clientId, clientSecret}, tenant={tenantIdentifier: <one fresh id shared by all 5>, tenantDisplayName: "race-tenant"}.
3. Exactly one call must succeed (the emulator gets this right). Inspect each loser's (code, message).
   Emulator: 409 ConflictException "An app authorization already exists for this application and tenant".
   AWS: either 409 ConflictException "Request to create AppAuthorization for App <app> with authType <authType> and tenantIdentifier <tenantIdentifier> is already in progress." (app/authType/tenantIdentifier from the request) or 400 ValidationException "ClientId is already being used for this AppBundle and App."
4. Confirm ListAppAuthorizations still shows exactly one authorization for that tenant, matching the winner.

Cause 2 (DeleteAppBundle on a non-empty bundle):
1. CreateAppBundle; CreateAppAuthorization(app=SLACK, authType=oauth2) inside it.
2. Poll ListAppAuthorizations until the new authorization is visible.
3. DELETE /appbundles/{bundle}.
   Emulator: 409 ConflictException. AWS: 400 ValidationException with required `reason`, message "The specified app bundle cannot be deleted.", bundle and authorization untouched.
4. Verify GetAppBundle still returns the bundle, then DeleteAppAuthorization and DELETE /appbundles/{bundle} again — it must now answer 204, and GetAppBundle must refuse 404 ResourceNotFoundException "The app bundle ID was not found."
