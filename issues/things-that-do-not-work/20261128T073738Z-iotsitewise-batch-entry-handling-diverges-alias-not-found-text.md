Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.batch-get-aggregates-rejects-unknown-entries-beside-empty-and-counted iotsitewise.batch-get-asset-property-aggregates-every-entry-refused-still-answers iotsitewise.batch-get-asset-property-value-every-entry-refused-still-answers iotsitewise.batch-get-asset-property-value-rejects-unbound-alias-until-bound iotsitewise.batch-get-asset-property-value-history-reports-bad-entries-beside-good-ones iotsitewise.batch-value-history-isolates-a-failing-entry-rejected iotsitewise.batch-get-asset-property-value-history-rejects-duplicate-ids-and-a-17th-entry iotsitewise.batch-get-value-history-seventeen-entries-rejected iotsitewise.batch-history-seventeen-entries-rejected iotsitewise.batch-get-asset-property-value-history-rejects-duplicate-entry-ids iotsitewise.batch-put-refuses-a-repeated-entry-id-rejected iotsitewise.batch-put-refuses-an-eleventh-entry-rejected iotsitewise.batch-put-entry-over-ten-values-is-refused-alone iotsitewise.batch-put-entry-naming-alias-and-ids-is-refused-alone iotsitewise.batch-put-every-entry-refused-answers-with-error-entries iotsitewise.batch-put-partial-entry-processing-stores-in-range-values iotsitewise.batch-put-refused-entries-keyed-by-entry-id-rejected
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.batch-get-aggregates-rejects-unknown-entries-beside-empty-and-counted|run#35|AssertionError|assert message.startswith("Property not found ("), (
Issue-Key: iotsitewise.batch-get-asset-property-aggregates-every-entry-refused-still-answers|_expect_refusals#22|AssertionError|assert message.startswith(form) and message.endswith(")"), (
Issue-Key: iotsitewise.batch-get-asset-property-value-every-entry-refused-still-answers|_not_found#23|AssertionError|assert message.startswith(lookup) and message.endswith(")"), (
Issue-Key: iotsitewise.batch-get-asset-property-value-rejects-unbound-alias-until-bound|_assert_not_found#28|AssertionError|assert message.startswith("Property not found ("), (
Issue-Key: iotsitewise.batch-get-asset-property-value-history-reports-bad-entries-beside-good-ones|run#23|AssertionError|assert errors == expected, f"error entries were {errors!r}, expected {expected!r}"
Issue-Key: iotsitewise.batch-value-history-isolates-a-failing-entry-rejected|run#26|AssertionError|assert failed == {
Issue-Key: iotsitewise.batch-get-asset-property-value-history-rejects-duplicate-ids-and-a-17th-entry|run#23|AssertionError|assert got == ("InvalidRequestException",
Issue-Key: iotsitewise.batch-get-value-history-seventeen-entries-rejected|run#20|AssertionError|assert (err["Code"], err.get("Message")) == (
Issue-Key: iotsitewise.batch-history-seventeen-entries-rejected|run#17|AssertionError|assert got == (
Issue-Key: iotsitewise.batch-get-asset-property-value-history-rejects-duplicate-entry-ids|_refusal#14|AssertionError|raise AssertionError(f"the request was accepted, answering {got!r}")
Issue-Key: iotsitewise.batch-put-refuses-a-repeated-entry-id-rejected|_refusal#7|AssertionError|raise AssertionError(f"the request was accepted: {answer!r}")
Issue-Key: iotsitewise.batch-put-refuses-an-eleventh-entry-rejected|run#8|AssertionError|assert got == (400, "InvalidRequestException",
Issue-Key: iotsitewise.batch-put-entry-over-ten-values-is-refused-alone|run#11|AssertionError|assert (refusal["errorCode"], refusal["errorMessage"]) == (
Issue-Key: iotsitewise.batch-put-entry-naming-alias-and-ids-is-refused-alone|run#8|AssertionError|assert reply["errorEntries"] == [{"entryId": "both", "errors": [{
Issue-Key: iotsitewise.batch-put-every-entry-refused-answers-with-error-entries|_server_clock#25|AssertionError|assert sep and tail.endswith(").") and tail[:-2].isdigit(), (
Issue-Key: iotsitewise.batch-put-partial-entry-processing-stores-in-range-values|_quoted_now#19|AssertionError|assert inner.endswith(").") and inner[:-2].isdigit(), f"no quoted time in {text!r}"
Issue-Key: iotsitewise.batch-put-refused-entries-keyed-by-entry-id-rejected|_clock_in#21|AssertionError|assert msg.endswith(").") and digits.isdigit(), f"the refusal quotes no clock: {msg!r}"
Issue-Commit: 794b6570c5e12ac0c4a18da454471fcedcd24ce4

