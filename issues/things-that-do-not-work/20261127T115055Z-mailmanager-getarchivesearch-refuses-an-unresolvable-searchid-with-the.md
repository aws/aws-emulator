Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search|run#13|AssertionError|assert message == "Search not found.", f"unexpected message {message!r}"
Issue-Commit: 93482e4439cf9a7b8a761a5b6e98393d2d44ec92

# mailmanager GetArchiveSearch refuses an unresolvable SearchId with the emulator-authored message "Invalid SearchId" instead of AWS's "Search not found."

> _Note: this issue was identified against the code base at commit `93482e4439cf9a7b8a761a5b6e98393d2d44ec92`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetArchiveSearch

## Description
One net-new root cause.

Cause 1 of 1: the ValidationException message GetArchiveSearch returns for a SearchId that no longer resolves is emulator-authored wording, not AWS's.
Failing check ids: mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search

AWS: GetArchiveSearch takes a required SearchId (Smithy: GetArchiveSearchRequest$SearchId, @length 1..64) and models no not-found error — its only client errors are AccessDeniedException (403), ThrottlingException (429) and ValidationException (400, "The request validation has failed. For details, see the accompanying error message"). The model states no message text, so this is recorded behavior only: when the search's archive has been deleted (DeleteArchive accepted, archive no longer serves job operations) a later GetArchiveSearch on a SearchId that had already reached COMPLETED is refused with HTTP 400, Error.Code "ValidationException" and Error.Message "Search not found." — the whole message is fixed text for this error class, with no variable part: AWS echoes neither the SearchId nor the ArchiveId back.

Emulator: the same call is refused with the right status and code (ValidationException/400) but the message "Invalid SearchId". The constant is defined in the service state as SEARCH_ID_INVALID = "Invalid SearchId" and returned by the handler when resolve_search_job() yields None, under a deliberate house rule that every unresolvable-identifier refusal in this service reads "Invalid " + the input member's name. That rule does not match what AWS words for this operation: the recorded refusal names the missing SEARCH ("Search not found."), not the member.

Why: a client that distinguishes refusal classes by message — AWS gives GetArchiveSearch no not-found error code, so the message is the only signal that separates "this search is gone" from "this SearchId is malformed/unacceptable" — misclassifies the gone-search case. Retry/cleanup logic keyed on the documented sentence never fires, and assertions or log-scrapers written against real AWS text fail against the emulator even though the status and code agree.

Fix: the refusal message in the GetArchiveSearch handler's unresolvable-SearchId branch (and the shared state constant it uses), not the resolution logic — resolve_search_job() already correctly treats a job whose archive is deleted or PENDING_DELETION as unreachable, and the status/code are already right. Note the shared constant is also used for the import-job read's JobId; only the search read is grounded in a failing check here, so scope the change to the message GetArchiveSearch serves rather than relabelling every member-named refusal.

Not asserted by this run: the check's next assertion (ListArchiveSearches on the deleted archive must give ValidationException "Archive is not active.") was never reached because the run stopped on the GetArchiveSearch message, so this issue makes no claim about it; re-replaying the listed check after the fix exercises it.

Traces: mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search.

## Evidence
Check mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search (verdict: mismatch).

Sequence: CreateArchive -> StartArchiveSearch(FromTimestamp=now-1d, ToTimestamp=now, MaxResults=10) -> poll GetArchiveSearch until Status.State == COMPLETED (positive control passes; GetArchiveSearch also reads back ArchiveId == the created archive) -> DeleteArchive(ArchiveId) -> GetArchiveSearch(SearchId=<the completed search>).

Expected (AWS, recorded): HTTP 400, Error.Code = "ValidationException", Error.Message = "Search not found." (entirely fixed text; no request or state values interpolated).
Actual (emulator): HTTP 400, Error.Code = "ValidationException" (matches), Error.Message = "Invalid SearchId".

Runner assertion:
  assert message == "Search not found.", f"unexpected message {message!r}"
  AssertionError: unexpected message 'Invalid SearchId'

Source of the diverging text in the checkout:
  services/service-com-amazonaws-mailmanager-mailmanagersvc/src/state.rs:676 — pub const SEARCH_ID_INVALID: &str = "Invalid SearchId";
  services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/get_archive_search.rs:170-175 — resolve_search_job() returns None -> ValidationException { message: SEARCH_ID_INVALID }.
  services/service-com-amazonaws-mailmanager-mailmanagersvc/src/state.rs:991 — resolve_search_job filters out a job whose archive fails check_archive_for_job (deleted or not ACTIVE).

Smithy (services/service-com-amazonaws-mailmanager-mailmanagersvc/smithy-model/com-amazonaws-mailmanager-mailmanagersvc.json): GetArchiveSearch input SearchId required @length 1..64; errors = AccessDeniedException(403), ThrottlingException(429), ValidationException(400) — no not-found error is modeled, so the message carries the distinction.

## Steps to Reproduce
1. mm = client("mailmanager").
2. arc_id = mm.create_archive(ArchiveName=<unique>)["ArchiveId"].
3. search_id = mm.start_archive_search(ArchiveId=arc_id, FromTimestamp=now-1day, ToTimestamp=now, MaxResults=10)["SearchId"].
4. Poll mm.get_archive_search(SearchId=search_id) until Status.State == "COMPLETED" (empty archive completes immediately).
5. mm.delete_archive(ArchiveId=arc_id).
6. Call mm.get_archive_search(SearchId=search_id) and inspect the ClientError: Error.Code is "ValidationException" as AWS gives, but Error.Message is "Invalid SearchId" where AWS returns "Search not found.".

Replay: mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search
