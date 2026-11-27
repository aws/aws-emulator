Issue-Source: trace-sweep-divergence
Issue-Trace: mailmanager-archive-search-lifecycle
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mailmanager-archive-search-lifecycle
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager|GetArchiveSearch|4xx->4xx|$.Message
Issue-Commit: ea7f0126e9823a5465a827b3d4383cc33bb07ceb

# mailmanager GetArchiveSearch ValidationException message is "Search not found." instead of AWS's "Invalid SearchId"

> _Note: this issue was identified against the code base at commit `ea7f0126e9823a5465a827b3d4383cc33bb07ceb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetArchiveSearch

## Description
Single cause (Cause 1 of 1): the emulator's GetArchiveSearch handler rejects an unresolvable SearchId with its own invented message text rather than the message AWS words for that error class.

AWS: GetArchiveSearch takes a required `SearchId` (Smithy: GetArchiveSearch.SearchId, target SearchId, @required, @length min 1 max 64) and models exactly three errors — AccessDeniedException (HTTP 403), ThrottlingException (HTTP 429) and ValidationException (HTTP 400, "The request validation has failed. For details, see the accompanying error message."). There is no ResourceNotFoundException on this operation, so a SearchId that names no live search job is reported as a *validation* failure. The recorded AWS response for such a SearchId is HTTP 400 with error code `ValidationException` and the body member `Message` set to the fixed text `Invalid SearchId` — that string is wholly fixed text for this error class: it interpolates nothing from the request (it does not echo the supplied SearchId) and nothing from service state.

Emulator: answers the same HTTP 400 / `ValidationException`, but serializes `$.Message` as `Search not found.` — a different fixed string (and a different sentence form, trailing period included). The replayed step reports `$.Message: value_mismatch (expected 'Invalid SearchId', got 'Search not found.')`. The handler takes the not-found branch when `store.resolve_search_job(..)` yields nothing and fills the ValidationException message from the service-state constant `SEARCH_NOT_FOUND`, which carries resource-not-found wording rather than the model's validation wording.

Why: the status and error code match, so a client's error *type* dispatch still works, but any client that branches on, logs, asserts, or surfaces the validation message gets the wrong text. SDK test suites, CLI output, and user-facing error surfaces that quote `Message` show "Search not found." where AWS shows "Invalid SearchId"; consumers that regex the message to distinguish a malformed SearchId from other validation failures on this operation never match. The message is part of the operation's contract, not cosmetic.

Fix: in the GetArchiveSearch handler's unresolvable-SearchId branch, emit ValidationException with AWS's fixed text `Invalid SearchId`. The message constant is shared from service state, so change it there only if every operation that uses it wants the same wording; otherwise give GetArchiveSearch its own validation message so the sibling archive-search operations are not silently re-worded. No status or error-code change is needed — only the message string.

Traces: mailmanager-archive-search-lifecycle#task#8 (GetArchiveSearch, expected 400 / actual 400, `$.Message` mismatch). Emulator code: services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/get_archive_search.rs lines 168-173 (the `resolve_search_job` none-branch returning ValidationException with `crate::state::SEARCH_NOT_FOUND`).

Note: the other open mailmanager issues (ListAddressListImportJobs stub, CreateRuleSet stub) and the cloudformation__mailmanager CreateStack issue cover unrelated faults and do not cover this message divergence.

## Evidence
Replayed step mailmanager-archive-search-lifecycle#task#8, operation GetArchiveSearch, verdict mismatch.
Expected status 400, actual status 400 — status and error code (`ValidationException`) agree.
Body divergence: `$.Message: value_mismatch (expected 'Invalid SearchId', got 'Search not found.')`.
Divergence key: service=mailmanager|GetArchiveSearch|status=400->400|keys=$.Message (cluster size 1).
Smithy model (services/service-com-amazonaws-mailmanager-mailmanagersvc/smithy-model/com-amazonaws-mailmanager-mailmanagersvc.json): GetArchiveSearch errors = AccessDeniedException(403), ThrottlingException(429), ValidationException(400); ValidationException has a single member `Message` (ErrorMessage). No not-found error is modeled for this operation.
Fixed vs variable text: `Invalid SearchId` is entirely fixed text for this error class; it contains no request-derived or state-derived substitution.

## Steps to Reproduce
1. Boot the emulator and replay fixture `mailmanager-archive-search-lifecycle`.
2. At step `task#8`, call mailmanager GetArchiveSearch with a `SearchId` that is syntactically valid (length 1..64) but resolves to no live search job — e.g. an id that was never minted, or one whose archive has been deleted/marked for deletion.
3. Observe the response: HTTP 400 with `ValidationException`.
4. Inspect the JSON body's `Message` member: the emulator returns `Search not found.`; AWS returns `Invalid SearchId`.
