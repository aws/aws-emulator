Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-quicksetup-not-found-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-quicksetup-not-found-error-paths ssm-quicksetup-validation-error-paths
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup|GetConfigurationManager|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|GetConfigurationManager|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|UpdateConfigurationManager|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|UpdateConfigurationManager|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|UpdateConfigurationManager|4xx->4xx|status
Issue-Key: ssm-quicksetup|DeleteConfigurationManager|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|DeleteConfigurationManager|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|DeleteConfigurationManager|4xx->4xx|status
Issue-Key: ssm-quicksetup|UpdateConfigurationDefinition|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|UpdateConfigurationDefinition|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|UpdateConfigurationDefinition|4xx->4xx|status
Issue-Key: ssm-quicksetup|TagResource|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|TagResource|4xx->4xx|status
Issue-Key: ssm-quicksetup|ListTagsForResource|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|ListTagsForResource|4xx->4xx|status
Issue-Key: ssm-quicksetup|UntagResource|4xx->4xx|$.message
Issue-Key: ssm-quicksetup|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ssm-quicksetup|UntagResource|4xx->4xx|status
Issue-Related: 20261110T211706Z-ssm-quicksetup-configuration-deployment-responses-diverge-minted-ids.md
Issue-Commit: 62e7cd207e97e43a34c717b397f398f40559e033

# ssm-quicksetup ARN-labelled routes no longer refuse an unentitled ARN with 401 UnauthorizedException

> _Note: this issue was identified against the code base at commit `62e7cd207e97e43a34c717b397f398f40559e033`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfigurationManager

## Description
2 causes; BOTH must be closed before this issue is submitted. They are two halves of one fault: the layer that used to stand in front of this endpoint's ARN-labelled routes and refuse an ARN it could not place as one the caller is entitled to is gone from this checkout, so eight recorded 401 UnauthorizedException refusals are now answered by the operations behind it (seven as the modelled 404, one as the modelled 400).

RELATED OPEN ISSUES, neither of which owns these steps (the replay reports owned_by: [] for all eight): 20261106T174308Z-ssm-quicksetup-the-configurationmanager-managerarn-routes-turn-a-deleted-manager.md and 20261102T090000Z-ssm-quicksetup-tag-plane-routes-answer-an-arn-naming-no.md required the MODELLED 404 ResourceNotFoundException for an ARN of the caller's OWN account and Region whose manager was deleted, and the first of those issues explicitly preserved this fixture's refusal as a requirement (its reproduction step 7: an ARN the caller is not entitled to "must still answer 401 UnauthorizedException with {\"message\": \"Unauthorized\"}, as the wire fixture ssm-quicksetup-not-found-error-paths records"). The emulator now satisfies those issues by deleting the front-door stage outright rather than by correcting its predicate, so the 404 path was won by losing the 401 path. The fix below must leave BOTH outcomes reachable; do not revert either issue.

=== Cause 1 of 8..2 — see per-cause headings below ===

