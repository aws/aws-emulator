Issue-Source: trace-sweep-divergence
Issue-Trace: autocomplete-validation-rejected
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autocomplete-validation-rejected search-text-validation-rejected suggest-validation-rejected
Issue-Shape-Id: com.amazonaws.geoplaces#PlacesService
Issue-Key: geo-places|Autocomplete|4xx->4xx|$.fieldList[name="BiasPosition"].message
Issue-Key: geo-places|Autocomplete|4xx->4xx|$.fieldList[name="Filter.BoundingBox"].message
Issue-Key: geo-places|Autocomplete|4xx->4xx|$.fieldList[name="Filter.Circle"]
Issue-Key: geo-places|Autocomplete|4xx->4xx|$.message
Issue-Key: geo-places|Autocomplete|4xx->4xx|$.reason
Issue-Key: geo-places|Autocomplete|4xx->4xx|$.fieldList
Issue-Key: geo-places|SearchText|4xx->4xx|$.fieldList[name="BiasPosition"]
Issue-Key: geo-places|SearchText|4xx->4xx|$.fieldList[name="MaxResults"]
Issue-Key: geo-places|SearchText|4xx->4xx|$.fieldList[name="QueryId"].message
Issue-Key: geo-places|SearchText|4xx->4xx|$.fieldList[name="QueryText"].message
Issue-Key: geo-places|SearchText|4xx->4xx|$.message
Issue-Key: geo-places|SearchText|4xx->4xx|$.reason
Issue-Key: geo-places|SearchText|4xx->2xx|$.fieldList
Issue-Key: geo-places|SearchText|4xx->2xx|$.message
Issue-Key: geo-places|SearchText|4xx->2xx|$.reason
Issue-Key: geo-places|SearchText|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: geo-places|SearchText|4xx->2xx|status
Issue-Key: geo-places|SearchText|4xx->4xx|$.fieldList[name="BiasPosition"].message
Issue-Key: geo-places|SearchText|4xx->4xx|$.fieldList[name="Filter.BoundingBox"].message
Issue-Key: geo-places|SearchText|4xx->4xx|$.fieldList[name="Filter.Circle"]
Issue-Key: geo-places|SearchText|4xx->4xx|$.fieldList
Issue-Key: geo-places|Suggest|4xx->4xx|$.fieldList[name="BiasPosition"].message
Issue-Key: geo-places|Suggest|4xx->4xx|$.fieldList[name="Filter.BoundingBox"]
Issue-Key: geo-places|Suggest|4xx->4xx|$.fieldList[name="Filter.Circle"].message
Issue-Key: geo-places|Suggest|4xx->4xx|$.message
Issue-Key: geo-places|Suggest|4xx->4xx|$.reason
Issue-Key: geo-places|Suggest|4xx->4xx|$.fieldList
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# ValidationException responses omit reason and fieldList and word validation messages unlike AWS

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Autocomplete

## Description
The geo-places input-refusal path diverges from AWS in three systematic ways: the ValidationException envelope loses its two @required members when the failure is caught by the framework boundary validator, the mutual-exclusion/required-one-of rules are worded with a sentence AWS does not use and reported under the wrong ValidationExceptionReason with the wrong FieldList membership, and one required-one-of rule is not enforced at all so an invalid request is answered 200. TEN causes are bundled below; all ten must be closed before this issue is resubmitted.

Shared Smithy grounding: ValidationException (httpError 400) models Message (jsonName "message"), Reason (jsonName "reason", @required, enum ValidationExceptionReason = UnknownOperation|Missing|CannotParse|FieldValidationFailed|Other|UnknownField) and FieldList (jsonName "fieldList", @required, list of ValidationExceptionField{name,message}). Reason and FieldList are @required, so NO ValidationException this service serves may omit them.

