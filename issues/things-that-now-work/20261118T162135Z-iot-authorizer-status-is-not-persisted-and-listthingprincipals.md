Issue-Source: trace-sweep-divergence
Issue-Trace: iot-authorizer-refusal-unresolvable-and-two-key-boundary
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-authorizer-refusal-unresolvable-and-two-key-boundary iot-principal-policy-thing-attachment-lifecycle iot-thing-principal-lifecycle iot-vpce-traj-attach-target-mistakes iot-thing-group-untag-noop-and-validation-errors
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DescribeAuthorizer|2xx->2xx|$.authorizerDescription.status
Issue-Key: iot|DeleteAuthorizer|2xx->4xx|status
Issue-Key: iot|ListThingPrincipals|2xx->2xx|$.principalsWithExclusiveAttachment
Issue-Key: iot|UntagResource|4xx->2xx|$
Issue-Key: iot|UntagResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|UntagResource|4xx->2xx|status
Issue-Related: 20261117T231652Z-iot-custom-authorizer-writes-tokensigningpublickeys-cardinality-is-unvalidated.md
Issue-Commit: 53152f24252eb4c6b050f7ba33da4a3e7b513fd2

# iot authorizer status is not persisted, and ListThingPrincipals and UntagResource responses diverge (4 causes)