Cause 1 of 2: every ARN-labelled route answers the modelled 404 ResourceNotFoundException where AWS answers the front door's 401 UnauthorizedException. Seven operations, seven steps, one shared refusal.
AWS: Smithy (services/service-com-amazonaws-ssmquicksetup-quicksetup/smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json) binds GetConfigurationManager (GET /configurationManager/{ManagerArn}), UpdateConfigurationManager (PUT, same URI), DeleteConfigurationManager (DELETE, same URI), UpdateConfigurationDefinition (PUT /configurationDefinition/{ManagerArn}/{Id}), TagResource (PUT /tags/{ResourceArn}), ListTagsForResource (GET /tags/{ResourceArn}) and UntagResource (DELETE /tags/{ResourceArn}) to exactly six errors each — AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). No 401 and no UnauthorizedException shape exists anywhere in this service's model, so this refusal is NOT a modelled error: it is stated by the layer in front of the service, and for it the recording is the only authority (recorded behavior only). What the recording states, identically for all seven operations on the ARN these steps carry: HTTP 401, header x-amzn-errortype: UnauthorizedException, body {"message": "Unauthorized"} — a LOWERCASE `message` member (unlike this service's own modelled errors, whose member is `Message`), whose sentence `Unauthorized` is WHOLLY FIXED text: it names no member, no id, no ARN, no limit and no value, so nothing in it is taken from the request or from state. Hypothesis (neither the model nor the recording states the discriminator): the front door judges the ARN in the path label — its own account id and Region against the caller and the endpoint being served, and its well-formedness — before the request reaches the service, and refuses what it cannot place as the caller's; the modelled 404 belongs to an ARN that DOES reach the service and resolves to nothing there, which is the outcome the two related issues above are about. A fixer must read the ARNs these fixture steps actually carry (replay the fixture) to pin the predicate, not guess it.
Emulator: there is nothing in front of the handlers any more. services/service-com-amazonaws-ssmquicksetup-quicksetup/src/service.rs no longer has a request-authorization stage at all: its module docs now assert the opposite rule ("Whether such an ARN names a resource this endpoint serves is a RESOURCE outcome, never an authorization one, so nothing here stands between those requests and their handler"), and the UNAUTHORIZED_CODE/UNAUTHORIZED_STATUS/UNAUTHORIZED_MESSAGE constants survive wired ONLY to override_routing_error, i.e. to a request that names no modelled route. So each of the seven requests reaches its handler, resolves the ARN against the store, finds nothing, and answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and a `Message` body member, where the recording has HTTP 401 with x-amzn-errortype: UnauthorizedException and a lowercase `message` of `Unauthorized`.
Why: the two outcomes are read differently by every client. 401 is an entitlement/credential signal — SDKs and SigV4 middleware drive credential-refresh and re-sign paths off it, and because UnauthorizedException is in no operation's generated error union it surfaces as an unmatched generic ClientError that no not-found branch catches. ResourceNotFoundException is the opposite instruction: idempotent cleanup, delete-then-verify loops and tag reconciliation treat it as "already gone" and proceed. An emulator that answers 404 to an ARN AWS refuses as unauthorized makes a client silently conclude that an ARN it may not touch is simply absent, so teardown and reconciliation flows that AWS stops dead run to completion against the emulator.
Fix: restore the front-door refusal stage in the ssm-quicksetup service crate's request pipeline (services/service-com-amazonaws-ssmquicksetup-quicksetup/src/service.rs — the seam that runs before operation dispatch, alongside the existing unauthorized() renderer and the UNAUTHORIZED_* constants, plus the module docs that now state the no-stage rule), with the predicate it was missing the first time: the ARN's OWN entitlement (its account id and Region against the caller and the endpoint, and its well-formedness), NOT presence of a row in the store. An ARN the caller owns in this Region must still fall through to the handler's modelled ResourceNotFoundException with that handler's own not-found sentence, which is what the two related issues above require.
Traces: ssm-quicksetup-not-found-error-paths#task#1 (GetConfigurationManager, the representative), #task#2 (UpdateConfigurationManager), #task#3 (DeleteConfigurationManager), #task#4 (UpdateConfigurationDefinition), #task#7 (TagResource), #task#8 (ListTagsForResource), #task#9 (UntagResource).

