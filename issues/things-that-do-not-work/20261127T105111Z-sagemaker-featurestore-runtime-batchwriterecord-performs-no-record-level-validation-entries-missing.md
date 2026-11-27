Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-featurestore-runtime.batch-write-record-error-entry-echoes-its-refused-request-entry sagemaker-featurestore-runtime.batch-write-record-every-entry-refused-answers-errors-not-unprocessed sagemaker-featurestore-runtime.batch-write-record-every-entry-refused-still-answers sagemaker-featurestore-runtime.batch-write-record-refused-entries-each-carry-their-own-entry sagemaker-featurestore-runtime.batch-write-record-unparseable-integral-refuses-whole-batch sagemaker-featurestore-runtime.batch-write-record-unparseable-value-refuses-whole-batch
Issue-Shape-Id: com.amazonaws.sagemakerfeaturestoreruntime#AmazonSageMakerFeatureStoreRuntime
Issue-Key: sagemaker-featurestore-runtime.batch-write-record-error-entry-echoes-its-refused-request-entry|run#13|AssertionError|assert len(got) == len(want), f"expected {len(want)} errors, got {len(got)}: {got!r}"
Issue-Key: sagemaker-featurestore-runtime.batch-write-record-every-entry-refused-answers-errors-not-unprocessed|run#15|AssertionError|assert sorted(by_sku) == sorted(expected), (
Issue-Key: sagemaker-featurestore-runtime.batch-write-record-every-entry-refused-still-answers|run#13|AssertionError|assert len(errors) == len(refused), (
Issue-Key: sagemaker-featurestore-runtime.batch-write-record-refused-entries-each-carry-their-own-entry|run#14|AssertionError|assert len(errors) == 2, (
Issue-Key: sagemaker-featurestore-runtime.batch-write-record-unparseable-integral-refuses-whole-batch|_refusal#6|AssertionError|raise AssertionError(f"expected the batch to be refused, it was answered: {answer!r}")
Issue-Key: sagemaker-featurestore-runtime.batch-write-record-unparseable-value-refuses-whole-batch|_refusal#6|AssertionError|raise AssertionError(f"expected the batch to be refused outright, it answered {result!r}")
Issue-Commit: 96893890513d5e7e8a4c2241420ddac2d94594cf

# BatchWriteRecord performs no record-level validation: entries missing the event-time feature, naming undefined features, using ValueAsStringList for a scalar, or carrying an unparseable Integral are all accepted and written

