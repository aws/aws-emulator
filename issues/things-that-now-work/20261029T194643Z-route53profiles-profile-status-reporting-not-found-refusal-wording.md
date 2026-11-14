Issue-Source: trace-sweep-divergence
Issue-Trace: route53profiles-profile-crud-and-tags
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53profiles-profile-crud-and-tags route53profiles-profile-error-paths
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles|GetProfile|2xx->2xx|$.Profile.StatusMessage
Issue-Key: route53profiles|DeleteProfile|2xx->2xx|$.Profile.StatusMessage
Issue-Key: route53profiles|GetProfile|4xx->4xx|$.Message
Issue-Key: route53profiles|CreateProfile|4xx->2xx|$.message
Issue-Key: route53profiles|CreateProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: route53profiles|CreateProfile|4xx->2xx|status
Issue-Key: route53profiles|GetProfileAssociation|4xx->4xx|$.Message
Issue-Key: route53profiles|DeleteProfile|4xx->4xx|$.Message
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# route53profiles Profile status reporting, not-found refusal wording and CreateProfile Name validation diverge

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProfile

## Description
SIX net-new wire causes in the route53profiles Profile plane, found by replaying the wire corpus (trace set simparity-wire-aws-v1). They share one root shape: the emulator reports the Profile lifecycle without the StatusMessage AWS attaches to every Profile status, it authors its own refusal prose instead of the Resolver-family [RSLVR-*] templates, and it never enforces the modeled Name pattern at all. ALL SIX must be closed before this issue is submitted.

Related but NOT covering: the open behavioral issue 'route53profiles error reporting diverges from AWS: ResourceNotFoundException/ConflictException messages lack the [RSLVR-*] templates, and a VPC already held by another Profile is refused with the wrong error code' (20261029T191831Z-route53profiles-error-reporting-diverges-from-aws-resourcenotfoundexception-conflictexception.md) describes the same not-found message family from the BEHAVIORAL corpus; the replay reports no owner for these wire steps, so Causes 3, 5 and 6 are filed here with their own fixture/step replay keys. Causes 1, 2 and 4 are not in that issue at all.

Cause 1 of 6 - GetProfile omits Profile.StatusMessage for a Profile in COMPLETE.
AWS: the Profile shape carries StatusMessage alongside Status (Smithy: shape Profile, member StatusMessage -> String, documented 'Status message that includes additiona information about the Profile'; GetProfile output member Profile, HTTP GET /profile/{ProfileId}, success 200). The recording shows AWS always populates it for a Profile that finished being created: the fixed text 'Created Profile' accompanying Status=COMPLETE. The pairing of a status with its human-readable message is recorded behavior only - the model does not enumerate the strings - but the member being present for a COMPLETE Profile is what the recording states.
Emulator: answers 200 with the right Status but no StatusMessage member at all ($.Profile.StatusMessage: missing, expected 'Created Profile', got None). CreateProfile stores status_message: None for every Profile it mints, so every read path that projects the stored record also omits it.
Why: a client that surfaces or branches on StatusMessage (the only place this service explains a non-terminal or failed Profile) sees nothing, so progress/diagnostic UIs render blank and any assertion that the created Profile reports its creation message fails against the emulator while passing against AWS.
Fix: CreateProfile's handler must set the Profile's status message when it mints the record in COMPLETE, and the state projection must carry it through; the field already exists on the stored struct and in the wire shape, it is simply never filled.
Traces: route53profiles-profile-crud-and-tags#task#2 (GetProfile right after CreateProfile), also route53profiles-profile-error-paths#task#7. Code: services/service-com-amazonaws-route53profiles-route53profiles/src/ops/create_profile.rs line 208 (status_message: None) and src/state.rs line 328 (profile_shape passes the stored None straight through).

Cause 2 of 6 - DeleteProfile omits Profile.StatusMessage on the deleted Profile it reports back.
AWS: DeleteProfile returns the Profile as it stands at deletion (Smithy: DeleteProfile, HTTP DELETE /profile/{ProfileId}, success 200, output member Profile). The recording shows the returned Profile in the DELETING status together with its matching message, fixed text 'Deleting Profile'. Recorded behavior only for the exact string; the member's presence for a deleting Profile is what the recording states.
Emulator: answers 200, sets Status=DELETING correctly, but leaves StatusMessage absent ($.Profile.StatusMessage: missing, expected 'Deleting Profile', got None), because the delete path mutates only status and modification_time on the removed record.
Why: the delete response is the only acknowledgement a caller gets that teardown started; without the message a client cannot distinguish 'deletion accepted' from a Profile whose status it cannot explain, and teardown assertions on the message fail.
Fix: DeleteProfile's handler, in the branch that mutates the removed record before projecting it - it must set the status message to match the DELETING status it already sets.
Traces: route53profiles-profile-crud-and-tags#teardown#4, also route53profiles-profile-error-paths#teardown#8. Code: src/ops/delete_profile.rs lines 194-197.

