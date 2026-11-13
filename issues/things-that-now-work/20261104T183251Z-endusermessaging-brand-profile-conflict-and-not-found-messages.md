Issue-Source: trace-sweep-divergence
Issue-Trace: endusermessaging-brand-profile-deletion-protection
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: endusermessaging-brand-profile-deletion-protection endusermessaging-brand-profile-idempotent-create endusermessaging-brand-profile-lifecycle
Issue-Shape-Id: com.amazonaws.endusermessaging#EndUserMessaging
Issue-Key: endusermessaging|DeleteBrandProfile|4xx->4xx|$.message
Issue-Key: endusermessaging|DeleteBrandProfile|4xx->4xx|$.resourceType
Issue-Key: endusermessaging|GetBrandProfile|4xx->4xx|$.resourceType
Issue-Related: 20261104T183251Z-endusermessaging-input-validation-refusals-omit-fieldlist-and-diverge.md
Issue-Commit: e5b2e36fc94a41937ab22fe33196324420972c76

# endusermessaging brand-profile conflict and not-found messages use emulator prose and an uppercase resourceType

> _Note: this issue was identified against the code base at commit `e5b2e36fc94a41937ab22fe33196324420972c76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBrandProfile

## Description
Two root causes in the endusermessaging brand-profile refusal bodies, observed replaying the wire corpus simparity-wire-aws-v1 against the running (bootstrapped) emulator. The HTTP statuses and error codes are correct; the message text and the resourceType vocabulary are not. The open behavioral issue '20261104T173700Z-endusermessaging-brand-profiles-2-net-new-causes-createbrandprofile.md' describes the same two message templates from behavioral checks, but it carries no wire replay key (behavioral corpus; no Issue-Trace/Issue-Step), so the wire steps below are reported unowned by the replay and are filed here with their fixture bindings so a fixer can re-replay them; the two open endusermessaging stub issues cover unrelated operations.

=== Cause 1 of 2: the deletion-protection ConflictException message is an emulator-authored sentence and resourceType is spelled in the wrong vocabulary ===
AWS: Smithy model ConflictException (HTTP 409, @error client; members message @required, resourceId, resourceType) is declared on DeleteBrandProfile. AWS words this refusal with a fully fixed template for the error class: 'Conflict occurred - Reason="DELETION_PROTECTION_ENABLED" ResourceType="brand-profile"'. Fixed text: 'Conflict occurred - Reason=' and ' ResourceType=' with the quoted reason token DELETION_PROTECTION_ENABLED (the state that caused the conflict) and the resource type in AWS's hyphenated vocabulary (brand-profile). No request-supplied value appears in it — in particular the brand profile id does NOT. The resourceType member is reported in the same hyphenated vocabulary as the message ('brand-profile').
Emulator: answers 409 ConflictException (correct) but with its own sentence 'Brand profile <brandProfileId> cannot be deleted while deletion protection is enabled.' — different fixed text, and it interpolates the id AWS's conflict message omits — and reports resourceType 'BRAND_PROFILE'.
Why: a client that reads Reason="..." out of the conflict message to tell deletion protection apart from other 409 conflict classes finds no Reason token at all; a client switching on resourceType against AWS's hyphenated vocabulary never matches, and support tooling/runbooks that match the refusal text do not fire.
Fix: DeleteBrandProfile's deletion-protection branch (its refusal construction) and the shared brand-profile resource-type constant — emit AWS's template with the reason token and hyphenated type as the only non-fixed parts, and report resourceType in that same vocabulary.
Traces: endusermessaging-brand-profile-deletion-protection#task#2.

=== Cause 2 of 2: the brand-profile ResourceNotFoundException message is an emulator-authored sentence and carries the same wrong resourceType ===
AWS: Smithy model ResourceNotFoundException (HTTP 404, @error client; members message @required, resourceId, resourceType) is declared on GetBrandProfile/UpdateBrandProfile/DeleteBrandProfile. AWS words it 'Resource not found - ResourceType="<type>" ResourceId="<id>"' — fixed text 'Resource not found - ResourceType=' and ' ResourceId=', with <type> the resource's hyphenated type (brand-profile) and <id> the identifier taken verbatim from the request. resourceType is likewise 'brand-profile'.
Emulator: answers 404 ResourceNotFoundException (correct) with 'Brand profile <id> was not found.' and resourceType 'BRAND_PROFILE'. This is the service's shared not-found constructor, so every brand-profile 404 on the surface carries it (the replay clusters three steps across two fixtures onto this one cause).
Why: a client parsing the type/id out of AWS's templated message to decide whether to recreate the resource or to report a bad identifier cannot, and the uppercase resourceType breaks the same vocabulary comparison as cause 1.
Fix: the service's shared brand-profile not-found error constructor (state module) — build AWS's template with the hyphenated resource type and the request's identifier as the only variable parts, and report resourceType in that vocabulary.
Traces: endusermessaging-brand-profile-deletion-protection#task#5, with clustered members endusermessaging-brand-profile-idempotent-create#task#5 and endusermessaging-brand-profile-lifecycle#task#7.

## Evidence
Cause 1 — fixture endusermessaging-brand-profile-deletion-protection step task#2 (DeleteBrandProfile, 409 vs 409):
  $.message expected: 'Conflict occurred - Reason="DELETION_PROTECTION_ENABLED" ResourceType="brand-profile"' (entirely fixed text for this error class)
  $.message actual:   'Brand profile bp-b95c1fa1800000001 cannot be deleted while deletion protection is enabled.' (the bp-… id is this run's minted value and is not part of any template)
  $.resourceType expected 'brand-profile', actual 'BRAND_PROFILE'.
  Source confirming the cause: services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/delete_brand_profile.rs:183-195 (conflict message format! string) and src/state.rs:222-224 (BRAND_PROFILE_RESOURCE_TYPE = "BRAND_PROFILE").

Cause 2 — fixture endusermessaging-brand-profile-deletion-protection step task#5 (GetBrandProfile, 404 vs 404):
  $.message expected: 'Resource not found - ResourceType="brand-profile" ResourceId="bp-b95c1fa1800000001"' — fixed text 'Resource not found - ResourceType=' / ' ResourceId=', type token 'brand-profile', id taken verbatim from the request (the bp-… value shown is this run's).
  $.message actual:   'Brand profile bp-b95c1fa1800000001 was not found.'
  $.resourceType expected 'brand-profile', actual 'BRAND_PROFILE'.
  Clustered members (same divergence_key service=endusermessaging|GetBrandProfile|status=404->404): endusermessaging-brand-profile-idempotent-create#task#5, endusermessaging-brand-profile-lifecycle#task#7.
  Source confirming the cause: services/service-com-amazonaws-endusermessaging-endusermessaging/src/state.rs:226-234 (brand_profile_not_found builds 'Brand profile {id_or_arn} was not found.' with resource_type BRAND_PROFILE).

## Steps to Reproduce
Cause 1:
1. POST /v1/brand-profiles with {"brandProfileName": "<any valid name>", "deletionProtectionEnabled": true}; note brandProfileId.
2. DELETE /v1/brand-profiles/{brandProfileId} — 409 ConflictException (correct status/code) but message is 'Brand profile <id> cannot be deleted while deletion protection is enabled.' instead of 'Conflict occurred - Reason="DELETION_PROTECTION_ENABLED" ResourceType="brand-profile"', and resourceType is 'BRAND_PROFILE' instead of 'brand-profile'.

Cause 2:
1. PUT /v1/brand-profiles/{brandProfileId} with {"deletionProtectionEnabled": false}, then DELETE /v1/brand-profiles/{brandProfileId} (succeeds).
2. GET /v1/brand-profiles/{brandProfileId} — 404 ResourceNotFoundException (correct status/code) but message is 'Brand profile <id> was not found.' instead of 'Resource not found - ResourceType="brand-profile" ResourceId="<id>"', and resourceType is 'BRAND_PROFILE'.

Replay: wire corpus simparity-wire-aws-v1, fixture endusermessaging-brand-profile-deletion-protection steps task#2 and task#5.