--- Cause 1 of 10: Autocomplete mutually-exclusive location fields — wrong message wording, wrong reason, FieldList names fields the request did not set ---
AWS: Autocomplete models BiasPosition, Filter.BoundingBox and Filter.Circle as mutually exclusive (Smithy: AutocompleteRequest$BiasPosition doc "The fields BiasPosition, FilterBoundingBox, and FilterCircle are mutually exclusive"), and none of them is required. A request that sets more than one is refused 400 ValidationException with the fixed text "Only one of the following fields may be set: " followed by the comma-separated, single-quoted names of the mutually exclusive group and a terminating "." — the member names are the model's spellings, the only variable part. reason is "FieldValidationFailed", and fieldList carries one entry per member THE REQUEST ACTUALLY SET (two entries when two were set), each repeating the top-level message.
Emulator: 400 with message "Only one of the following fields must be set: 'BiasPosition', 'Filter.BoundingBox', 'Filter.Circle'." ("must be set" instead of "may be set"), reason "Other" instead of "FieldValidationFailed", and a fieldList entry for ALL THREE alternatives including Filter.Circle, which the request did not set.
Why: a client that branches on reason never sees FieldValidationFailed for a field-level failure, and a client that surfaces fieldList to a user highlights a field the user never filled in; the message text does not match AWS's.
Fix: the Autocomplete handler's mutual-exclusion branch (services/service-com-amazonaws-geoplaces-placesservice/src/ops/autocomplete.rs, areas>1 branch) plus the shared refusal builders state::only_one_of and state::validation_exception_for — the wording helper must emit "may be set" for an optional mutually-exclusive group, the reason must be FieldValidationFailed, and the FieldList must be built from the members present in the request rather than from the whole group.
Traces: autocomplete-validation-rejected#task#2.

--- Cause 2 of 10: Autocomplete enum rejection — boundary-validator wording and no reason/fieldList ---
AWS: Smithy: AutocompleteRequest$IntendedUse targets enum AutocompleteIntendedUse (values: SingleUse). A value outside the enumeration is refused 400 ValidationException with message "'IntendedUse' must contain an enumerated value from AutocompleteIntendedUse" — fixed text "' must contain an enumerated value from " around the member name and the ENUM SHAPE NAME, not the value list — reason "FieldValidationFailed", and fieldList = [{name: the member, message: the same sentence}].
Emulator: 400 with message "'IntendedUse' must be one of: SingleUse" and NO reason and NO fieldList key at all.
Why: the response omits two @required members of the modeled error shape, so a strict SDK deserializer fails or yields a null reason/fieldList; the message names the permitted value list where AWS names the enum type.
Fix: the boundary-refusal adapter (service::EmulatedService::override_refusal together with state::violation_message / state::must_be_one_of) must build a full ValidationException via state::validation_exception(REASON_FIELD_VALIDATION_FAILED, member, …) instead of a message-only error, and the enum wording must name the target enum shape.
Traces: autocomplete-validation-rejected#task#3 (cluster also covers autocomplete-validation-rejected#task#5).

--- Cause 3 of 10: Autocomplete MaxResults above its range — reason and fieldList missing ---
AWS: Smithy: AutocompleteRequest$MaxResults @range min 1 max 20. A larger value is refused 400 ValidationException with reason "FieldValidationFailed" and fieldList = [{name: "MaxResults", message: "'MaxResults' must not be greater than <operation's modeled maximum>"}]; the maximum in the text is the operation's own modeled bound, not a literal.
Emulator: the message matches, but the body carries neither reason nor fieldList.
Why: the @required members of ValidationException are absent, so a client cannot tell which field failed from the structured payload.
Fix: same boundary-refusal adapter as Cause 2 — every framework-caught constraint violation must be rendered as a complete ValidationException with reason FieldValidationFailed and a one-entry FieldList naming the member.
Traces: autocomplete-validation-rejected#task#4.

