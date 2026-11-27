Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-domain-and-space-singleton-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-domain-and-space-singleton-lifecycle cloudwatchomni-domain-validation-and-notfound-errors
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|DeleteDomain|4xx->4xx|$.message
Issue-Key: cloudwatchomni|UpdateDomain|4xx->4xx|$.message
Issue-Commit: c2a9aa425461796da61883782981cd948eb4a841

# cloudwatchomni domain error messages diverge from AWS wording for DeleteDomain conflict and UpdateDomain not-found

> _Note: this issue was identified against the code base at commit `c2a9aa425461796da61883782981cd948eb4a841`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDomain

## Description
TWO net-new causes: two domain-scoped errors return the correct HTTP status and error type but a `message` AWS never emits. The message is part of the contract, so each is a defect with its own message template to fix; close both and re-replay both steps.

Related but NOT covering these: open issue 20261111T022337Z-cloudwatchomni-conflict-and-validation-error-messages-diverge-from.md reports the DeleteDomain non-empty wording from the BEHAVIORAL suite (its cause 4, keyed on check cloudwatchomni.domain-with-a-space-cannot-be-deleted) and carries no wire replay key for the step below. Open issue 20261109T172919Z-cloudwatchomni-resourcenotfoundexception-messages-do-not-match-aws-wording.md covers the not-found wording for the Get-style reads (GetDomain, GetAccessGrant, GetAlert, CreateSpace) and owns those replay keys; it does not carry UpdateDomain's not-found message, which is cause 2 here. Open issue 20261111T063558Z-cloudwatchomni-error-messages-diverge-from-aws-wording-for.md is the view/deep-link message bundle — different operations.

Cause 1 of 2: DeleteDomain's non-empty-domain ConflictException message omits AWS's fixed text and the associated-space count (fixture cloudwatchomni-domain-and-space-singleton-lifecycle, step task#6).
  AWS: Smithy DeleteDomain is @idempotent, takes only domainId (@pattern ^d-[0-9a-z]{1,25}$) and declares ConflictException (errorKind client, @httpError 409); the message text is recorded behavior only. While the domain still holds spaces the delete is refused with HTTP 409 and the template "Cannot delete Domain <domainId> because it has <n> associated Spaces. Delete all Spaces first." — the fixed text is "Cannot delete Domain ", " because it has " and " associated Spaces. Delete all Spaces first.", with two variables: the request's domainId and the current count of spaces in that domain taken from state (the recorded run's count of 1 fills the template and is not part of it).
  Emulator: the refusal is HTTP 409 with message "Domain '<domainId>' still contains spaces and cannot be deleted." — different fixed text, the domainId quoted, and the space count dropped entirely. Status and error type are correct.
  Why: a client that surfaces or parses the refusal (reporting how many spaces remain before teardown can proceed) has nothing to parse, and any exact-match assertion or log matcher built against AWS's sentence fails. The count is the actionable part of AWS's message and the emulator never emits it.
  Fix: the DeleteDomain handler's non-empty-domain ConflictException message construction in the cloudwatchomni service crate (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend) — emit AWS's fixed text with the request's domainId and the live associated-space count from state.
  Traces: cloudwatchomni-domain-and-space-singleton-lifecycle#task#6 (DeleteDomain, 409 expected and returned, $.message value_mismatch).

Cause 2 of 2: UpdateDomain's ResourceNotFoundException message is re-worded and echoes the requested domainId (fixture cloudwatchomni-domain-validation-and-notfound-errors, step task#4).
  AWS: Smithy UpdateDomain declares ResourceNotFoundException (errorKind client, @httpError 404); the message text is recorded behavior only. Updating a domain that does not exist is refused with HTTP 404 and the message "Domain not found" — wholly FIXED text with NO variable part: the requested domainId does not appear in it.
  Emulator: the refusal is HTTP 404 with message "Domain '<domainId>' does not exist." — different fixed text plus the request's domainId interpolated into an error AWS keeps opaque.
  Why: a client that classifies the failure on AWS's text (idempotent update-if-exists wrappers and contract tests commonly do) does not recognise the emulator's sentence, and the emulator reflects caller-supplied input back in a message AWS deliberately keeps free of it. It is also inconsistent with the same service's sibling not-found template for the read path.
  Fix: the UpdateDomain handler's domain-lookup failure path — use the shared domain-not-found formatter and AWS's fixed message "Domain not found" with no interpolation (the same formatter the Get-style reads need per the related open issue).
  Traces: cloudwatchomni-domain-validation-and-notfound-errors#task#4 (UpdateDomain, 404 expected and returned, $.message value_mismatch).

## Evidence
Cause 1 — fixture cloudwatchomni-domain-and-space-singleton-lifecycle, step task#6, operation DeleteDomain, verdict mismatch, expected status 409 and actual status 409. Replay detail:
  $.message: value_mismatch (expected 'Cannot delete Domain d-cy5xn789rb9xgj6plvuxmdq3q because it has 1 associated Spaces. Delete all Spaces first.', got "Domain 'd-cy5xn789rb9xgj6plvuxmdq3q' still contains spaces and cannot be deleted.")
Fixed text required: "Cannot delete Domain " + <domainId> + " because it has " + <n> + " associated Spaces. Delete all Spaces first.". Variables: the request's domainId and the count of spaces currently associated with that domain, read from state. The recorded id d-cy5xn789rb9xgj6plvuxmdq3q and count 1 are this run's values, not part of the template.

Cause 2 — fixture cloudwatchomni-domain-validation-and-notfound-errors, step task#4, operation UpdateDomain, verdict mismatch, expected status 404 and actual status 404. Replay detail:
  $.message: value_mismatch (expected 'Domain not found', got "Domain 'd-a2e8666e5a684b0cd608f8' does not exist.")
Fixed text required: the whole sentence 'Domain not found', with NO variable part. The emulator's quoted domainId is a request value that must not appear.

Smithy cited: DeleteDomain @idempotent, input domainId only, errors AccessDenied 403, ConflictException (client) @httpError 409, InternalServer 500, Throttling 429, Validation 400. UpdateDomain errors include ResourceNotFoundException (client) @httpError 404 and ValidationException 400. Neither shape fixes its message text, so both AWS wordings above are recorded behavior only; the statuses and error types the emulator returns are already correct.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it, then replay the wire corpus.
2. Cause 1 — fixture 'cloudwatchomni-domain-and-space-singleton-lifecycle' step task#6: with a domain that still holds at least one space, call DeleteDomain(domainId=<that domain>). Expected HTTP 409 ConflictException with message "Cannot delete Domain <domainId> because it has <n> associated Spaces. Delete all Spaces first." where <n> is the live space count. Actual: HTTP 409 with "Domain '<domainId>' still contains spaces and cannot be deleted.".
3. Cause 2 — fixture 'cloudwatchomni-domain-validation-and-notfound-errors' step task#4: call UpdateDomain(domainId=<a well-formed but non-existent d-... id>). Expected HTTP 404 ResourceNotFoundException with message exactly 'Domain not found'. Actual: HTTP 404 with "Domain '<domainId>' does not exist.".
4. Re-replay both steps; the statuses already match, so only the $.message comparison closes them.
