Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-domain-and-space-singleton-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-domain-and-space-singleton-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|CreateSpace|4xx->2xx|$.__type
Issue-Key: cloudwatchomni|CreateSpace|4xx->2xx|$.message
Issue-Key: cloudwatchomni|CreateSpace|4xx->2xx|status
Issue-Key: cloudwatchomni|DeleteDomain|2xx->4xx|$
Issue-Key: cloudwatchomni|DeleteDomain|2xx->4xx|status
Issue-Key: cloudwatchomni|GetDomain|4xx->2xx|$.__type
Issue-Key: cloudwatchomni|GetDomain|4xx->2xx|$.domain.customEndpointUrls[*]
Issue-Key: cloudwatchomni|GetDomain|4xx->2xx|$.domain.domainEndpointUrl
Issue-Key: cloudwatchomni|GetDomain|4xx->2xx|$.message
Issue-Key: cloudwatchomni|GetDomain|4xx->2xx|status
Issue-Commit: c2a9aa425461796da61883782981cd948eb4a841

# cloudwatchomni domain and space lifecycle state: second CreateSpace accepted and DeleteDomain then wedged on a stale space association

> _Note: this issue was identified against the code base at commit `c2a9aa425461796da61883782981cd948eb4a841`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSpace

## Description
THREE net-new causes in the cloudwatchomni emulator, all in the domain/space state layer: the account+Region space singleton rule is not enforced on CreateSpace, and the domain's space association is then never released, so the teardown half of the fixture cannot complete — DeleteDomain is refused on a domain that should be empty and GetDomain keeps serving a domain that should be gone. Each cause carries its own replay key below; close all three and re-replay all three steps before submitting.

Related but NOT covering these: open issue 20261111T022337Z-cloudwatchomni-account-region-singleton-rules-unenforced-and-name.md reports the CreateSpace singleton rule from the BEHAVIORAL suite (its cause 2, keyed on check cloudwatchomni.account-region-holds-at-most-one-space); it carries no wire replay key for this fixture step, and it says nothing about the delete-path causes 2 and 3 here. Open issue 20261111T022337Z-cloudwatchomni-conflict-and-validation-error-messages-diverge-from.md concerns only the WORDING of the DeleteDomain conflict message, not a conflict raised where AWS deletes successfully.