> _Note: this issue was identified against the code base at commit `96893890513d5e7e8a4c2241420ddac2d94594cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchWriteRecord

## Description
Four net-new root causes in the emulator's BatchWriteRecord handler. The only refusal it produces today is the missing-feature-group one; every record-shape validation AWS applies is absent, so invalid entries are accepted (Errors == [], UnprocessedEntries == []) and in the whole-batch cases data is written that AWS would never store.

Cause 1 of 4: an entry whose Record omits the group's event-time feature is not refused.
AWS: Smithy models BatchWriteRecord with Entries (1..25), output 200, and BatchWriteRecordResponse with required Errors and UnprocessedEntries; BatchWriteRecordError carries Entry, ErrorCode and ErrorMessage, and ValidationError is a client error with httpError 400. The model does not spell the rule out, so for the rule itself this is recorded behavior only: AWS answers HTTP 200 and emits ONE Errors row for that entry, with the refused request Entry echoed back UNCHANGED, ErrorCode "ValidationError" and ErrorMessage "Validation Error: Provided Record doesn't contain a required FeatureValue. Provide a FeatureValue with the name [<EventTimeFeatureName>]." FIXED text: "Validation Error: Provided Record doesn't contain a required FeatureValue. Provide a FeatureValue with the name [" and "]."; the VARIABLE part is the group's EventTimeFeatureName, taken from the feature group's definition in state. The entry must NOT be stored (a later GetRecord for that identifier returns no Record), while its valid siblings in the same call ARE stored.
Emulator: the entry is accepted — Errors is empty for it — and, where the checks probed, nothing distinguishes it from a valid write.
Traces: sagemaker-featurestore-runtime.batch-write-record-error-entry-echoes-its-refused-request-entry, sagemaker-featurestore-runtime.batch-write-record-every-entry-refused-answers-errors-not-unprocessed, sagemaker-featurestore-runtime.batch-write-record-every-entry-refused-still-answers, sagemaker-featurestore-runtime.batch-write-record-refused-entries-each-carry-their-own-entry, sagemaker-featurestore-runtime.batch-write-record-unparseable-value-refuses-whole-batch.

Cause 2 of 4: an entry whose Record names a feature the group does not define is not refused.
AWS: recorded behavior only for the rule; HTTP 200 with one Errors row for that entry, Entry echoed unchanged, ErrorCode "ValidationError", ErrorMessage "Validation Error: Provided values for Feature(s) contain invalid entries which don't exist in FeatureGroup [<group>]." FIXED text: "Validation Error: Provided values for Feature(s) contain invalid entries which don't exist in FeatureGroup [" and "]."; VARIABLE part: the feature group name from the entry's FeatureGroupName. The entry is not stored; the corrected entry (same record without the stray feature) is then accepted and readable.
Emulator: accepted silently. In the all-refused batches the emulator produced only the missing-feature-group row and omitted this one.
Traces: sagemaker-featurestore-runtime.batch-write-record-every-entry-refused-answers-errors-not-unprocessed, sagemaker-featurestore-runtime.batch-write-record-every-entry-refused-still-answers, sagemaker-featurestore-runtime.batch-write-record-refused-entries-each-carry-their-own-entry.

Cause 3 of 4: a FeatureValue supplied as ValueAsStringList for a scalar (non-collection) feature is not refused.
AWS: Smithy's FeatureValue carries both ValueAsString and ValueAsStringList; for a feature whose definition is a scalar type, AWS requires the scalar member. Recorded behavior for the message: HTTP 200 with one Errors row for that entry, Entry echoed unchanged, ErrorCode "ValidationError", ErrorMessage "Validation Error: ValueAsString required for feature [<feature>]." FIXED text: "Validation Error: ValueAsString required for feature [" and "]."; VARIABLE part: the offending feature name from the request's Record. The entry is not stored.
Emulator: accepted with no error row.
Traces: sagemaker-featurestore-runtime.batch-write-record-error-entry-echoes-its-refused-request-entry.

Cause 4 of 4: a value that cannot be parsed into its Integral feature type does not refuse the WHOLE request.
AWS: this one is NOT a per-entry error — it is a request-level refusal. Smithy declares ValidationError as a client error with httpError 400 on BatchWriteRecord. Recorded behavior: the entire call fails with HTTP 400, Error Code "ValidationError" and Message "Validation Error: Attempted to parse the feature value for the feature named [<feature>] into a FeatureValue of type Integral. The provided value must be within the range of a 64 bit signed integer." FIXED text: "Validation Error: Attempted to parse the feature value for the feature named [" and "] into a FeatureValue of type Integral. The provided value must be within the range of a 64 bit signed integer."; VARIABLE part: the feature name from the request's Record whose declared FeatureType is Integral. NO entry of that request is stored, including the entries whose values were perfectly valid. The legal neighbour — the same batch with a parseable value — is accepted in full, and an entry refused per-entry (Cause 1) does NOT take its siblings down, which is what makes the whole-batch scope specific to the parse failure.
Emulator: the call is answered HTTP 200 with Errors == [] and UnprocessedEntries == [] — no refusal anywhere, and the unparseable value is ingested.
Why (all four): BatchWriteRecord's entire value proposition is per-entry partial failure plus request-level rejection of malformed values. The emulator reports neither, so (a) a client's error-handling branch for Errors[] is never exercised and ships untested, (b) the emulator's online store ends up holding records AWS refuses — missing event times, undefined columns, non-numeric Integrals — so subsequent GetRecord/BatchGetRecord/ListRecords reads diverge from AWS for the rest of the session, and (c) a caller matching an error back to its request via the echoed Entry has nothing to match. For Cause 4 a client also loses the all-or-nothing guarantee and cannot tell whether a retry would duplicate work.
Fix: BatchWriteRecord's validation and handler (services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/batch_write_record.rs) plus the shared feature-value coercion it should call: validate each Entry against the resolved group's FeatureDefinitions (required record-identifier and event-time features present; no undefined feature names; scalar features given via ValueAsString) and emit one Errors row per refused entry carrying that Entry verbatim while leaving the store untouched for it; hoist the Integral/Fractional parse into a pre-pass that fails the whole request with the modelled 400 ValidationError before any entry is written.

## Evidence
Cause 1+3 — batch-write-record-error-entry-echoes-its-refused-request-entry:
  Request: Entries = [good(ord-a), {Record without the "placed" event-time value}, good(ord-b), {Record with basket given as ValueAsStringList:["3"]}, good(ord-c)]
  Expected: Errors holding exactly 2 rows — one echoing the no-event-time entry with "Validation Error: Provided Record doesn't contain a required FeatureValue. Provide a FeatureValue with the name [placed].", one echoing the list-for-scalar entry with "Validation Error: ValueAsString required for feature [basket]."; both ErrorCode "ValidationError"; neither entry stored
  Actual: AssertionError "expected 2 errors, got 0: []"

Cause 1+2 — batch-write-record-every-entry-refused-answers-errors-not-unprocessed:
  Request: three entries — one naming a non-existent feature group, one adding undefined feature "colour", one omitting "seen_at"
  Expected Errors to name records ['sku-orphan','sku-stray','sku-untimed']
  Actual: "Errors named records ['sku-orphan'], expected exactly ['sku-orphan', 'sku-stray', 'sku-untimed']"

Cause 1+2 — batch-write-record-every-entry-refused-still-answers:
  Expected all 3 entries refused (missing group / undefined "gift" feature / missing "placed")
  Actual: "expected all 3 entries refused, got 1: [\"Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name [arn:aws:sagemaker:us-west-2:<account>:feature-group/...]\"]"

Cause 1+2 — batch-write-record-refused-entries-each-carry-their-own-entry:
  Request: [valid sku-first, entry with undefined "colour", entry without "seen_at", valid sku-last]
  Expected: exactly 2 Errors rows, each carrying its own Entry verbatim ("...invalid entries which don't exist in FeatureGroup [G]." and "...Provide a FeatureValue with the name [seen_at].")
  Actual: "expected exactly 2 refused entries, got 0: []"

Cause 4 — batch-write-record-unparseable-integral-refuses-whole-batch:
  Request: three entries for an Integral feature "count" with values 5, "five", 6
  Expected: ClientError HTTP 400, "ValidationError", "Validation Error: Attempted to parse the feature value for the feature named [count] into a FeatureValue of type Integral. The provided value must be within the range of a 64 bit signed integer.", and none of the three stored
  Actual: "expected the batch to be refused, it was answered: {'Errors': [], 'UnprocessedEntries': []}"

Cause 4 — batch-write-record-unparseable-value-refuses-whole-batch:
  Request: Entries = [reading="40", reading="forty"] on Integral feature "reading"
  Expected: HTTP 400 "ValidationError" "Validation Error: Attempted to parse the feature value for the feature named [reading] into a FeatureValue of type Integral. ..."
  Actual: HTTP 200, {'Errors': [], 'UnprocessedEntries': []}

## Steps to Reproduce
1. CreateFeatureGroup G with RecordIdentifierFeatureName=sku, EventTimeFeatureName=seen_at, FeatureDefinitions sku(String)/seen_at(Fractional)/units(Integral), OnlineStoreConfig.EnableOnlineStore=true; wait for FeatureGroupStatus=Created.
2. Cause 1: BatchWriteRecord with Entries=[{G, Record=[sku="s1", seen_at=<now>, units="1"]}, {G, Record=[sku="s2", units="2"]}] (second omits seen_at). Expect HTTP 200 with one Errors row echoing the second Entry verbatim, ErrorCode "ValidationError", ErrorMessage "Validation Error: Provided Record doesn't contain a required FeatureValue. Provide a FeatureValue with the name [seen_at]."; then GetRecord(G,"s2") must return no Record. The emulator returns Errors=[].
3. Cause 2: BatchWriteRecord with Entries=[{G, Record=[sku="s3", seen_at=<now>, units="3", colour="teal"]}] where colour is undefined. Expect one Errors row "Validation Error: Provided values for Feature(s) contain invalid entries which don't exist in FeatureGroup [G]." and s3 unstored; the emulator accepts it.
4. Cause 3: BatchWriteRecord with Entries=[{G, Record=[sku="s4", seen_at=<now>, {FeatureName:"units", ValueAsStringList:["4"]}]}]. Expect one Errors row "Validation Error: ValueAsString required for feature [units]." and s4 unstored; the emulator accepts it.
5. Cause 4: BatchWriteRecord with Entries=[{G, Record=[sku="s5", seen_at=<now>, units="5"]}, {G, Record=[sku="s6", seen_at=<now>, units="five"]}]. Expect the WHOLE call to fail with HTTP 400 "ValidationError" "Validation Error: Attempted to parse the feature value for the feature named [units] into a FeatureValue of type Integral. The provided value must be within the range of a 64 bit signed integer.", with neither s5 nor s6 stored. The emulator answers HTTP 200 with Errors=[] and UnprocessedEntries=[].
6. Control for 5: the same batch with units="55" instead of "five" must be accepted in full and both records readable.