Cause 3 of 6 - GetProfile's ResourceNotFoundException message is emulator-authored instead of the [RSLVR-05007] template.
AWS: a Profile id that names nothing is refused with ResourceNotFoundException at HTTP 400 (Smithy: GetProfile errors include ResourceNotFoundException, errorKind client, httpError 400 defaulted). The message is one fixed template shared by every resource kind in this Resolver-family service: the fixed text [RSLVR-05007] Can't find the resource with ID "<id>". - i.e. the literal sub-error code [RSLVR-05007], the fixed sentence Can't find the resource with ID, and ONE variable part, the id taken verbatim from the request. The template itself is recorded behavior only (the model documents the error only as a missing resource); the status and code are modeled.
Emulator: right status (400) and right code, but the message reads 'Profile <id> was not found.' - different fixed text, no [RSLVR-05007] code, and it names the resource kind where AWS's template does not.
Why: this service exposes its sub-error code ONLY inside the message, so a client branching on [RSLVR-05007] never matches and falls through to a generic failure path; 'assert the Profile is gone after delete' checks that compare the message also fail.
Fix: the shared not-found message builder in this service's state module that every handler routes its Profile not-found refusal through - it must build the single [RSLVR-05007] template, filling the id from the request.
Traces: route53profiles-profile-crud-and-tags#teardown#5 (GetProfile on a just-deleted id), also route53profiles-profile-error-paths#task#3. Code: src/state.rs lines 373-378 (profile_not_found).

Cause 4 of 6 - CreateProfile never enforces Name's modeled pattern: a numeric-only name is accepted with 200 where AWS refuses with 400.
AWS: Name is constrained (Smithy: CreateProfile input member Name, target Name, length 0..64, pattern ^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$ - the negative lookahead forbids an all-digits name) and CreateProfile models ValidationException (errorKind client, httpError 400). The recorded refusal is the standard constraint-violation body: HTTP 400, header x-amzn-errortype naming the error shape, and body key 'message' (lowercase) with the fixed text '<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>'. Its variable parts are the count of violations, the rejected value from the request, the input member name as the wire spells it ('name'), and the pattern from the model with its ^/$ anchors stripped.
Emulator: accepts the request outright - status: expected 400, got 200; $.message missing; $headers.x-amzn-errortype missing. The handler runs no constraint check and the request walk's validate stage does not enforce the pattern, so an invalid Profile is minted and persisted.
Why: the worst class of divergence here - the emulator is over-permissive, so a client that would have been refused by AWS proceeds with an unnameable Profile, and every subsequent read/list/associate step works against state AWS would never hold. Negative tests asserting the 400 pass against AWS and fail here, and the Profile leaks into the store.
Fix: CreateProfile's input validation - the modeled length/pattern constraints on Name must be enforced before the handler mints anything, and the violation rendered as the service's ValidationException with the standard constraint message and the x-amzn-errortype header.
Traces: route53profiles-profile-error-paths#task#2 (CreateProfile with an all-digits Name). Code: src/ops/create_profile.rs lines 154-223 (handle performs no Name check; input.name is stored unvalidated at line 202).

Cause 5 of 6 - GetProfileAssociation's ResourceNotFoundException message is emulator-authored instead of the same [RSLVR-05007] template.
AWS: GetProfileAssociation refuses an unknown association id with ResourceNotFoundException at HTTP 400 (Smithy: GetProfileAssociation, HTTP GET /profileassociation/{ProfileAssociationId}, errors include ResourceNotFoundException, errorKind client, httpError 400 defaulted). The message is the SAME single template as Cause 3 - fixed text [RSLVR-05007] Can't find the resource with ID "<id>". with the id from the request - AWS does not vary it by resource kind. Recorded behavior only for the template text.
Emulator: 400 with the correct code but the message 'Profile association <id> was not found.' - different fixed text, no [RSLVR-05007], and kind-specific wording AWS does not use.
Why: same as Cause 3 for the association plane: [RSLVR-05007] matching fails, and 'the association is gone after disassociate' assertions fail.
Why it is not Cause 3: a different builder on a different resource kind - fixing the Profile builder alone leaves this one diverging.
Fix: the shared association not-found message builder in the state module, which must emit the identical [RSLVR-05007] template rather than its own prose.
Traces: route53profiles-profile-error-paths#task#4 (GetProfileAssociation on an id that was never minted). Code: src/state.rs lines 382-391 (profile_association_not_found).

