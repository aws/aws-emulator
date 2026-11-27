Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-get-asset-content-input-validation
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-get-asset-content-input-validation devops-agent-get-asset-file-input-bounds devops-agent-list-asset-files-paging-bounds devops-agent-list-asset-files-unknown-asset-and-version devops-agent-list-asset-types-page-size-bounds devops-agent-list-assets-filter-and-paging-validation devops-agent-list-chats-max-results-bounds devops-agent-list-chats-page-size-and-deleted-space-refusals devops-agent-list-goals-filter-validation devops-agent-list-journal-records-input-validation
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|GetAssetContent|4xx->4xx|$.fieldList
Issue-Key: devops-agent|GetAssetContent|4xx->4xx|$.message
Issue-Key: devops-agent|GetAssetFile|4xx->4xx|$.fieldList
Issue-Key: devops-agent|GetAssetFile|4xx->4xx|$.message
Issue-Key: devops-agent|ListAssetFiles|4xx->4xx|$.fieldList
Issue-Key: devops-agent|ListAssetFiles|4xx->4xx|$.message
Issue-Key: devops-agent|ListAssetTypes|4xx->4xx|$.fieldList
Issue-Key: devops-agent|ListAssetTypes|4xx->4xx|$.message
Issue-Key: devops-agent|ListAssets|4xx->4xx|$.fieldList
Issue-Key: devops-agent|ListAssets|4xx->4xx|$.message
Issue-Key: devops-agent|ListChats|4xx->4xx|$.fieldList
Issue-Key: devops-agent|ListChats|4xx->4xx|$.message
Issue-Key: devops-agent|ListGoals|4xx->4xx|$.fieldList
Issue-Key: devops-agent|ListGoals|4xx->4xx|$.message
Issue-Key: devops-agent|ListJournalRecords|4xx->4xx|$.fieldList
Issue-Key: devops-agent|ListJournalRecords|4xx->4xx|$.message
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# devops-agent range-constraint ValidationException omits fieldList and uses non-AWS message wording

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAssetContent, GetAssetFile, ListAssetFiles, ListAssetTypes, ListAssets, ListChats, ListGoals, ListJournalRecords

## Description
Eight net-new wire divergences in devops-agent that share ONE fault: when an input member violates a modeled @range constraint the emulator returns the right HTTP status (400) and the right error code (ValidationException) but the wrong error BODY — the modeled `fieldList` member is absent, and `message` is worded differently from AWS. Each cause below is a separate replay cluster on a separate operation/member and a fixer must close all eight before submitting.

=== Shared requirement (applies to all eight causes) ===
AWS: Smithy: ValidationException { message: String (required) — "A summary of the validation failure"; fieldList: ValidationExceptionFieldList of ValidationExceptionField { path: String (required) — "A JSONPointer expression to the structure member whose value failed to satisfy the modeled constraints"; message: String (required) } }, errorKind client, httpError 400; ValidationException is modeled on every operation named below. Recorded behavior: AWS populates fieldList with one entry per violated constraint and words the two strings as
  message       = "<N> validation error detected. <the field message>"
  field.path    = "/<memberName>"
  field.message = "Value at '/<memberName>' failed to satisfy constraint: Member must be <constraint phrase>"
Fixed text of that class: " validation error detected. " (a PERIOD after "detected"), "Value at '", "' failed to satisfy constraint: Member must be ". Variable parts, taken from the request and from the model: N = the number of violations; the member's JSON pointer (leading '/', used even for @httpQuery- and body-bound members); and the constraint phrase read off the model — a @range carrying BOTH bounds reads "between <min> and <max>, inclusive", a @range carrying only min reads "greater than or equal to <min>". AWS does NOT echo the offending value.
Emulator: `fieldList` is missing from the body altogether, and message reads "<N> validation error detected: Value '<the submitted value>' at '<memberName>' failed to satisfy constraint: Member must have value <one-sided phrase>" — five wording divergences at once: a COLON instead of the period, the submitted value echoed back, the member named WITHOUT the leading '/', "Member must have value ..." instead of "Member must be ...", and only the single bound that was crossed instead of AWS's inclusive-range phrase.
Why: a client that reads ValidationException.fieldList to point a user at the offending input field (the AWS SDKs surface it, and it is the only machine-readable part of the error) gets null from the emulator and must fall back to scraping prose; and any client, test or operator matching AWS's message text does not recognize the emulator's. The emulator additionally leaks the submitted value into an error string where AWS deliberately does not.
Fix: the shared constraint-validation stage of the request walk (deserialize -> validate -> handle -> serialize) in the devops-agent service crate, and the ValidationException serializer it feeds: build one ValidationExceptionField per violated constraint with the JSON-pointer path, render the constraint phrase from the modeled @range (both bounds -> "between X and Y, inclusive"), compose the summary with "<N> validation error detected. ", and stop echoing the submitted value.

