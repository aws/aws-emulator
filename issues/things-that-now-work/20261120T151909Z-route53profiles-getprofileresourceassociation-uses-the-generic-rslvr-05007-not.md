Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.disassociate-resource-from-profile-removes-it
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles.disassociate-resource-from-profile-removes-it|run#14|AssertionError|assert error["Message"] == expected_message, f"expected {expected_message!r}, got {error['Message']!r}"
Issue-Commit: 53763dd8e71a257e04fb6c212b98aad1027c99f7

# route53profiles GetProfileResourceAssociation uses the generic [RSLVR-05007] not-found text instead of the Profile-association template [RSLVR-05200] Can't locate the Profile association with ID

> _Note: this issue was identified against the code base at commit `53763dd8e71a257e04fb6c212b98aad1027c99f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProfileResourceAssociation

## Description
ONE net-new behavioral cause in the route53profiles Profile-resource-association plane, found by replaying the behavioral suite (simparity-behavioral-aws-v1) against the booted emulator. The two other failures in this service are already covered and were skipped: the UpdateProfileResourceAssociation scaffold stub (filed for me by the replay) and the in-VPC SERVFAIL check owned by 20261120T045811Z-route53profiles-the-vpc-route-53-resolver-answers-servfail.md.

This is NOT covered by the open issue 20261104T173229Z-route53profiles-getprofileresourceassociation-refuses-an-unknown-resource-association-id.md. That issue is about an id that was NEVER minted, which AWS refuses as an INVALID-PARAMETER class error with the template '[RSLVR-05200] The Profile resource association ID in your request isn't valid: "<id>". Check the ID and try again.' This cause is the complementary path: an id that WAS minted and has since been removed by DisassociateResourceFromProfile. There AWS does answer ResourceNotFoundException (the error CODE the emulator already returns is correct, so that issue's fix does not touch it), but with a DIFFERENT, association-specific message template, which the emulator still gets wrong. Fixing only the never-minted path leaves this check failing.

Cause 1 of 1: after a resource association is disassociated and has disappeared, GetProfileResourceAssociation on its id is refused with the shared generic Resolver-family not-found text instead of the Profile-association-specific not-found template.

AWS: GetProfileResourceAssociation is GET /profileresourceassociation/{ProfileResourceAssociationId}, success 200, and models AccessDeniedException, InvalidParameterException, ResourceNotFoundException, ThrottlingException and ValidationException, all errorKind client at httpError 400 (defaulted) (Smithy: operation GetProfileResourceAssociation; input member ProfileResourceAssociationId -> ResourceId, @httpLabel, required, @length 1..64; shape ResourceNotFoundException {Message, ResourceType}, doc 'The resource you are associating is not found.'). The model does not state the message text, so the wording is recorded behavior only: the recording shows HTTP 400, Error.Code 'ResourceNotFoundException' (this family carries its sub-error code inside the message, not in Error.Code), and a Message built from the Profile-association template whose FIXED text is the literal sub-error code '[RSLVR-05200]' followed by "Can't locate the Profile association with ID: " and a trailing '.', with the id quoted in double quotes. Its only VARIABLE part is the id, taken verbatim from the request path — in the replay that happened to be 'rpr-52f8055800000002', a value the emulator minted in that run and no part of the requirement. In full: '[RSLVR-05200] Can\'t locate the Profile association with ID: "<ProfileResourceAssociationId>".'

Emulator: answers HTTP 400 with Error.Code 'ResourceNotFoundException' (correct) but Message '[RSLVR-05007] Can\'t find the resource with ID "<id>".' — the generic resource-kind-agnostic template. The handler routes the store miss through the shared not-found helper, so a removed resource association gets the same prose as any other missing resource of the service, with the wrong sub-error code ([RSLVR-05007] instead of [RSLVR-05200]) and the wrong sentence.

Why: the Resolver family reports its real sub-error code inside the message, so the message IS the machine-readable discriminator here. A client that greps the message for '[RSLVR-05200]' to recognise a removed Profile association, logs or asserts on the sentence, or parses out the quoted id after 'Profile association with ID:' never matches the emulator's reply; code that routes on the sub-error code sees [RSLVR-05007] and treats a deleted Profile association as some unrelated missing resource. Any test or runbook pinned to AWS's wording fails against the emulator even though the status and error code are right.

Fix: the message builder for a resource-association not-found refusal, and the error GetProfileResourceAssociation's handler selects for a store miss. The association-specific template belongs beside the service's other Resolver-family message builders in the state module rather than delegating to the generic helper; the handler should keep answering ResourceNotFoundException for an id that was minted and then removed. Note this must be reconciled with the open never-minted-id issue above, which requires a different error class and a different [RSLVR-05200] template for an id that was never minted — the two paths need distinguishing, not one shared message.

Traces: behavioral check route53profiles.disassociate-resource-from-profile-removes-it (asserts the post-disassociation GetProfileResourceAssociation refusal's Code and Message). Code: services/service-com-amazonaws-route53profiles-route53profiles/src/ops/get_profile_resource_association.rs lines 166-175 (store miss mapped through crate::state::profile_resource_association_not_found) and services/service-com-amazonaws-route53profiles-route53profiles/src/state.rs lines 1017-1019 and 1098-1107 (resource_not_found_message / profile_resource_association_not_found, the generic [RSLVR-05007] template).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator: 16/19 passed (84.2%), 2 mismatched, 1 errored, all under service route53profiles. Of the 3 failures, 1 is ALREADY FILED (UpdateProfileResourceAssociation scaffold stub) and 1 is OWNED by 20261120T045811Z-route53profiles-the-vpc-route-53-resolver-answers-servfail.md; this is the one unowned cluster.

[mismatch] route53profiles.disassociate-resource-from-profile-removes-it
  check_route53profiles_disassociate_resource_from_profile_removes_it.py line 62:
  assert error["Message"] == expected_message
  AssertionError: expected '[RSLVR-05200] Can\'t locate the Profile association with ID: "rpr-52f8055800000002".', got '[RSLVR-05007] Can\'t find the resource with ID "rpr-52f8055800000002".'

The preceding assertions in the same check passed, so: DisassociateResourceFromProfile returned the association with Status DELETING/DELETED, the association did disappear, and the later Get refusal carries Error.Code 'ResourceNotFoundException' as AWS does. The sole divergence is the Message.

Fixed text of the requirement: '[RSLVR-05200] ' + "Can't locate the Profile association with ID: " + the id in double quotes + '.'. Variable part: the ProfileResourceAssociationId from the request path. 'rpr-52f8055800000002' is the id that run minted and is not part of the requirement. Emulator's fixed text: '[RSLVR-05007] ' + "Can't find the resource with ID " + quoted id + '.'.

## Steps to Reproduce
Boot the emulator and point a route53profiles client (and a route53resolver client) at it.

1. CreateProfile(Name=<any>, ClientToken=<any>) -> note Profile.Id.
2. route53resolver CreateFirewallRuleGroup(CreatorRequestId=<any>, Name=<any>) -> note FirewallRuleGroup.Arn.
3. AssociateResourceToProfile(ProfileId=<profile id>, ResourceArn=<rule group arn>, Name=<any>, ResourceProperties='{"priority": 150}') -> note ProfileResourceAssociation.Id (an 'rpr-' id). Poll GetProfileResourceAssociation until Status == COMPLETE.
4. DisassociateResourceFromProfile(ProfileId=<profile id>, ResourceArn=<rule group arn>) -> returns that same Id with Status DELETING/DELETED. Poll until GetProfileResourceAssociation on that id starts failing.
5. Call GetProfileResourceAssociation(ProfileResourceAssociationId=<that id>).
   Expect: HTTP 400, Error.Code 'ResourceNotFoundException', Error.Message '[RSLVR-05200] Can\'t locate the Profile association with ID: "<that id>".' with the id echoed verbatim from the request.
   Actual: HTTP 400, Error.Code 'ResourceNotFoundException', Error.Message '[RSLVR-05007] Can\'t find the resource with ID "<that id>".'
6. ListProfileResourceAssociations(ProfileId=<profile id>) must not list the id (this already holds).

Positive control that must keep working: before the disassociate, GetProfileResourceAssociation on the minted id still answers 200 with the ProfileResourceAssociation member.

Then re-replay behavioral check route53profiles.disassociate-resource-from-profile-removes-it in full and confirm it passes.