Cause 6 of 6 - DeleteProfile's not-found refusal carries the same emulator-authored message.
AWS: DeleteProfile on an id that names no Profile answers ResourceNotFoundException at HTTP 400 (Smithy: DeleteProfile errors include ResourceNotFoundException, errorKind client, httpError 400 defaulted) with the [RSLVR-05007] Can't find the resource with ID "<id>". template, id from the request. Recorded behavior only for the template text.
Emulator: 400 with the right code and the wrong message 'Profile <id> was not found.'.
Why: a teardown or idempotent-delete path that inspects the message to confirm the resource is absent cannot match AWS's [RSLVR-05007] marker and treats a benign absence as an unknown failure.
Why it is not Cause 3: same builder, but a distinct operation and a distinct recorded step - the replay reports it as its own cluster, so it must be re-replayed on DeleteProfile as well as GetProfile to confirm the fix.
Fix: same shared builder as Cause 3; verify DeleteProfile's not-found branch routes through it after the change.
Traces: route53profiles-profile-error-paths#task#6 (DeleteProfile on a never-created id). Code: src/ops/delete_profile.rs lines 162-167 and src/state.rs lines 373-378.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator: 17 steps, 5 passed, 12 mismatched, score 29.4%, 6 divergence clusters - every one of them route53profiles, none stub-blocked and none owned by an open issue.

Cause 1 - fixture route53profiles-profile-crud-and-tags step task#2, GetProfile, expected 200 / actual 200: $.Profile.StatusMessage: missing (expected 'Created Profile', got None). Cluster also covers route53profiles-profile-error-paths#task#7.

Cause 2 - fixture route53profiles-profile-crud-and-tags step teardown#4, DeleteProfile, expected 200 / actual 200: $.Profile.StatusMessage: missing (expected 'Deleting Profile', got None). Cluster also covers route53profiles-profile-error-paths#teardown#8.

Cause 3 - fixture route53profiles-profile-crud-and-tags step teardown#5, GetProfile, expected 400 / actual 400: $.Message value_mismatch - expected '[RSLVR-05007] Can\'t find the resource with ID "rp-d831635700000001".', got 'Profile rp-d831635700000001 was not found.'. Cluster also covers route53profiles-profile-error-paths#task#3. (The rp-… value is that run's minted id; the requirement is the template with the id taken from the request.)

Cause 4 - fixture route53profiles-profile-error-paths step task#2, CreateProfile: status expected 400, got 200; $.message missing (expected "1 validation error detected: Value '123456' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!^[0-9]+$)([a-zA-Z0-9\\-_' ']+)", got None); $headers.x-amzn-errortype missing (expected the error-shape header, got None). '123456' is that recording's rejected value; the fixed text and the pattern are the requirement.

Cause 5 - fixture route53profiles-profile-error-paths step task#4, GetProfileAssociation, expected 400 / actual 400: $.Message value_mismatch - expected '[RSLVR-05007] Can\'t find the resource with ID "rpa-doesnotexist000".', got 'Profile association rpa-doesnotexist000 was not found.'.

Cause 6 - fixture route53profiles-profile-error-paths step task#6, DeleteProfile, expected 400 / actual 400: $.Message value_mismatch - expected '[RSLVR-05007] Can\'t find the resource with ID "rp-doesnotexist00000".', got 'Profile rp-doesnotexist00000 was not found.'.

Statuses and error codes are correct on Causes 3, 5 and 6 (400 / ResourceNotFoundException); the divergence there is the message text. Causes 1 and 2 are missing response members on a 200. Cause 4 is a missing refusal entirely.

## Steps to Reproduce
Boot the emulator and point a route53profiles client at it.

Cause 1: CreateProfile(Name=<valid name>, ClientToken=<token>); GetProfile(ProfileId=<returned id>). Expect Profile.Status=COMPLETE AND Profile.StatusMessage='Created Profile'; the emulator omits StatusMessage.

Cause 2: with that Profile and no associations, DeleteProfile(ProfileId=<id>). Expect 200 with Profile.Status=DELETING AND Profile.StatusMessage='Deleting Profile'; the emulator omits StatusMessage.

Cause 3: immediately after that delete, GetProfile(ProfileId=<the deleted id>). Expect 400 ResourceNotFoundException with Message '[RSLVR-05007] Can\'t find the resource with ID "<that id>".'; the emulator answers 'Profile <id> was not found.'.

Cause 4: CreateProfile(Name='123456', ClientToken=<token>) - an all-digits name, which the modeled pattern's negative lookahead forbids. Expect HTTP 400 with header x-amzn-errortype naming the validation error shape and body {"message": "1 validation error detected: Value '123456' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!^[0-9]+$)([a-zA-Z0-9\\-_' ']+)"}; the emulator answers 200 and persists the Profile. Positive control that must keep working: a name with letters, digits, '-', '_', apostrophes or spaces still creates successfully.

Cause 5: GetProfileAssociation(ProfileAssociationId='rpa-doesnotexist000') - any id that was never minted. Expect 400 ResourceNotFoundException with Message '[RSLVR-05007] Can\'t find the resource with ID "<that id>".'; the emulator answers 'Profile association <id> was not found.'.

Cause 6: DeleteProfile(ProfileId='rp-doesnotexist00000') - any id that was never minted. Expect 400 ResourceNotFoundException with the same '[RSLVR-05007] Can\'t find the resource with ID "<that id>".' message; the emulator answers 'Profile <id> was not found.'.

Then re-replay fixtures route53profiles-profile-crud-and-tags and route53profiles-profile-error-paths in full: all 17 steps must pass.