=== Cause 1 of 8: GetAssetContent.assetVersion @range min 1 ===
AWS: Smithy: GetAssetContent input assetVersion -> Integer, @httpQuery("assetVersion"), range {min: 1}. A value below 1 is refused 400 ValidationException with field path "/assetVersion" and the phrase "Member must be greater than or equal to 1".
Emulator: 400 ValidationException, fieldList null, message "1 validation error detected: Value '0' at 'assetVersion' failed to satisfy constraint: Member must have value greater than or equal to 1".
Traces: devops-agent-get-asset-content-input-validation#task#4 (cluster also covers task#5, task#6, task#7 of the same fixture).

=== Cause 2 of 8: GetAssetFile.assetVersion @range min 1 ===
AWS: Smithy: GetAssetFile input assetVersion -> Integer, @httpQuery, range {min: 1}; same envelope as cause 1 with path "/assetVersion".
Emulator: fieldList null and the same "Member must have value greater than or equal to 1" wording.
Traces: devops-agent-get-asset-file-input-bounds#task#4 (cluster also covers task#7, task#8).

=== Cause 3 of 8: ListAssetFiles.maxResults @range 1..100 ===
AWS: Smithy: ListAssetFiles input maxResults -> Integer, @httpQuery("maxResults"), range {min: 1, max: 100}; @paginated pageSize. Out-of-range is refused with path "/maxResults" and "Member must be between 1 and 100, inclusive" — the SAME phrase whether the low or the high bound was crossed.
Emulator: fieldList null and a one-sided phrase, e.g. "Member must have value less than or equal to 100".
Traces: devops-agent-list-asset-files-paging-bounds#task#4 (cluster also covers task#5, task#7 and devops-agent-list-asset-files-unknown-asset-and-version#task#6).

=== Cause 4 of 8: ListAssetTypes.maxResults @range 1..100 ===
AWS: Smithy: ListAssetTypes input maxResults -> Integer, @httpQuery, range {min: 1, max: 100}; path "/maxResults", phrase "Member must be between 1 and 100, inclusive".
Emulator: fieldList null; for a value below the minimum it reports the one-sided "Member must have value greater than or equal to 1" instead of the inclusive-range phrase. (An unrelated open issue, 20261127T091917Z-devops-agent-listassettypes-stub.md, calls ListAssetTypes an unimplemented stub; the replay shows it validating input, and this divergence is the validation body, not the missing handler.)
Traces: devops-agent-list-asset-types-page-size-bounds#task#3 (cluster also covers task#4).

=== Cause 5 of 8: ListAssets.maxResults @range 1..100 ===
AWS: Smithy: ListAssets input maxResults -> Integer, @httpQuery, range {min: 1, max: 100}; path "/maxResults", phrase "Member must be between 1 and 100, inclusive".
Emulator: fieldList null, one-sided "Member must have value less than or equal to 100". (See also the separate open stub issue 20261127T091917Z-devops-agent-listassets-stub.md, which does not cover this error body.)
Traces: devops-agent-list-assets-filter-and-paging-validation#task#7 (cluster also covers task#8).

=== Cause 6 of 8: ListChats.maxResults @range 1..20 ===
AWS: Smithy: ListChats input maxResults -> Integer, @httpQuery("maxResults"), range {min: 1, max: 20}. Note the limit is 20, not 100, so the phrase is "Member must be between 1 and 20, inclusive" at path "/maxResults".
Emulator: fieldList null, one-sided "Member must have value less than or equal to 20".
Traces: devops-agent-list-chats-max-results-bounds#task#4 (cluster also covers task#5 and devops-agent-list-chats-page-size-and-deleted-space-refusals#task#3, #task#4).

=== Cause 7 of 8: ListGoals.limit @range 1..100 ===
AWS: Smithy: ListGoals input limit -> Integer, range {min: 1, max: 100}, default 50, @paginated pageSize; it is a BODY member of the POST /backlog/agent-space/{agentSpaceId}/goals/list request, and AWS still names it with a JSON pointer, path "/limit", phrase "Member must be between 1 and 100, inclusive".
Emulator: fieldList null, message names the member as 'limit' without the pointer slash and uses the one-sided phrase. (Open stub issue 20261127T091917Z-devops-agent-listgoals-stub.md is about the missing handler, not this error body.)
Traces: devops-agent-list-goals-filter-validation#task#3 (cluster also covers task#4, task#5, task#6).