> _Note: this issue was identified against the code base at commit `53152f24252eb4c6b050f7ba33da4a3e7b513fd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAuthorizer

## Description
Four net-new causes on the iot success paths: a requested authorizer status is dropped at the write (which then also breaks the delete), a modeled list response omits a member AWS always sends, and one input-validation refusal does not happen at all. All four are one service (iot); each is numbered below with its own fixture and step. Causes 1 and 2 are the same state defect observed at two operations and are listed separately because they are two distinct recorded divergences a fixer must re-replay.

The open issue 'iot custom-authorizer writes: tokenSigningPublicKeys cardinality is unvalidated and a created authorizer is reported ACTIVE' (20261117T231652Z-iot-custom-authorizer-writes-tokensigningpublickeys-cardinality-is-unvalidated.md) is adjacent but does not cover causes 1-2: it is about a create AWS rejects outright being persisted at all, whereas here a write AWS accepted asked for status INACTIVE and the emulator stored ACTIVE, and it carries neither replay key below.

Cause 1 of 4: an authorizer write that requests status INACTIVE is stored as ACTIVE.
AWS: Smithy: CreateAuthorizer.status targets AuthorizerStatus, an enum whose values are exactly [ACTIVE, INACTIVE], documented "The status of the create authorizer request."; AuthorizerDescription.status is the same enum, documented "The status of the authorizer." So the status the caller asks for is the status a subsequent DescribeAuthorizer must report. The recorded DescribeAuthorizer answers 200 with authorizerDescription.status = "INACTIVE" after a write that asked for INACTIVE.
Emulator: answers 200 with authorizerDescription.status = "ACTIVE" - the requested status is ignored and the record is stored active.
Why: an INACTIVE authorizer must not authorize connections, so a client that stages an authorizer as INACTIVE before wiring it up gets one that the emulator treats as live; and because the status is wrong in state, every status-conditioned follow-on (cause 2) misbehaves too.
Fix: the CreateAuthorizer/UpdateAuthorizer handler's state write - persist the request's `status` member (defaulting to ACTIVE only when it is absent) instead of hardcoding ACTIVE - and the DescribeAuthorizer projection that reads it back.
Traces: iot-authorizer-refusal-unresolvable-and-two-key-boundary#task#6. (The creationDate/lastModifiedDate values also reported in this cluster are per-run clock values and are NOT part of this finding.)

Cause 2 of 4: DeleteAuthorizer refuses to delete the authorizer, with 400.
AWS: Smithy: DeleteAuthorizer -> DELETE /authorizer/{authorizerName}, success code 200 with an empty body; its modeled errors are DeleteConflictException (httpError 409), InvalidRequestException (400), ResourceNotFoundException (404) and the server/throttling set. The recorded response is 200: the authorizer is INACTIVE at that point, so the delete is allowed. Note that when IoT does refuse a delete because the authorizer is still in use/active, the class is DeleteConflictException at 409 - never a bare 400.
Emulator: answers 400, so the authorizer is never deleted and the fixture's teardown leaves it behind (the following DescribeAuthorizer still answers 200). Hypothesis: the handler guards deletion on the authorizer being INACTIVE and, because cause 1 stored it ACTIVE, the guard fires; even on its own terms it would be the wrong class and status.
Why: the documented lifecycle deactivate-then-delete cannot be completed. A client or teardown script that follows AWS's sequence gets a permanent 400, leaks the authorizer, and then collides on the next create of the same name; and the 400 does not map to the DeleteConflictException an SDK would catch for a genuine in-use conflict.
Fix: the DeleteAuthorizer handler's precondition check - evaluate it against the persisted status (see cause 1) and, when it must refuse, raise DeleteConflictException with httpError 409.
Traces: iot-authorizer-refusal-unresolvable-and-two-key-boundary#teardown#7.

Cause 3 of 4: ListThingPrincipals omits principalsWithExclusiveAttachment.
AWS: recorded behavior only - the on-disk model's ListThingPrincipalsResponse declares only `principals` and `nextToken`, so this member postdates the checked-in model. The recording shows AWS answering 200 with `principalsWithExclusiveAttachment` present as an empty list whenever no attached principal holds an exclusive attachment, alongside `principals`. The live API's exclusive-attachment concept is corroborated in the same model by AttachThingPrincipal.thingPrincipalType, a ThingPrincipalType enum with values [EXCLUSIVE_THING, NON_EXCLUSIVE_THING]. The requirement is that the member is always serialized (empty list, not omitted) when there is nothing to report.
Emulator: answers 200 with the member absent entirely (null).
Why: a client that reads response.principalsWithExclusiveAttachment to decide whether a principal is exclusively attached gets null instead of an empty list and either NPEs or has to special-case the emulator; code that iterates the list unconditionally breaks.
Fix: the ListThingPrincipals output serializer/projection - emit principalsWithExclusiveAttachment, derived from the thingPrincipalType recorded at attach time, and serialize it as an empty list when no principal is exclusively attached.
Traces: iot-principal-policy-thing-attachment-lifecycle#task#6 (cluster also covers iot-principal-policy-thing-attachment-lifecycle#task#14, iot-thing-principal-lifecycle#task#4, iot-thing-principal-lifecycle#task#6, iot-vpce-traj-attach-target-mistakes#task#13).

Cause 4 of 4: UntagResource accepts an empty tagKeys list with 200 instead of refusing it.
AWS: Smithy: UntagResource.tagKeys is @required and targets TagKeyList; the operation models InvalidRequestException (httpError 400). The model states no minimum list length, so the refusal itself is recorded behavior only: AWS answers 400 with message "The validated collection is empty" - the entire message is fixed text with no variable part - when tagKeys is present but empty.
Emulator: answers 200 with an empty JSON body, silently treating the call as a no-op.
Why: the caller believes tags were removed when nothing was validated or changed. A client that builds tagKeys dynamically and ends up with an empty list gets success from the emulator and a hard 400 from AWS, so the bug is invisible until it runs for real; and a test that asserts the refusal cannot be written against the emulator.
Fix: the UntagResource handler's input validation - reject an empty tagKeys list with InvalidRequestException (400) and the message above, before any state mutation.
Traces: iot-thing-group-untag-noop-and-validation-errors#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (169 fixtures, 1120 steps). All four clusters are verdict=mismatch, service=iot, stub_path empty, owned_by empty.

Cause 1 - DescribeAuthorizer, expected 200 / actual 200:
  $.authorizerDescription.status: expected "INACTIVE", got "ACTIVE"
  (same cluster also reports authorizerDescription.creationDate / lastModifiedDate, which are per-run clock values and are not part of this finding)

Cause 2 - DeleteAuthorizer, expected 200 / actual 400:
  status: expected 200, got 400
  Downstream effect in the same fixture: the next DescribeAuthorizer answers 200 where AWS answered 404, i.e. the authorizer survived the delete.

Cause 3 - ListThingPrincipals, expected 200 / actual 200, cluster covers 5 steps:
  $.principalsWithExclusiveAttachment: missing (expected [], got None)

Cause 4 - UntagResource, expected 400 / actual 200:
  status: expected 400, got 200
  $: empty_body (expected {"message": "The validated collection is empty"}, got {})
  $headers.x-amzn-errortype: missing (expected an error type header, got none)

Model evidence (services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json): AuthorizerStatus enum values [ACTIVE, INACTIVE]; CreateAuthorizerRequest.status -> AuthorizerStatus ("The status of the create authorizer request."); AuthorizerDescription.status -> AuthorizerStatus ("The status of the authorizer."); DeleteAuthorizer -> DELETE /authorizer/{authorizerName}, code 200, errors {DeleteConflictException 409, InvalidRequestException 400, ResourceNotFoundException 404, ...}; ListThingPrincipalsResponse members {principals, nextToken} only (principalsWithExclusiveAttachment postdates this model); ThingPrincipalType enum [EXCLUSIVE_THING, NON_EXCLUSIVE_THING]; UntagResource.tagKeys @required -> TagKeyList, errors include InvalidRequestException 400.

## Steps to Reproduce
Boot the emulator from this checkout and replay the corpus (replay_wire_corpus), or drive the steps directly:

Causes 1 and 2: POST /authorizer/<name> with authorizerFunctionArn, tokenKeyName, one or two tokenSigningPublicKeys and status=INACTIVE. Then GET /authorizer/<name> and read authorizerDescription.status - AWS reports INACTIVE, the emulator reports ACTIVE (cause 1). Then DELETE /authorizer/<name> - AWS answers 200 and a following GET answers 404 ResourceNotFoundException; the emulator answers 400 and the following GET still answers 200 (cause 2). Replay iot-authorizer-refusal-unresolvable-and-two-key-boundary#task#6 and #teardown#7.

Cause 3: create a thing, create/register a certificate, PUT /things/<thing>/principals to attach it, then GET /things/<thing>/principals. AWS's 200 body carries principalsWithExclusiveAttachment as [] next to principals; the emulator omits the member. Replay iot-principal-policy-thing-attachment-lifecycle#task#6 (also #task#14, iot-thing-principal-lifecycle#task#4 and #task#6, iot-vpce-traj-attach-target-mistakes#task#13).

Cause 4: create a thing group and tag it, then POST /untag with that group's resourceArn and tagKeys set to an empty list. AWS answers 400 with {"message": "The validated collection is empty"}; the emulator answers 200 with {}. Replay iot-thing-group-untag-noop-and-validation-errors#task#7.