--- Cause 4 of 10: SearchText QueryId conflicts — only one pair reported, wrong wording, wrong reason ---
AWS: Smithy: SearchTextRequest declares QueryText and QueryId (both optional, mutually exclusive) and QueryId cannot be combined with the query-shaping members (MaxResults, BiasPosition). A request that sets QueryId alongside them is refused 400 ValidationException with reason "FieldValidationFailed"; the top-level message is the pairwise sentence "Only one of the following fields may be set: 'QueryId', 'QueryText'." and fieldList carries ONE ENTRY PER CONFLICTING MEMBER, each worded with its own pair — "Only one of the following fields may be set: 'QueryId', '<member>'." for QueryText, MaxResults and BiasPosition — plus an entry for QueryId itself when the supplied id is not one the service issued, worded "'QueryId' must be a valid ID." The fixed text is the "Only one of the following fields may be set: " prefix and the "' must be a valid ID." rule; the member names come from the request.
Emulator: 400 with message "Only one of the following fields must be set: 'QueryText', 'QueryId'." (wrong verb, QueryId/QueryText order reversed), reason "Other", and a fieldList of only QueryText and QueryId both carrying that one message — no MaxResults entry, no BiasPosition entry, and QueryId's entry does not report the invalid-id rule.
Why: a client is told only about the QueryText/QueryId pair and never learns that MaxResults and BiasPosition must be dropped when resuming a Suggest QueryId, so it retries with the same unusable request.
Fix: the SearchText handler's QueryText/QueryId match arm (src/ops/search_text.rs, the (Some,Some) branch) must accumulate one FieldList entry per conflicting member using the pairwise wording and validate the QueryId itself, and report reason FieldValidationFailed.
Traces: search-text-validation-rejected#task#2.

--- Cause 5 of 10: SearchText accepts a request that sets none of BiasPosition/Filter.BoundingBox/Filter.Circle (200 instead of 400) ---
AWS: Smithy: SearchTextRequest$BiasPosition doc requires exactly one of BiasPosition, Filter.BoundingBox or Filter.Circle. A request that sets NONE of them is refused 400 ValidationException, reason "FieldValidationFailed", message "Exactly one of the following fields must be set: " + the quoted group + ".", with one fieldList entry per member of the group (all three, because any one of them could have satisfied the rule). An x-amzn-errortype header accompanies the error.
Emulator: answers 200 with a normal SearchTextResponse — the handler only checks areas>1 (src/ops/search_text.rs) and never the areas==0 case.
Why: a client with a bug that drops the search area gets plausible-looking results drawn from the whole catalog instead of the refusal AWS gives, so the bug ships undetected and the emulator's result set is unreachable in production.
Fix: the SearchText handler's area-exclusivity check must enforce exactly-one (reject 0 as well as >1) and raise the required-one-of ValidationException.
Traces: search-text-validation-rejected#task#3.

--- Cause 6 of 10: SearchText two location fields set — "Only one" wording instead of "Exactly one", wrong reason, FieldList over-populated ---
AWS: for SearchText the location group is required AND exclusive, so setting two of them is refused 400 with the fixed text "Exactly one of the following fields must be set: " + the quoted group + ".", reason "FieldValidationFailed", and a fieldList entry only for the members the request actually set.
Emulator: message "Only one of the following fields must be set: …", reason "Other", and a fieldList entry for all three alternatives including the unset Filter.Circle.
Why: the message differs from AWS's for this error class, and the structured field list points at a field the request never carried.
Fix: as Cause 1, but in src/ops/search_text.rs — a required-one-of group needs an "Exactly one of the following fields must be set" builder distinct from state::only_one_of, reason FieldValidationFailed, and a FieldList derived from the members present.
Traces: search-text-validation-rejected#task#4.

--- Cause 7 of 10: SearchText MaxResults below its range — reason and fieldList missing ---
AWS: Smithy: SearchTextRequest$MaxResults @range min 1 max 100. A value below the minimum is refused 400 with reason "FieldValidationFailed" and fieldList = [{name: "MaxResults", message: "'MaxResults' must not be less than <modeled minimum>"}].
Emulator: the message matches, but reason and fieldList are absent from the body.
Why: same as Cause 3 — the error shape's @required members are missing.
Fix: the boundary-refusal adapter (service::EmulatedService::override_refusal / state::violation_message).
Traces: search-text-validation-rejected#task#5.

