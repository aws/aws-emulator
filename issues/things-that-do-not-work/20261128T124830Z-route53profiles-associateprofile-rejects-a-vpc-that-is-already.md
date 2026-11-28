Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.associate-profile-duplicate-vpc-rejected
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles.associate-profile-duplicate-vpc-rejected|run#10|AssertionError|assert error["Code"] == "LimitExceededException", f"expected LimitExceededException, got {error['Code']!r}"
Issue-Commit: 79b59159163307e875fa49ab8f4a087782266bf8

# route53profiles: AssociateProfile rejects a VPC that is already associated with another Profile as ValidationException instead of LimitExceededException

> _Note: this issue was identified against the code base at commit `79b59159163307e875fa49ab8f4a087782266bf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateProfile

## Description
Cause 1 of 1: AssociateProfile classifies the "VPC already carries a Profile association" refusal under the wrong error shape and the wrong message. Failing check: route53profiles.associate-profile-duplicate-vpc-rejected.

AWS: A VPC may be associated with at most one Route 53 Profile. When AssociateProfile is called with a ResourceId (VPC) that already has a COMPLETE association to a different Profile, AWS refuses the call with HTTP 400 and error code LimitExceededException — the per-VPC profile-association limit has been reached. Smithy: AssociateProfile models LimitExceededException (errorKind client, httpError 400, members Message + ResourceType, doc "The request caused one or more limits to be exceeded.") alongside ValidationException (doc "You have provided an invalid command."); the request here is well-formed — ProfileId/ResourceId/Name all satisfy their modeled constraints (ResourceId @length 1..64, Name pattern ^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$) — so it is a limit refusal, not a validation refusal. Recorded behavior: the error Message is the fixed text "VPC [" + <the ResourceId from the request> + "] exceeded its maximum profile association limit." — the only variable part is the VPC id taken from the request; everything else is fixed wording.

Emulator: the second AssociateProfile call (profile_b + the VPC already associated to profile_a) is refused, but with Error.Code "ValidationException" rather than "LimitExceededException". The check asserts the code first and fails there, so the emulator's Message for this refusal is not even reached; it must also carry the fixed text above with the request's VPC id substituted, and the LimitExceededException body may carry ResourceType.

Why: clients and SDK retry/branch logic key on the error code. Callers that treat LimitExceededException as "this VPC is taken, free it or pick another VPC" (and ValidationException as "my request is malformed, never retry") take the wrong branch: an orchestrator reassigning a Profile to a VPC sees a permanent bad-request instead of a recoverable capacity/limit condition, and automation that waits for the previous association to be removed before retrying never retries at all. Error-shape mismatch also changes the deserialized exception type in every SDK, breaking `except LimitExceededException` handlers.

Fix: the AssociateProfile handler's association-conflict validation — the branch that detects an existing profile association for the requested ResourceId must raise the LimitExceededException shape (HTTP 400, code LimitExceededException) with the per-VPC limit message templated from the request's ResourceId, instead of falling through to the generic ValidationException path. The rest of the check's expectations (the VPC still lists only profile_a in ListProfileAssociations, and the identical call for profile_b succeeding after DisassociateProfile frees the VPC) are gated behind this assertion and should be re-verified once the code is corrected.

Traces: route53profiles.associate-profile-duplicate-vpc-rejected (assertion: expected LimitExceededException, got 'ValidationException').

## Evidence
Check: route53profiles.associate-profile-duplicate-vpc-rejected (mismatch)

Expected (real AWS, per the recording and the Smithy model):
  HTTP 400, Error.Code = "LimitExceededException"
  Error.Message = "VPC [<vpc-id from request ResourceId>] exceeded its maximum profile association limit."

Actual (emulator):
  Error.Code = "ValidationException"

Failure output:
  File "check_route53profiles_associate_profile_duplicate_vpc_rejected.py", line 43, in run
    assert error["Code"] == "LimitExceededException", f"expected LimitExceededException, got {error['Code']!r}"
  AssertionError: expected LimitExceededException, got 'ValidationException'

Smithy (services/service-com-amazonaws-route53profiles-route53profiles): AssociateProfile errors include both LimitExceededException and ValidationException; LimitExceededException = {Message, ResourceType}, doc "The request caused one or more limits to be exceeded."

## Steps to Reproduce
1. Create a VPC (ec2:CreateVpc).
2. Create two Route 53 Profiles, profile_a and profile_b (route53profiles:CreateProfile).
3. AssociateProfile(ProfileId=profile_a, ResourceId=<vpc>, Name=<unique>) and poll GetProfileAssociation until Status is COMPLETE.
4. Call AssociateProfile(ProfileId=profile_b, ResourceId=<the same vpc>, Name=<unique>).
5. Observe the returned Error.Code: the emulator returns "ValidationException"; real AWS returns "LimitExceededException" with Message "VPC [<vpc-id>] exceeded its maximum profile association limit."
6. (Downstream, currently unreached) ListProfileAssociations(ResourceId=<vpc>) must still show only profile_a; after DisassociateProfile(profile_a, vpc) completes, the identical AssociateProfile call for profile_b must succeed.