# Batch entry handling diverges: alias not-found text, per-entry error wording, entry/value limit messages, duplicate entryId accepted, and wrong timestamp-range error code

> _Note: this issue was identified against the code base at commit `794b6570c5e12ac0c4a18da454471fcedcd24ce4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutAssetPropertyValue

## Description
This issue bundles 8 distinct root causes in the IoT SiteWise batch data-plane (BatchGetAssetPropertyValue, BatchGetAssetPropertyValueHistory, BatchGetAssetPropertyAggregates, BatchPutAssetPropertyValue). A fixer must close ALL EIGHT before submitting; each cause names its own failing checks.

Cause 1 of 8: a propertyAlias that is not bound to any property is reported as a missing time series, not as a missing property.
AWS: an entry (or single read) that names a propertyAlias no property is associated with is refused ResourceNotFoundException with the message "Property not found (Property Alias = '<alias>')" — fixed text "Property not found (Property Alias = ", then the alias exactly as sent, then the closing parenthesis; the same family as the ids form "Property not found (Property Id = <propertyId>, Asset Id = <assetId>)" (Smithy: ResourceNotFoundException, client, httpError 404; AssetPropertyAlias @length 1-2048; message text is recorded behavior).
Emulator: answers "Time Series with ID <alias> not found" — a different noun, a different template and no parenthesised Property Alias form.
Why: a client that distinguishes an unbound alias from other not-found conditions by the documented "Property not found (...)" prefix misroutes the error; the alias-binding workflow (BatchGet before/after AssociateTimeSeriesToAssetProperty) cannot be driven off the message.
Fix: the alias resolution step shared by the batch and single property-value handlers must raise the property-not-found error with the alias form, not the time-series store's own message.
Traces: iotsitewise.batch-get-aggregates-rejects-unknown-entries-beside-empty-and-counted, iotsitewise.batch-get-asset-property-aggregates-every-entry-refused-still-answers, iotsitewise.batch-get-asset-property-value-every-entry-refused-still-answers, iotsitewise.batch-get-asset-property-value-rejects-unbound-alias-until-bound.

Cause 2 of 8: per-entry errorMessage wording for an inverted window and for an entry that identifies no property.
AWS: inside errorEntries of a batch read, the per-entry messages are the bracketed validation forms: an entry whose startDate is not before its endDate gives errorCode InvalidRequestException with errorMessage "[Start date must be before end date.]"; an entry naming neither (assetId+propertyId) nor propertyAlias gives InvalidRequestException "[Both property ID and asset ID are required. Alternatively, an alias must be provided if the IDs are absent.]" — both entirely fixed text including the square brackets (recorded behavior; InvalidRequestException is the modeled 400 error).
Emulator: answers "endDate must not precede startDate." and "Each entry must specify either assetId and propertyId, or propertyAlias." — emulator-authored, unbracketed.
Why: the batch surface reports per-entry failures only through these strings, so a client classifying entry failures by message sees neither AWS form.
Fix: the per-entry validation of the batch read handlers (the entry validator that produces errorEntries).
Traces: iotsitewise.batch-get-asset-property-value-history-reports-bad-entries-beside-good-ones, iotsitewise.batch-value-history-isolates-a-failing-entry-rejected.

Cause 3 of 8: the >16-entry request limit message.
AWS: a batch read carrying more than 16 entries is refused whole with 400 InvalidRequestException and the message "Limit has been reached. Maximum allowed limit is 16 entries per request." — fixed text; the limit 16 is the documented per-request entry cap (Smithy: entries required list, no @length in the model, so the cap and the text are recorded/documented behavior; InvalidRequestException client 400).
Emulator: answers InvalidRequestException "A request carries 1 to 16 entries, but this one carries 17." — emulator-authored and parameterised on the submitted count.
Why: the limit is enforced but reported with wording no AWS client recognises; the submitted count is per-request detail AWS does not put in this message.
Fix: the request-level entry-count validation shared by BatchGetAssetPropertyValue, BatchGetAssetPropertyValueHistory and BatchGetAssetPropertyAggregates.
Traces: iotsitewise.batch-get-asset-property-value-history-rejects-duplicate-ids-and-a-17th-entry, iotsitewise.batch-get-value-history-seventeen-entries-rejected, iotsitewise.batch-history-seventeen-entries-rejected.

Cause 4 of 8: a repeated entryId is accepted instead of refusing the whole request.
AWS: entry ids must be unique within one request (Smithy: EntryId @length 1-64, pattern ^[a-zA-Z0-9_-]+$ — the uniqueness rule is the documented per-request requirement). A request repeating an entryId is refused whole with HTTP 400 InvalidRequestException and the fixed message "Entry Id is not unique", and nothing is read or stored.
Emulator: accepts it. BatchGetAssetPropertyValueHistory with two entries sharing entryId "twin" answered 200 with two successEntries both keyed "twin"; BatchPutAssetPropertyValue repeating an entryId answered 200 with errorEntries [] and stored the values.
Why: a client cannot key results by entryId at all (duplicate keys collide silently), and a put that AWS rejects wholesale is partially applied on the emulator — a real divergence in stored state, not just wording.
Fix: a request-level duplicate-entryId check in the batch read and batch put handlers' validation, before any entry is served or stored.
Traces: iotsitewise.batch-get-asset-property-value-history-rejects-duplicate-entry-ids, iotsitewise.batch-put-refuses-a-repeated-entry-id-rejected.

Cause 5 of 8: BatchPutAssetPropertyValue's >10-entry limit message.
AWS: a put carrying more than 10 entries is refused with HTTP 400 InvalidRequestException and the fixed message "Limit has been reached. Maximum allowed limit is 10 entries per request." (10 entries per BatchPutAssetPropertyValue request is the documented cap; the text is recorded behavior).
Emulator: 400 InvalidRequestException "A request carries at most 10 entries, but this one carries 11."
Why: same wording mismatch as Cause 3 on the write path.
Fix: BatchPutAssetPropertyValue's request-level entry-count validation.
Traces: iotsitewise.batch-put-refuses-an-eleventh-entry-rejected.

Cause 6 of 8: an entry with more than 10 propertyValues is refused with the WRONG error code and message.
AWS: PutAssetPropertyValueEntry.propertyValues accepts at most 10 elements (Smithy doc: "You can specify up to 10 propertyValues array elements"). An entry carrying 11 is refused per-entry inside errorEntries with errorCode "InvalidRequestException" and errorMessage "Limit has been reached. Maximum allowed limit is 10 values per entry.", its timestamps member listing every timestamp of the refused entry; sibling entries in the same request are still stored.
Emulator: errorCode "LimitExceededException" with errorMessage "An entry carries at most 10 propertyValues, but this one carries 11." — wrong error class (LimitExceededException is the modeled 410 throttle/limit error, not this per-entry validation failure) and emulator-authored text.
Why: a client branching on errorCode treats a permanent validation failure as a service limit (commonly retried or escalated) instead of fixing the request.
Fix: the per-entry value-count validation in the BatchPutAssetPropertyValue handler.
Traces: iotsitewise.batch-put-entry-over-ten-values-is-refused-alone.

Cause 7 of 8: a put entry naming propertyAlias together with assetId and propertyId.
AWS: refused per-entry with errorCode InvalidRequestException and the fixed message "Either an alias can be provided OR both property ID and asset ID. All three cannot be provided.", with the entry's timestamps listed, while the sibling entry naming only the ids is stored.
Emulator: errorCode is right but the message is "Each entry must specify either assetId and propertyId, or propertyAlias."
Why: the all-three case is indistinguishable from the names-nothing case (Cause 2), which AWS words differently.
Fix: the entry property-identification validation in the BatchPutAssetPropertyValue handler.
Traces: iotsitewise.batch-put-entry-naming-alias-and-ids-is-refused-alone.

Cause 8 of 8: a timestamp outside the ingest window is refused with the wrong error code and a message that does not state the constraint or quote the service clock.
AWS: a value whose timestamp lies outside [now - 604800 s, now + 600 s] is refused per-entry with errorCode "TimestampOutOfRangeException" and errorMessage "Value '<timeInSeconds>' at 'timeInSeconds' failed to satisfy constraint: Member must be within 604800 seconds before and 600 seconds after the current timestamp (<service-now>)." — fixed text with two variable parts: the rejected timestamp from the request and the service's own current epoch second in the trailing parentheses; the whole entry is refused and nothing from it is stored (recorded behavior).
Emulator: errorCode is not AWS's and the message is emulator-authored: "The timestamp 1795159373.000000000 is outside the inclusive range of [-7 days, +10 minutes] around the current time, so IoT SiteWise cannot store it." — it neither names the member and the 604800/600-second constraint nor quotes the service clock, so a caller cannot recover the server's notion of now.
Why: clients that resynchronise their clock from this message (the documented way to discover skew against the ingest window) get nothing to parse, and errorCode-based handling of out-of-range ingest fails.
Fix: the timestamp-window validation in the BatchPutAssetPropertyValue handler (error class plus message template).
Traces: iotsitewise.batch-put-every-entry-refused-answers-with-error-entries, iotsitewise.batch-put-partial-entry-processing-stores-in-range-values, iotsitewise.batch-put-refused-entries-keyed-by-entry-id-rejected.

## Evidence
Cause 1 — expected message starting "Property not found (Property Alias = " and ending ")"; got 'Time Series with ID /sim2real/substation84e9eb192469/feeder/voltage not found', 'Time Series with ID /sim2real/unbound-55700aa10467/heat not found', 'Time Series with ID /sim2real/unbound-e3bc7a23db19/pressure not found', 'Time Series with ID /sim2real/plant3b7918bee1a7/line-4/pressure not found' (the alias strings are per-run; the template is the requirement).
Cause 2 — expected {'backwards': ('InvalidRequestException', '[Start date must be before end date.]'), 'anonymous': ('InvalidRequestException', '[Both property ID and asset ID are required. Alternatively, an alias must be provided if the IDs are absent.]')}; got {'backwards': ('InvalidRequestException', 'endDate must not precede startDate.'), 'anonymous': ('InvalidRequestException', 'Each entry must specify either assetId and propertyId, or propertyAlias.')}. The ids-form entry in the same response ('Property not found (Property Id = ..., Asset Id = ...)') matched, so only these two strings diverge.
Cause 3 — expected ('InvalidRequestException', 'Limit has been reached. Maximum allowed limit is 16 entries per request.'); got ('InvalidRequestException', 'A request carries 1 to 16 entries, but this one carries 17.').
Cause 4 — BatchGetAssetPropertyValueHistory with entries ['twin','twin'] returned 200 {'errorEntries': [], 'successEntries': [{'entryId': 'twin', ...}, {'entryId': 'twin', ...}], 'skippedEntries': []}; expected ('InvalidRequestException', 'Entry Id is not unique'). BatchPutAssetPropertyValue repeating an entryId returned HTTP 200 {'errorEntries': []}; expected (400, 'InvalidRequestException', 'Entry Id is not unique') and nothing stored.
Cause 5 — expected (400, 'InvalidRequestException', 'Limit has been reached. Maximum allowed limit is 10 entries per request.'); got (400, 'InvalidRequestException', 'A request carries at most 10 entries, but this one carries 11.').
Cause 6 — expected errorCode 'InvalidRequestException' with errorMessage 'Limit has been reached. Maximum allowed limit is 10 values per entry.'; got errorCode 'LimitExceededException' with 'An entry carries at most 10 propertyValues, but this one carries 11.' (timestamps member was populated).
Cause 7 — expected errorEntries [{entryId: 'both', errors: [{errorCode: 'InvalidRequestException', errorMessage: 'Either an alias can be provided OR both property ID and asset ID. All three cannot be provided.', timestamps: [the entry's timestamp]}]}]; got errorMessage 'Each entry must specify either assetId and propertyId, or propertyAlias.'
Cause 8 — expected errorCode 'TimestampOutOfRangeException' and message "Value '<ts>' at 'timeInSeconds' failed to satisfy constraint: Member must be within 604800 seconds before and 600 seconds after the current timestamp (<now>)."; got 'The timestamp 1795159373.000000000 is outside the inclusive range of [-7 days, +10 minutes] around the current time, so IoT SiteWise cannot store it.' (and 1795854062.../1795159627... in the other two runs — per-run values; the missing constraint text and missing quoted clock are the defect).

## Steps to Reproduce
1. Create an asset model with DOUBLE measurement properties, create an asset, wait for ACTIVE, ingest a GOOD reading.
2. Cause 1: BatchGetAssetPropertyValue / ...ValueHistory / ...Aggregates with an entry naming a propertyAlias bound to nothing; read the errorEntries message.
3. Cause 2: BatchGetAssetPropertyValueHistory with one entry whose startDate >= endDate and one entry naming neither ids nor alias, beside a good entry; read errorEntries.
4. Cause 3: send 17 entries to BatchGetAssetPropertyValueHistory.
5. Cause 4: send two entries sharing one entryId to BatchGetAssetPropertyValueHistory, and two put entries sharing one entryId to BatchPutAssetPropertyValue; then read the property history to see whether anything was stored.
6. Cause 5: BatchPutAssetPropertyValue with 11 entries.
7. Cause 6: BatchPutAssetPropertyValue with one entry carrying 11 propertyValues beside a 10-value entry.
8. Cause 7: BatchPutAssetPropertyValue with an entry naming assetId, propertyId AND propertyAlias beside an ids-only entry.
9. Cause 8: BatchPutAssetPropertyValue with a value timestamped 8 days in the past (and one ~10 minutes in the future); inspect errorEntries[].errors[].errorCode/errorMessage.