--- Cause 8 of 10: Suggest two location fields set — "Only one" wording instead of "Exactly one", wrong reason, FieldList over-populated ---
AWS: Suggest's BiasPosition / Filter.BoundingBox / Filter.Circle group behaves like SearchText's: a request setting two of them is refused 400 with "Exactly one of the following fields must be set: " + the quoted group + ".", reason "FieldValidationFailed", and fieldList entries only for the members the request set.
Emulator: message "Only one of the following fields must be set: …", reason "Other", and an extra fieldList entry for the unset Filter.BoundingBox.
Why: wrong error text and a field list that misattributes the failure.
Fix: the Suggest handler's area-exclusivity branch (src/ops/suggest.rs) plus the shared wording/FieldList builders in state.rs.
Traces: suggest-validation-rejected#task#2.

--- Cause 9 of 10: Suggest missing required QueryText — wrong message and reason "Missing" absent ---
AWS: Smithy: SuggestRequest$QueryText is @required. Omitting it is refused 400 ValidationException with message "'QueryText' is a required field" — fixed text "' is a required field" around the member name — reason "Missing" (the ValidationExceptionReason value documented as "The required input is missing"), and fieldList = [{name: the member, message: the same sentence}].
Emulator: 400 with message "'QueryText' must not be null." and no reason and no fieldList.
Why: a client cannot distinguish a missing-input failure (reason Missing) from a field-validation failure, and the message is not AWS's wording for an absent required member.
Fix: the boundary-refusal adapter must map ViolationKind::MissingRequiredMember to reason Missing with the "is a required field" wording (state::must_not_be_null / state::violation_message) and emit the FieldList.
Traces: suggest-validation-rejected#task#3 (cluster also covers suggest-validation-rejected#task#4).

--- Cause 10 of 10: Suggest MaxQueryRefinements above its range — reason and fieldList missing ---
AWS: Smithy: SuggestRequest$MaxQueryRefinements @range min 1 max 10. A larger value is refused 400 with reason "FieldValidationFailed" and fieldList = [{name: "MaxQueryRefinements", message: "'MaxQueryRefinements' must not be greater than <modeled maximum>"}].
Emulator: the message matches, but reason and fieldList are absent.
Why: same as Causes 3 and 7 — the @required members of ValidationException are dropped.
Fix: the boundary-refusal adapter.
Traces: suggest-validation-rejected#task#5.

Note: the only other open geo-places issue is "ReverseGeocode returns the default-view PlaceId when PoliticalView is requested" (20261124T125728Z-geo-places-reversegeocode-…), which concerns PlaceId derivation and covers none of the causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 25%, 15/20 steps mismatched). Per cause, expected (recorded AWS) vs actual (emulator):