Cause 1 of 3: CreateSpace does not enforce the one-space-per-account-per-Region rule (fixture cloudwatchomni-domain-and-space-singleton-lifecycle, step task#5).
  AWS: Smithy CreateSpace declares ConflictException (errorKind client, @httpError 409) alongside ValidationException (400) and ResourceNotFoundException (404); the cardinality rule itself is recorded behavior only. The recording shows that while the account already holds a space in the Region, a second CreateSpace is refused with HTTP 409, __type com.amazon.cloudwatchomnifrontend#ConflictException and the message template "A space already exists for account <accountId> in region <region>" — the fixed text is "A space already exists for account " and " in region ", with the caller's account id and the request's Region as the only variables (the recorded run's 000000000000 / us-west-2 merely fill the template and are not part of it).
  Emulator: the second CreateSpace answers HTTP 200 with a complete second `space` member (fresh spaceId/spaceArn) under the same domain; no ConflictException is raised at any point.
  Why: a client that uses the refusal to converge (create-or-reuse, or a CloudFormation-style provisioner) silently creates a duplicate space AWS would never have created, and every later space-scoped call (dashboards, access profiles, grants, GetSpace/ListSpaces) answers from an estate that cannot exist on AWS. The conflict-handling branch of such a client is never exercised.
  Fix: CreateSpace's handler/state layer in the cloudwatchomni service crate (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend) — an account+Region space-uniqueness check evaluated BEFORE the space is inserted, returning ConflictException (409) with AWS's message template.
  Traces: cloudwatchomni-domain-and-space-singleton-lifecycle#task#5 (CreateSpace, expected 409, actual 200).

Cause 2 of 3: DeleteDomain refuses with ConflictException NOT_EMPTY a domain whose spaces have been deleted (fixture cloudwatchomni-domain-and-space-singleton-lifecycle, step task#13).
  AWS: Smithy DeleteDomain is @idempotent, takes only domainId (@pattern ^d-[0-9a-z]{1,25}$) and returns an EMPTY output shape; ConflictException (client, 409) is declared only for the non-empty case. Recorded behavior: once the domain holds no spaces, DeleteDomain answers HTTP 200 with an empty body `{}`.
  Emulator: the same call answers HTTP 409 with body {"__type": "ConflictException", "message": "Domain '<domainId>' still contains spaces and cannot be deleted.", "conflictType": "NOT_EMPTY", "resourceId": "<domainId>", "resourceType": "Domain"} — it still believes spaces are associated with the domain after they were deleted. Hypothesis: the association is stale either because DeleteSpace does not remove the space from the domain's space set, or because the extra space wrongly admitted by cause 1 is still attached; both leave the non-empty guard permanently true.
  Why: the documented teardown order (delete the spaces, then the domain) never terminates against the emulator — the domain is undeletable, so the account is wedged holding a domain it cannot remove, and because the domain is also treated as an account singleton nothing can be re-created afterwards. A client retrying the @idempotent delete loops on a 409 forever.
  Fix: the domain/space state layer — release the space association when a space is deleted (and when it was never legitimately created), and evaluate DeleteDomain's non-empty guard against the live space set so an empty domain deletes with a 200 empty body.
  Traces: cloudwatchomni-domain-and-space-singleton-lifecycle#task#13 (DeleteDomain, expected 200 with {}, actual 409 ConflictException NOT_EMPTY).

Cause 3 of 3: GetDomain keeps serving a deleted domain with 200 instead of ResourceNotFoundException (fixture cloudwatchomni-domain-and-space-singleton-lifecycle, step task#14).
  AWS: Smithy GetDomain is @readonly, takes domainId and declares ResourceNotFoundException (client, @httpError 404). Recorded behavior: after the domain is deleted, GetDomain answers HTTP 404 with __type com.amazon.cloudwatchomnifrontend#ResourceNotFoundException and the message "Domain not found" — wholly fixed text, with NO variable part (the domainId does not appear).
  Emulator: GetDomain answers HTTP 200 and returns the whole `domain` envelope (including its endpoint URLs) for a domain the fixture already deleted — no ResourceNotFoundException is produced. Hypothesis: the domain still exists only because the preceding DeleteDomain was refused by cause 2; the requirement asserted here is independent, namely that a deleted domain is not readable.
  Why: a client that polls GetDomain to confirm deletion (the normal wait-for-gone pattern) never observes the terminal state and either loops to its timeout or concludes the delete failed; cleanup verification and re-create flows both break, and the stale body advertises endpoint URLs for a resource that no longer exists.
  Fix: the domain state layer and GetDomain's lookup — removal must make the domain unreadable, and a missing domainId must map to ResourceNotFoundException (404) with AWS's fixed message "Domain not found".
  Traces: cloudwatchomni-domain-and-space-singleton-lifecycle#task#14 (GetDomain, expected 404, actual 200).

## Evidence
Fixture cloudwatchomni-domain-and-space-singleton-lifecycle, all three steps verdict mismatch.

Cause 1 — step task#5, CreateSpace, expected status 409, actual 200. Replay detail:
  status: expected 409, got 200
  $.__type: missing (expected 'com.amazon.cloudwatchomnifrontend#ConflictException', got None)
  $.message: missing (expected 'A space already exists for account 000000000000 in region us-west-2', got None)
The account id and Region in that message are the template's two variables (taken from the caller's identity and the request's Region); "A space already exists for account " / " in region " is the fixed text. The recorded 000000000000/us-west-2 are per-run values.

Cause 2 — step task#13, DeleteDomain, expected status 200, actual 409. Replay detail:
  status: expected 200, got 409
  $: value_mismatch (expected {}, got {'__type': 'ConflictException', 'message': "Domain 'd-cy5xn789rb9xgj6plvuxmdq3q' still contains spaces and cannot be deleted.", 'conflictType': 'NOT_EMPTY', 'resourceId': 'd-cy5xn789rb9xgj6plvuxmdq3q', 'resourceType': 'Domain'})
Expected: HTTP 200 with an empty JSON body (DeleteDomainOutput has no members). The domain id is a per-run value; the requirement is that no conflict is raised for an empty domain.

Cause 3 — step task#14, GetDomain, expected status 404, actual 200. Replay detail:
  status: expected 404, got 200
  $.__type: missing (expected 'com.amazon.cloudwatchomnifrontend#ResourceNotFoundException', got None)
  $.message: missing (expected 'Domain not found', got None)
  $.domain.domainEndpointUrl / $.domain.customEndpointUrls[0]: present in the actual body although no body is expected at all
Expected: 404 ResourceNotFoundException with the fixed message 'Domain not found'.

Smithy cited: CreateSpace input name (@length 3..64, @pattern ^[a-z0-9]+(-[a-z0-9]+)*$, required), domainId (DomainId, required), dataAccessRoleArn (required), clientToken (@idempotencyToken); errors AccessDenied 403, Conflict 409, InternalServer 500, ResourceNotFound 404, ServiceQuotaExceeded 402, Throttling 429, Validation 400. DeleteDomain @idempotent, input domainId only, EMPTY output shape, errors AccessDenied 403, Conflict 409, InternalServer 500, Throttling 429, Validation 400. GetDomain @readonly, errors include ResourceNotFoundException @httpError 404. Neither the space-singleton rule nor any message text is modeled — those are recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) and follow fixture 'cloudwatchomni-domain-and-space-singleton-lifecycle'.
3. Cause 1 — step task#5: with a domain created and one space already in the account/Region, call CreateSpace again in that same account and Region. Expected HTTP 409, __type com.amazon.cloudwatchomnifrontend#ConflictException, message "A space already exists for account <accountId> in region <region>". Actual: HTTP 200 with a second space.
4. Delete the space(s) in the domain (DeleteSpace), then Cause 2 — step task#13: call DeleteDomain(domainId=<the domain>). Expected HTTP 200 with an empty body. Actual: HTTP 409 ConflictException with conflictType NOT_EMPTY and message "Domain '<domainId>' still contains spaces and cannot be deleted.".
5. Cause 3 — step task#14: call GetDomain(domainId=<the deleted domain>). Expected HTTP 404 ResourceNotFoundException with message exactly 'Domain not found'. Actual: HTTP 200 with the full domain envelope.
6. Re-replay all three steps after fixing; cause 2 and cause 3 only become observable once the delete path is reached, so none of the three may be closed on the strength of another.
