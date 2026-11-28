Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.get-asset-property-aggregates-rejects-alias-alongside-asset-and-property-ids iotsitewise.get-asset-property-value-history-rejects-an-alias-beside-both-ids iotsitewise.get-asset-property-value-rejects-an-alias-beside-both-ids iotsitewise.get-asset-property-value-rejects-an-asset-id-without-its-property-id iotsitewise.get-asset-property-aggregates-rejects-max-results-above-2500 iotsitewise.get-asset-property-aggregates-rejects-start-date-not-before-end-date iotsitewise.get-interpolated-asset-property-values-rejects-a-start-not-before-its-end iotsitewise.get-asset-property-value-history-rejects-a-window-not-opening-before-it-closes iotsitewise.get-interpolated-asset-property-values-rejects-an-unsupported-interpolation-type iotsitewise.interpolation-window-narrower-than-the-reading-gap-yields-no-value iotsitewise.invoke-assistant-uppercase-conversation-id-rejected iotsitewise.list-assets-all-without-model-id-rejected iotsitewise.bulk-import-adaptive-csv-values-readable-after-completion iotsitewise.bulk-import-row-for-unknown-asset-completes-with-failures iotsitewise.bulk-import-concurrent-same-name-admits-one-job-rejected iotsitewise.delete-component-model-in-use-rejected
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.get-asset-property-aggregates-rejects-alias-alongside-asset-and-property-ids|run#18|AssertionError|assert all_three == (
Issue-Key: iotsitewise.get-asset-property-value-history-rejects-an-alias-beside-both-ids|run#24|AssertionError|assert got == ("InvalidRequestException", _ALL_THREE), (
Issue-Key: iotsitewise.get-asset-property-value-rejects-an-alias-beside-both-ids|run#11|AssertionError|assert refused == (
Issue-Key: iotsitewise.get-asset-property-value-rejects-an-asset-id-without-its-property-id|run#10|AssertionError|assert refused == (
Issue-Key: iotsitewise.get-asset-property-aggregates-rejects-max-results-above-2500|run#14|AssertionError|assert over == (
Issue-Key: iotsitewise.get-asset-property-aggregates-rejects-start-date-not-before-end-date|_refusal#13|AssertionError|raise AssertionError(f"the request was accepted, answering {got!r}")
Issue-Key: iotsitewise.get-interpolated-asset-property-values-rejects-a-start-not-before-its-end|_refusal#8|AssertionError|raise AssertionError(f"the request was accepted and answered {answer!r}")
Issue-Key: iotsitewise.get-asset-property-value-history-rejects-a-window-not-opening-before-it-closes|run#18|AssertionError|assert got == ("InvalidRequestException", _WINDOW_MSG), (
Issue-Key: iotsitewise.get-interpolated-asset-property-values-rejects-an-unsupported-interpolation-type|run#12|AssertionError|assert refused == ("InvalidRequestException",
Issue-Key: iotsitewise.interpolation-window-narrower-than-the-reading-gap-yields-no-value|run#10|AssertionError|assert t0 + 120 in stamped, (
Issue-Key: iotsitewise.invoke-assistant-uppercase-conversation-id-rejected|run#6|AssertionError|assert message.startswith(_PREFIX) and message.endswith(_SUFFIX), (
Issue-Key: iotsitewise.list-assets-all-without-model-id-rejected|run#14|AssertionError|assert explicit == needs_model, f"filter ALL with no model id was refused with {explicit!r}"
Issue-Key: iotsitewise|CreateBulkImportJob|InvalidRequestException
Issue-Key: iotsitewise.bulk-import-concurrent-same-name-admits-one-job-rejected|run#33|AssertionError|assert len(winners) == 1, f"{len(winners)} of 5 concurrent creates with one name were accepted"
Issue-Key: iotsitewise.delete-component-model-in-use-rejected|run#17|AssertionError|assert got == ("InvalidRequestException", "Component Asset Model is being used."), (
Issue-Commit: 794b6570c5e12ac0c4a18da454471fcedcd24ce4