Cause 2 of 2: the front door's refusal is also ordered BEFORE the modelled constraint walk, so a path label that is not a well-formed ARN earns 401 Unauthorized and not 400 ValidationException.
AWS: recorded behavior only for the ordering. The model does state the constraint itself — GetConfigurationManagerInput$ManagerArn is @required, @httpLabel, @length(min: 1) and @pattern "^arn:aws:ssm-quicksetup:([^:]+):(\\d{12}):configuration-manager/[0-9a-fA-F]{8}\\b-[0-9a-fA-F]{4}\\b-[0-9a-fA-F]{4}\\b-[0-9a-fA-F]{4}\\b-[0-9a-fA-F]{12}$" — but the recording shows a label of the wrong shape never reaches that walk: GetConfigurationManager with a managerArn that is not an ARN at all answers the SAME front-door refusal as cause 1, HTTP 401 with x-amzn-errortype: UnauthorizedException and body {"message": "Unauthorized"} (fixed text, no variable part). Hypothesis: a label that cannot be parsed as an ARN cannot be placed as one the caller is entitled to, so the entitlement refusal precedes — and absorbs — the pattern refusal on these routes.
Emulator: the request reaches the modelled constraint walk and is refused there instead, with HTTP 400, x-amzn-errortype: ValidationException and the JSON-protocol constraint sentence `1 validation error detected: Value '<label value>' at 'managerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <the model's pattern, unanchored>` — fixed text around three variable parts (the rejected value and the member name, both from the request, and the pattern from the model). That rendering is correct for a member AWS does validate; what diverges is that for an ARN path label on these routes AWS never gets that far.
Why: a client probing an identifier it does not own, or passing through a malformed identifier, is told its INPUT is bad (a 400 that SDKs never retry and user code reports as a programming error, leaking the rejected value and the service's internal pattern) where AWS tells it the call was not authorized. Error-handling and retry classification diverge, and a 400 also hides the entitlement boundary a client is probing for.
Fix: same component as cause 1 — the restored front-door stage in services/service-com-amazonaws-ssmquicksetup-quicksetup/src/service.rs must run BEFORE the modelled constraint walk for the ARN path labels of these routes, so a label it cannot place is refused 401 rather than falling through to override_refusal / aws_validation_refusal. The constraint walk itself and its 400 wording must not change for any other member.
Traces: ssm-quicksetup-validation-error-paths#task#1 (GetConfigurationManager).

NOT filed here, for the record: the two remaining divergences in this corpus (ssm-quicksetup-host-management-configuration-deploys#task#6 and #task#7) are already owned by open issue 20261110T211706Z-ssm-quicksetup-configuration-deployment-responses-diverge-minted-ids.md, which carries their replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 36 steps, 26 passed, 10 mismatched (score 72.2%), 0 blocked, 0 errored, emulator_up true (/__admin/health answered 200). Eight of the ten mismatches are the ones filed here; the other two are owned by an existing issue.

Cause 1 — fixture ssm-quicksetup-not-found-error-paths. All seven steps carry the IDENTICAL verdict detail:
```
status: expected 401, got 404
$.message: missing (expected 'Unauthorized', got None)
$headers.x-amzn-errortype: value_mismatch (expected 'UnauthorizedException', got 'ResourceNotFoundException')
```
- task#1 GetConfigurationManager  (divergence_key service=ssm-quicksetup|GetConfigurationManager|status=401->404)
- task#2 UpdateConfigurationManager
- task#3 DeleteConfigurationManager
- task#4 UpdateConfigurationDefinition
- task#7 TagResource
- task#8 ListTagsForResource
- task#9 UntagResource
Expected (AWS, recorded): HTTP 401; x-amzn-errortype: UnauthorizedException; body member lowercase `message` = "Unauthorized" (wholly fixed text — no member name, id, ARN or value from the request or state).
Actual (emulator): HTTP 404; x-amzn-errortype: ResourceNotFoundException; no lowercase `message` member at all (the modelled error carries `Message`).

Cause 2 — fixture ssm-quicksetup-validation-error-paths, task#1, GetConfigurationManager (divergence_key service=ssm-quicksetup|GetConfigurationManager|status=401->400):
```
status: expected 401, got 400
$.message: value_mismatch (expected 'Unauthorized', got "1 validation error detected: Value 'not-an-arn' at 'managerArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{12}")
$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
```
Expected (AWS, recorded): HTTP 401; UnauthorizedException; `message` = "Unauthorized". Actual (emulator): HTTP 400; ValidationException; the constraint sentence above — whose fixed text is `N validation error detected: Value '...' at '...' failed to satisfy constraint: Member must satisfy regular expression pattern: ...`, with the rejected value and the member name taken from the request and the expression taken from the model (the literal `not-an-arn` is this recording's filler, not a requirement).

Model — smithy_query(query="operation", operation="GetConfigurationManager,UpdateConfigurationManager,DeleteConfigurationManager,UpdateConfigurationDefinition,TagResource,ListTagsForResource,UntagResource", model_path="services/service-com-amazonaws-ssmquicksetup-quicksetup/smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json"): all seven bind AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). No 401, no UnauthorizedException anywhere in the service model — the refused-at-the-front-door answer is unmodelled by construction, which is why the recording is the authority for it.

Code — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/service.rs at this checkout:
- lines 43-65: module doc now states "A route's target resource is resolved by the operation, not in front of it" and "nothing here stands between those requests and their handler", explicitly for /configurationManager/{ManagerArn}, /tags/{ResourceArn} and /configurationDefinition/{ManagerArn}/{Id} — the three route families all eight failing steps hit.
- lines 61-65: records the consequence that a path label outside its @pattern "is refused by the modelled constraint walk, in the `400 ValidationException` envelope" — which is exactly cause 2's observed 400.
- lines 95-173: UNAUTHORIZED_CODE = "UnauthorizedException", UNAUTHORIZED_STATUS = 401, UNAUTHORIZED_MESSAGE = "Unauthorized", the lowercase `message` member schema and the unauthorized() renderer all still exist and still render the recorded envelope correctly.
- lines 230-244: that renderer is now reachable ONLY from override_routing_error, i.e. for a request naming no modelled route. No stage calls it for a request that DOES name a route, so no ARN-labelled request can be refused 401 any more.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1. Fixture ssm-quicksetup-not-found-error-paths mismatches on task#1, #2, #3, #4, #7, #8 and #9 (expected 401, got 404) and fixture ssm-quicksetup-validation-error-paths mismatches on task#1 (expected 401, got 400).
2. Cause 1, manually: send signed ssm-quicksetup requests for the ARN those fixture steps carry (replay the fixture to read it off the recorded request) against each of the seven routes — GET, PUT and DELETE /configurationManager/{ManagerArn}; PUT /configurationDefinition/{ManagerArn}/{Id}; GET, PUT and DELETE /tags/{ResourceArn}?tagKeys=k. Each answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and a `Message` member, instead of HTTP 401 with x-amzn-errortype: UnauthorizedException and body {"message": "Unauthorized"}.
3. Cause 2: GET /configurationManager/not-an-arn (any label that does not match ManagerArn's @pattern). It answers HTTP 400 ValidationException quoting the rejected label and the model's pattern, instead of HTTP 401 UnauthorizedException with {"message": "Unauthorized"} — i.e. the restored stage must run ahead of the modelled constraint walk for these path labels.
4. Regression controls that MUST keep passing after the fix (they pass today and are what the two related open issues bought):
   a. Create a manager (POST /configurationManager), delete it (DELETE /configurationManager/{ManagerArn}), then GET the same ARN — an ARN of the caller's own account and the endpoint's Region — and confirm it still answers the MODELLED HTTP 404 ResourceNotFoundException with Message "Quick setup manager with arn: <that ManagerArn> could not be found.", not 401.
   b. GET /tags/{that deleted ARN} and PUT /tags/{that deleted ARN} still answer their own modelled 404 ResourceNotFoundException sentences, not 401.
   c. A request naming no modelled route at all (e.g. DELETE /tags/{ResourceArn} with no tagKeys query) still answers 401 UnauthorizedException via override_routing_error.
5. Re-replay the corpus: all eight steps in trace_ids plus the representative must pass, and the 26 steps that pass today must still pass.