=== Cause 8 of 8: ListJournalRecords.limit @range 1..100 ===
AWS: Smithy: ListJournalRecords input limit -> Integer, range {min: 1, max: 100}, default 100, @paginated pageSize, a body member of POST /journal/agent-space/{agentSpaceId}/journalRecords; path "/limit", phrase "Member must be between 1 and 100, inclusive".
Emulator: fieldList null and the one-sided phrase. (Open stub issue 20261127T091917Z-devops-agent-listjournalrecords-stub.md does not cover this error body.)
Traces: devops-agent-list-journal-records-input-validation#task#5 (cluster also covers task#6, task#7).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Status and error code agree in every cause (400 / ValidationException); the body diverges. Per cause, expected (recorded AWS) vs actual (emulator):

Cause 1 — devops-agent-get-asset-content-input-validation#task#4, GetAssetContent:
  expected $.fieldList = [{'path': '/assetVersion', 'message': "Value at '/assetVersion' failed to satisfy constraint: Member must be greater than or equal to 1"}]; actual: None (member absent).
  expected $.message = "1 validation error detected. Value at '/assetVersion' failed to satisfy constraint: Member must be greater than or equal to 1"; actual: "1 validation error detected: Value '0' at 'assetVersion' failed to satisfy constraint: Member must have value greater than or equal to 1".

Cause 2 — devops-agent-get-asset-file-input-bounds#task#4, GetAssetFile: identical expected/actual pair for /assetVersion.

Cause 3 — devops-agent-list-asset-files-paging-bounds#task#4, ListAssetFiles:
  expected $.fieldList = [{'path': '/maxResults', 'message': "Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 100, inclusive"}]; actual: None.
  expected $.message = "1 validation error detected. Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 100, inclusive"; actual: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".

Cause 4 — devops-agent-list-asset-types-page-size-bounds#task#3, ListAssetTypes: same expected pair as cause 3; actual: "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1", fieldList None.

Cause 5 — devops-agent-list-assets-filter-and-paging-validation#task#7, ListAssets: same expected pair as cause 3; actual: "... Value '101' at 'maxResults' ... Member must have value less than or equal to 100", fieldList None.

Cause 6 — devops-agent-list-chats-max-results-bounds#task#4, ListChats:
  expected $.message = "1 validation error detected. Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 20, inclusive" and the matching one-entry fieldList; actual: "1 validation error detected: Value '21' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20", fieldList None.

Cause 7 — devops-agent-list-goals-filter-validation#task#3, ListGoals:
  expected $.fieldList = [{'path': '/limit', 'message': "Value at '/limit' failed to satisfy constraint: Member must be between 1 and 100, inclusive"}] and $.message = "1 validation error detected. Value at '/limit' failed to satisfy constraint: Member must be between 1 and 100, inclusive"; actual: fieldList None, message "1 validation error detected: Value '101' at 'limit' failed to satisfy constraint: Member must have value less than or equal to 100".

Cause 8 — devops-agent-list-journal-records-input-validation#task#5, ListJournalRecords: same expected/actual pair as cause 7 for /limit.

The quoted numbers ('0', '101', '21') are the values the fixture's request carried; the requirement is the template, not those values.

## Steps to Reproduce
Boot the emulator from this checkout and replay the named fixture steps (fixture#step given per cause above). By hand, against a live agent space:
1. GET /asset/agent-space/{agentSpaceId}/assets/{assetId}/content?assetVersion=0 — 400 ValidationException with no fieldList and the 'Member must have value greater than or equal to 1' wording.
2. GET .../assets/{assetId}/files/{path}?assetVersion=0 — same.
3. GET .../assets/{assetId}/files?maxResults=101 — expect 'Member must be between 1 and 100, inclusive' plus a one-entry fieldList at path '/maxResults'.
4. GET /asset/types?maxResults=0 — same inclusive-range phrase expected.
5. GET /asset/agent-space/{agentSpaceId}/assets?maxResults=101 — same.
6. GET /agents/agent-space/{agentSpaceId}/chat/list?maxResults=21 — expect 'between 1 and 20, inclusive'.
7. POST /backlog/agent-space/{agentSpaceId}/goals/list with body {"limit": 101} — expect fieldList path '/limit'.
8. POST /journal/agent-space/{agentSpaceId}/journalRecords with body {"executionId": "<id>", "limit": 101} — same.
In each case inspect the response body for the `fieldList` member and compare `message` character-for-character with the expected text above.