1. autocomplete-validation-rejected#task#2, Autocomplete, 400 vs 400: $.message expected "Only one of the following fields may be set: 'BiasPosition', 'Filter.BoundingBox', 'Filter.Circle'.", got "Only one of the following fields must be set: 'BiasPosition', 'Filter.BoundingBox', 'Filter.Circle'."; $.reason expected 'FieldValidationFailed', got 'Other'; $.fieldList[name="Filter.Circle"] extra (expected absent, got an entry).
2. autocomplete-validation-rejected#task#3, 400 vs 400: $.message expected "'IntendedUse' must contain an enumerated value from AutocompleteIntendedUse", got "'IntendedUse' must be one of: SingleUse"; $.reason expected 'FieldValidationFailed', got None; $.fieldList expected [{name 'IntendedUse', message as above}], got None.
3. autocomplete-validation-rejected#task#4, 400 vs 400: $.reason expected 'FieldValidationFailed', got None; $.fieldList expected [{name 'MaxResults', message "'MaxResults' must not be greater than 20"}], got None.
4. search-text-validation-rejected#task#2, 400 vs 400: $.message expected "Only one of the following fields may be set: 'QueryId', 'QueryText'.", got "Only one of the following fields must be set: 'QueryText', 'QueryId'."; $.reason expected 'FieldValidationFailed', got 'Other'; fieldList entries for 'MaxResults' ("Only one of the following fields may be set: 'QueryId', 'MaxResults'.") and 'BiasPosition' ("… 'QueryId', 'BiasPosition'.") expected and absent; fieldList[name="QueryId"].message expected "'QueryId' must be a valid ID.", got the top-level mutual-exclusion sentence.
5. search-text-validation-rejected#task#3, expected 400, GOT 200: $.message expected "Exactly one of the following fields must be set: 'BiasPosition', 'Filter.BoundingBox', 'Filter.Circle'.", got None; $.reason expected 'FieldValidationFailed', got None; $.fieldList expected three entries (BiasPosition, Filter.BoundingBox, Filter.Circle), got None; header x-amzn-errortype expected present, got None.
6. search-text-validation-rejected#task#4, 400 vs 400: $.message expected "Exactly one of the following fields must be set: 'BiasPosition', 'Filter.BoundingBox', 'Filter.Circle'.", got "Only one of the following fields must be set: …"; $.reason expected 'FieldValidationFailed', got 'Other'; $.fieldList[name="Filter.Circle"] extra.
7. search-text-validation-rejected#task#5, 400 vs 400: $.reason expected 'FieldValidationFailed', got None; $.fieldList expected [{name 'MaxResults', message "'MaxResults' must not be less than 1"}], got None.
8. suggest-validation-rejected#task#2, 400 vs 400: $.message expected "Exactly one of the following fields must be set: …", got "Only one of the following fields must be set: …"; $.reason expected 'FieldValidationFailed', got 'Other'; $.fieldList[name="Filter.BoundingBox"] extra.
9. suggest-validation-rejected#task#3, 400 vs 400: $.message expected "'QueryText' is a required field", got "'QueryText' must not be null."; $.reason expected 'Missing', got None; $.fieldList expected [{name 'QueryText', message "'QueryText' is a required field"}], got None.
10. suggest-validation-rejected#task#5, 400 vs 400: $.reason expected 'FieldValidationFailed', got None; $.fieldList expected [{name 'MaxQueryRefinements', message "'MaxQueryRefinements' must not be greater than 10"}], got None.

Code evidence: state::only_one_of emits "Only one of the following fields must be set: …" and is the only exclusivity wording the crate has (state.rs ~line 2442); state::validation_exception_for builds the FieldList from the WHOLE group passed by the handler (state.rs ~line 2330); src/ops/search_text.rs ~line 298 and src/ops/autocomplete.rs ~line 265 pass all three area members and REASON_OTHER, and search_text.rs checks only areas>1; state::violation_message (state.rs ~line 2484) renders a framework violation as a bare message, which service::EmulatedService::override_refusal serves without Reason/FieldList.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Representative: fixture autocomplete-validation-rejected, step task#2 — POST /v2/autocomplete with QueryText plus two of BiasPosition / Filter.BoundingBox / Filter.Circle; compare body message, reason and fieldList against the recorded 400.
3. Remaining causes: autocomplete-validation-rejected#task#3 (IntendedUse outside AutocompleteIntendedUse), #task#4 (MaxResults > 20); search-text-validation-rejected#task#2 (QueryId with QueryText + MaxResults + BiasPosition), #task#3 (POST /v2/search-text with QueryText and none of BiasPosition/Filter.BoundingBox/Filter.Circle — observe 200 instead of 400), #task#4 (two area fields), #task#5 (MaxResults < 1); suggest-validation-rejected#task#2 (two area fields), #task#3 (QueryText omitted), #task#5 (MaxQueryRefinements > 10).
4. In every case assert the body carries both reason and fieldList and that message matches the recorded AWS wording.