# Single-read validation and control-plane refusals diverge: property-identification and window messages, windows with start == end accepted, interpolation gaps, InvokeAssistant/ListAssets wording, and CreateBulkImportJob refused outright

> _Note: this issue was identified against the code base at commit `794b6570c5e12ac0c4a18da454471fcedcd24ce4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAssetPropertyValue

## Description
This issue bundles 10 distinct root causes outside the batch surface. A fixer must close ALL TEN before submitting; each cause names its own failing checks.

Cause 1 of 10: the property-identification refusals on the single-property reads use emulator-authored text.
AWS: GetAssetPropertyValue, GetAssetPropertyValueHistory and GetAssetPropertyAggregates refuse two distinct mistakes with two distinct fixed messages, both 400 InvalidRequestException (Smithy: assetId/propertyId ID, propertyAlias AssetPropertyAlias, all optional query params; InvalidRequestException client 400): naming propertyAlias together with assetId and propertyId gives "Either an alias can be provided OR both property ID and asset ID. All three cannot be provided."; naming an assetId with no propertyId — or naming nothing at all — gives "Both property ID and asset ID are required. Alternatively, an alias must be provided if the IDs are absent." Both are entirely fixed text (recorded behavior).
Emulator: answers InvalidRequestException "Specify either assetId and propertyId, or propertyAlias." for BOTH cases, collapsing two different diagnoses into one emulator-authored string.
Why: a client cannot tell "you over-specified" from "you under-specified", and neither message matches AWS.
Fix: the property-identification validation shared by the three single-property read handlers.
Traces: iotsitewise.get-asset-property-aggregates-rejects-alias-alongside-asset-and-property-ids, iotsitewise.get-asset-property-value-history-rejects-an-alias-beside-both-ids, iotsitewise.get-asset-property-value-rejects-an-alias-beside-both-ids, iotsitewise.get-asset-property-value-rejects-an-asset-id-without-its-property-id.

Cause 2 of 10: maxResults above the cap is not reported as a constraint violation.
AWS: GetAssetPropertyAggregates with maxResults above 2500 is refused 400 InvalidRequestException with the standard constraint message "1 validation error detected: Value '<maxResults>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 2500" — fixed text with the submitted value and the limit 2500 (the Smithy model declares only range min 1, so the 2500 ceiling and this wording are recorded behavior).
Emulator: InvalidRequestException "maxResults is at most 2500, but this request asks for 2501."
Why: callers (and SDK/CLI users) expect the "N validation error(s) detected: Value ... failed to satisfy constraint" family for input-constraint failures; the emulator's prose is unrecognisable to anything parsing it.
Fix: the GetAssetPropertyAggregates handler's maxResults validation (emit the standard constraint-violation message).
Traces: iotsitewise.get-asset-property-aggregates-rejects-max-results-above-2500.

Cause 3 of 10: a window whose start is not before its end is ACCEPTED by the aggregate and interpolated reads.
AWS: GetAssetPropertyAggregates and GetInterpolatedAssetPropertyValues require the window to open strictly before it closes; startDate == endDate (and start after end) are refused 400 InvalidRequestException "Start date must be before end date." (fixed text; recorded behavior). Nothing is returned.
Emulator: answers 200. GetAssetPropertyAggregates with startDate == endDate returned {'aggregatedValues': []}, and GetInterpolatedAssetPropertyValues with startTimeInSeconds == endTimeInSeconds returned an interpolated point.
Why: a request AWS rejects succeeds here, and worse the interpolated read invents a value for an empty interval, so a client's validation bug stays invisible and the data it reads is fabricated.
Fix: the window validation in the GetAssetPropertyAggregates and GetInterpolatedAssetPropertyValues handlers, before any aggregation/interpolation runs.
Traces: iotsitewise.get-asset-property-aggregates-rejects-start-date-not-before-end-date, iotsitewise.get-interpolated-asset-property-values-rejects-a-start-not-before-its-end.

Cause 4 of 10: GetAssetPropertyValueHistory's window message.
AWS: the same rule on the history read is refused 400 InvalidRequestException with the same fixed message "Start date must be before end date.", both for a reversed window and for one whose start equals its end.
Emulator: refuses (so the rule is enforced) but with "endDate must not precede startDate."
Why: message-keyed clients break; the three reads also disagree with each other.
Fix: the GetAssetPropertyValueHistory handler's window validation (share the constant with Cause 3).
Traces: iotsitewise.get-asset-property-value-history-rejects-a-window-not-opening-before-it-closes.

Cause 5 of 10: the unsupported-interpolation-type message.
AWS: GetInterpolatedAssetPropertyValues with a type other than LINEAR_INTERPOLATION or LOCF_INTERPOLATION is refused 400 InvalidRequestException with "Given interpolation type <type> is not supported" — fixed text with the submitted type as its only variable part (Smithy: type InterpolationType @length 1-256, a free string, so the supported set and the message are recorded behavior).
Emulator: "The type must be LINEAR_INTERPOLATION or LOCF_INTERPOLATION, not SPLINE_INTERPOLATION."
Why: wording mismatch on a validation error clients commonly surface verbatim.
Fix: the interpolation-type validation in the GetInterpolatedAssetPropertyValues handler.
Traces: iotsitewise.get-interpolated-asset-property-values-rejects-an-unsupported-interpolation-type.

Cause 6 of 10: an interval whose window reaches no reading is omitted instead of returned without a value.
AWS: GetInterpolatedAssetPropertyValues returns one entry per interval across the requested range; when intervalWindowInSeconds is too narrow to reach any reading, the interval is still listed, stamped at its own timestamp, carrying no value (recorded behavior; interpolatedAssetPropertyValues is a required output list).
Emulator: returns an empty interpolatedAssetPropertyValues list — the interval at the offset is missing entirely (a 30 s window over a 240 s reading gap produced []), while the same query with a wide window returns the interpolated point.
Why: a client aligning interpolated series to fixed intervals loses the slot instead of seeing a gap, so series silently shift or shorten.
Fix: the interpolation state/serializer in the GetInterpolatedAssetPropertyValues handler — emit every interval and omit only the value.
Traces: iotsitewise.interpolation-window-narrower-than-the-reading-gap-yields-no-value.

Cause 7 of 10: InvokeAssistant's conversationId pattern refusal neither quotes the value nor prints the pattern as AWS does.
AWS: a conversationId that violates its pattern is refused HTTP 400 InvalidRequestException with "1 validation error detected: Value '<conversationId as sent>' at 'conversationId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}" — the submitted value is quoted and the pattern is printed WITHOUT the ^ and $ anchors (Smithy: ConversationId @length 36 and pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$; the rendering is the standard constraint message).
Emulator: answers "1 validation error detected: Value at 'conversationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$" — the "Value '<x>'" clause has lost the value entirely and the pattern is printed with the anchors.
Why: the caller cannot see which value was rejected (the one thing the message exists to report), and the anchored pattern does not match AWS's text.
Fix: the constraint-violation message renderer used by the InvokeAssistant handler's validation (quote the offending value; strip the anchors when printing a pattern).
Traces: iotsitewise.invoke-assistant-uppercase-conversation-id-rejected.

Cause 8 of 10: ListAssets' missing-model refusal message.
AWS: ListAssets with filter=ALL (and with no filter at all, since ALL is the default) but no assetModelId is refused 400 InvalidRequestException with the fixed message "AssetModelID is required when listing ALL assets." (Smithy: ListAssetsFilter = ALL|TOP_LEVEL, assetModelId optional; the message is recorded behavior).
Emulator: "assetModelId is required when filtering by ALL."
Why: wording mismatch, including the capitalisation of AssetModelID, on a refusal clients surface verbatim.
Fix: the ListAssets handler's filter/model-id validation.
Traces: iotsitewise.list-assets-all-without-model-id-rejected.

Cause 9 of 10: CreateBulkImportJob refuses every job with a storage-tier precondition, so bulk import is unusable.
AWS: CreateBulkImportJob accepts a job that names jobName, jobRoleArn, files, errorReportLocation and a csv jobConfiguration and answers HTTP 202 with jobId, jobName and jobStatus (Smithy: CreateBulkImportJob http 202, output jobId/jobName/jobStatus required) without requiring any prior storage-configuration change; the job then runs and its rows become readable (or report per-row failures). A second CONCURRENT create with the same jobName is the only refusal: ResourceAlreadyExistsException "Bulk Import Job <jobName> in use", with exactly one of the racing creates winning.
Emulator: refuses every CreateBulkImportJob with InvalidRequestException "Before you create a bulk import job that ingests data into time series outside of a workspace, you must enable IoT SiteWise warm tier or IoT SiteWise cold tier." — in the concurrency check 0 of 5 creates were admitted, so the name-conflict behavior cannot be reached at all.
Why: the whole bulk-import path is closed: no job can be created, no imported data can be read, and the one-winner/ResourceAlreadyExistsException contract is unobservable.
Fix: the CreateBulkImportJob handler — drop the unconditional tier precondition, create the job (202 + jobStatus), and enforce the concurrent same-name conflict instead.
Traces: iotsitewise.bulk-import-adaptive-csv-values-readable-after-completion, iotsitewise.bulk-import-row-for-unknown-asset-completes-with-failures, iotsitewise.bulk-import-concurrent-same-name-admits-one-job-rejected.

Cause 10 of 10: deleting a component asset model that a composite model still uses reports internal ids instead of AWS's message.
AWS: DeleteAssetModel on an asset model that is composed into another model (via CreateAssetModelCompositeModel with composedAssetModelId) is refused 400 InvalidRequestException with the fixed message "Component Asset Model is being used." — no ids, no variable part — and the model stays ACTIVE with its composition unchanged (Smithy: DeleteAssetModel errors include InvalidRequestException client 400; message is recorded behavior).
Emulator: refuses with InvalidRequestException "Asset model <id> is referenced by composite model <id> of asset model <id> when delete was called" — an emulator-authored sentence built from three internal ids.
Why: a client keyed on the in-use refusal does not recognise it; the ids are per-run values AWS does not emit here.
Fix: the DeleteAssetModel handler's in-use (composition) check message.
Traces: iotsitewise.delete-component-model-in-use-rejected.

Related open issues (neither covers the above): 20261029T194801Z-iotsitewise-asset-hierarchy-refusals-answer-the-wrong-error-class.md (AssociateAssets/DisassociateAssets/DeleteAsset refusals) and 20261109T115126Z-iotsitewise-deleteassetmodel-accepts-a-delete-on-a-model-still.md (DeleteAssetModel on a CREATING model).

## Evidence
Cause 1 — expected ('InvalidRequestException', 'Either an alias can be provided OR both property ID and asset ID. All three cannot be provided.') for alias+both ids and ('InvalidRequestException', 'Both property ID and asset ID are required. Alternatively, an alias must be provided if the IDs are absent.') for assetId-only/nothing; got ('InvalidRequestException', 'Specify either assetId and propertyId, or propertyAlias.') in all four cases.
Cause 2 — expected ('InvalidRequestException', "1 validation error detected: Value '2501' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 2500"); got ('InvalidRequestException', 'maxResults is at most 2500, but this request asks for 2501.').
Cause 3 — GetAssetPropertyAggregates with startDate == endDate returned 200 {'aggregatedValues': []}; GetInterpolatedAssetPropertyValues with start == end returned 200 {'interpolatedAssetPropertyValues': [{'timestamp': {...}, 'value': {'doubleValue': 70.0}}]}. Both expected ('InvalidRequestException', 'Start date must be before end date.').
Cause 4 — expected ('InvalidRequestException', 'Start date must be before end date.'); got ('InvalidRequestException', 'endDate must not precede startDate.').
Cause 5 — expected ('InvalidRequestException', 'Given interpolation type SPLINE_INTERPOLATION is not supported'); got ('InvalidRequestException', 'The type must be LINEAR_INTERPOLATION or LOCF_INTERPOLATION, not SPLINE_INTERPOLATION.').
Cause 6 — with intervalWindowInSeconds=30 over a reading gap the interval at offset 120 s was absent: interpolatedAssetPropertyValues == []; expected that interval present with no value (the same query with intervalWindowInSeconds=240 returned the interpolated 35.0).
Cause 7 — expected "1 validation error detected: Value '<the uppercase UUID sent>' at 'conversationId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"; got "1 validation error detected: Value at 'conversationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$" (status 400 / InvalidRequestException were correct).
Cause 8 — expected ('InvalidRequestException', 'AssetModelID is required when listing ALL assets.'); got ('InvalidRequestException', 'assetModelId is required when filtering by ALL.').
Cause 9 — CreateBulkImportJob answered InvalidRequestException 'Before you create a bulk import job that ingests data into time series outside of a workspace, you must enable IoT SiteWise warm tier or IoT SiteWise cold tier.' where a 202 with jobId/jobName/jobStatus was recorded; the concurrency check reported "0 of 5 concurrent creates with one name were accepted" instead of 1 winner plus 4 x ('ResourceAlreadyExistsException', 'Bulk Import Job <jobName> in use').
Cause 10 — expected ('InvalidRequestException', 'Component Asset Model is being used.'); got ('InvalidRequestException', 'Asset model dba8b7c2-99ae-4ceb-854c-745e7e520b19 is referenced by composite model 62719d85-7ec3-4f0f-9d74-e4d9cd2a2a1b of asset model 7c70fc91-e024-4b88-98df-20b434616a3d when delete was called') — the three UUIDs are that run's values; AWS's message carries none.

## Steps to Reproduce
1. Create an asset model with a DOUBLE measurement property, create an asset, wait for ACTIVE, ingest GOOD readings.
2. Cause 1: call GetAssetPropertyValue / GetAssetPropertyValueHistory / GetAssetPropertyAggregates with assetId+propertyId+propertyAlias, then with assetId only, then with nothing.
3. Cause 2: GetAssetPropertyAggregates with maxResults=2501.
4. Cause 3: GetAssetPropertyAggregates with startDate == endDate; GetInterpolatedAssetPropertyValues with startTimeInSeconds == endTimeInSeconds.
5. Cause 4: GetAssetPropertyValueHistory with endDate before startDate, and with startDate == endDate.
6. Cause 5: GetInterpolatedAssetPropertyValues with type='SPLINE_INTERPOLATION'.
7. Cause 6: ingest two readings 240 s apart and call GetInterpolatedAssetPropertyValues with intervalInSeconds=60 and intervalWindowInSeconds=30 over the gap; inspect interpolatedAssetPropertyValues.
8. Cause 7: InvokeAssistant with conversationId set to an upper-case UUID.
9. Cause 8: ListAssets(filter='ALL') with no assetModelId, and ListAssets() with no arguments.
10. Cause 9: put a CSV in S3, create a job role, then CreateBulkImportJob with files/errorReportLocation/csv jobConfiguration; also fire five concurrent creates sharing one jobName.
11. Cause 10: create two asset models, CreateAssetModelCompositeModel on the parent with composedAssetModelId = the component, then DeleteAssetModel on the component.
