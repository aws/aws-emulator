Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-featurestore-runtime.batch-get-record-duplicate-feature-group-identifiers-rejected sagemaker-featurestore-runtime.batch-get-record-unknown-feature-name-reported-per-identifier sagemaker-featurestore-runtime.batch-get-record-undefined-feature-name-refused-per-identifier sagemaker-featurestore-runtime.batch-get-record-every-identifier-refused-answers-errors-only
Issue-Shape-Id: com.amazonaws.sagemakerfeaturestoreruntime#AmazonSageMakerFeatureStoreRuntime
Issue-Key: sagemaker-featurestore-runtime.batch-get-record-duplicate-feature-group-identifiers-rejected|_refusal#14|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: sagemaker-featurestore-runtime.batch-get-record-unknown-feature-name-reported-per-identifier|run#15|AssertionError|assert answer["Errors"] == [want_error], f"Errors was {answer['Errors']!r}, expected [{want_error!r}]"
Issue-Key: sagemaker-featurestore-runtime.batch-get-record-undefined-feature-name-refused-per-identifier|run#18|AssertionError|assert records == {(good_group, "mem-cy"): {"tier": "bronze"}}, (
Issue-Key: sagemaker-featurestore-runtime.batch-get-record-every-identifier-refused-answers-errors-only|run#18|AssertionError|assert rows == want, f"Errors held {rows!r}, expected {want!r}"
Issue-Commit: 96893890513d5e7e8a4c2241420ddac2d94594cf

# BatchGetRecord skips two per-identifier validations: duplicate feature-group Identifiers are accepted, and undefined FeatureNames are silently served instead of refused in Errors

> _Note: this issue was identified against the code base at commit `96893890513d5e7e8a4c2241420ddac2d94594cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetRecord

## Description
Two net-new root causes in the emulator's BatchGetRecord handler. Both are validations the operation never performs, so requests AWS refuses are answered 200 with data.

Cause 1 of 2: BatchGetRecord accepts two Identifiers entries that name the SAME feature group.
AWS: Smithy (services/.../com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime.json) models BatchGetRecord$Identifiers as a list of 1..100 BatchGetRecordIdentifier, output 200, and declares the service-common ValidationError (errorKind client, httpError 400). The model does not state the uniqueness rule, so for that rule this is recorded behavior only: AWS refuses the whole request with HTTP 400, Error Code "ValidationError" and Message "Validation Error: identifier list contains at least two identifiers having same feature group name. Remove duplicates and try again." That message is entirely FIXED text — it has no variable parts taken from the request. The legal neighbour in the same check shows the equivalent single-Identifier request (both record identifiers listed under one FeatureGroupName) must still succeed with both Records.
Emulator: the split form is answered HTTP 200 with Records holding both records and Errors == [] / UnprocessedIdentifiers == []. No refusal at all.
Why: a client that relies on the refusal to catch its own request-building bug (the same group fanned out across several Identifiers entries) gets a successful read instead, so a duplicate-group request that real AWS rejects at the edge appears valid in test and then fails only in production. Worse, any client reading error-handling coverage from this behaviour concludes the constraint does not exist.
Fix: input validation of the BatchGetRecord handler — reject before any record lookup when the resolved FeatureGroupName appears in more than one element of Identifiers.
Traces: sagemaker-featurestore-runtime.batch-get-record-duplicate-feature-group-identifiers-rejected.

Cause 2 of 2: BatchGetRecord ignores FeatureNames that the feature group does not define.
AWS: Smithy models BatchGetRecordIdentifier$FeatureNames and BatchGetRecordResponse with three required members Records, Errors and UnprocessedIdentifiers, plus BatchGetRecordError carrying FeatureGroupName, RecordIdentifierValueAsString, ErrorCode and ErrorMessage; the operation's success status is 200. Recorded behavior for the rule itself: when an Identifiers entry names a feature the group's FeatureDefinitions do not contain, AWS still answers HTTP 200 but serves NONE of that entry's record identifiers in Records — it emits ONE Errors row per record identifier of that entry, with ErrorCode "ValidationError" and ErrorMessage "Validation Error: Provided FeatureName(s) [<names>] doesn't exist in FeatureGroup [<group>]." The FIXED text is "Validation Error: Provided FeatureName(s) [", "] doesn't exist in FeatureGroup [" and "]."; the VARIABLE parts are the undefined feature name(s) taken from the request's FeatureNames and the feature group name taken from that same Identifiers entry. UnprocessedIdentifiers stays empty, and other Identifiers entries in the same call are served normally. The legal neighbour (the same entry naming only defined features) must return the records, so the refusal is caused by the undefined name alone.
Emulator: the undefined name is dropped. With one bad entry and one good one the emulator returned Errors == [] and Records holding every record including the refused group's (check batch-get-record-unknown-feature-name-reported-per-identifier got Errors [] where one row was required; batch-get-record-undefined-feature-name-refused-per-identifier got all three records served instead of only the other group's). In the all-refused case (batch-get-record-every-identifier-refused-answers-errors-only) the emulator reported only the missing-feature-group row and omitted the undefined-FeatureName row entirely, so Errors held 1 of the 2 required rows.
Why: a typo'd or stale FeatureNames list is answered with a quietly narrowed record instead of an error, so a client cannot distinguish "feature absent from this record" from "feature does not exist in this group" and will ship a projection that silently never returns the column it asked for. It also reads records AWS refuses to serve, which changes the per-identifier partial-failure contract clients code against.
Fix: the BatchGetRecord handler's per-identifier validation and its Errors assembly (services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/batch_get_record.rs) — validate each entry's FeatureNames against the resolved group's feature definitions and fan the refusal out to one Errors row per record identifier of that entry, excluding them from Records.
Traces: sagemaker-featurestore-runtime.batch-get-record-unknown-feature-name-reported-per-identifier, sagemaker-featurestore-runtime.batch-get-record-undefined-feature-name-refused-per-identifier, sagemaker-featurestore-runtime.batch-get-record-every-identifier-refused-answers-errors-only.

## Evidence
Cause 1 — batch-get-record-duplicate-feature-group-identifiers-rejected:
  Request: Identifiers=[{FeatureGroupName: G, RecordIdentifiersValueAsString: ["north"]}, {FeatureGroupName: G, RecordIdentifiersValueAsString: ["south"]}]
  Expected: ClientError HTTP 400, Code "ValidationError", Message "Validation Error: identifier list contains at least two identifiers having same feature group name. Remove duplicates and try again."
  Actual: HTTP 200, Records = both records (north/amber, south/cobalt), Errors = [], UnprocessedIdentifiers = []

Cause 2a — batch-get-record-unknown-feature-name-reported-per-identifier:
  Request: Identifiers=[{G_asked, ["east"], FeatureNames:["shade","hue"]}, {G_other, ["west"]}] where G_asked defines key/ts/shade and no "hue"
  Expected: Errors = [{FeatureGroupName: G_asked, RecordIdentifierValueAsString: "east", ErrorCode: "ValidationError", ErrorMessage: "Validation Error: Provided FeatureName(s) [hue] doesn't exist in FeatureGroup [G_asked]."}], Records = only (G_other,"west")
  Actual: Errors = []

Cause 2b — batch-get-record-undefined-feature-name-refused-per-identifier:
  Request: bad group asked for FeatureNames ["tier","nickname"] over identifiers mem-ada and mem-bo; good group asked for ["tier"] over mem-cy
  Expected: Records = {(good,mem-cy): {tier: bronze}} only, plus one ValidationError row per refused identifier (mem-ada, mem-bo)
  Actual: Records = {(bad,mem-ada):{tier:gold}, (bad,mem-bo):{tier:silver}, (good,mem-cy):{tier:bronze}} — the undefined name was dropped and all three records served

Cause 2c — batch-get-record-every-identifier-refused-answers-errors-only:
  Expected Errors (sorted): [(G, "dev-7", "ValidationError", "Validation Error: Provided FeatureName(s) [voltage] doesn't exist in FeatureGroup [G]."), (G_missing, "dev-1", "ValidationError", "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name [arn:aws:sagemaker:<region>:<account>:feature-group/G_missing].")]
  Actual Errors: only the missing-feature-group row; the undefined-FeatureName row was absent

## Steps to Reproduce
1. CreateFeatureGroup G_a with RecordIdentifierFeatureName=key, EventTimeFeatureName=ts, FeatureDefinitions key(String)/ts(Fractional)/shade(String), OnlineStoreConfig.EnableOnlineStore=true; wait for FeatureGroupStatus=Created. Do the same for a second group G_b.
2. PutRecord two records into G_a (key=north and key=south) and one into G_b.
3. Cause 1: call BatchGetRecord with Identifiers=[{G_a,["north"]},{G_a,["south"]}]. Expect HTTP 400 ValidationError "Validation Error: identifier list contains at least two identifiers having same feature group name. Remove duplicates and try again."; the emulator answers 200 with both records. Control: the same two identifiers under ONE Identifiers entry must succeed (it does).
4. Cause 2: call BatchGetRecord with Identifiers=[{G_a, ["north","south"], FeatureNames:["shade","hue"]}, {G_b, ["<b-id>"], FeatureNames:["shade"]}] where "hue" is undefined in G_a. Expect HTTP 200 with Records holding only G_b's record and one Errors row per G_a identifier reading "Validation Error: Provided FeatureName(s) [hue] doesn't exist in FeatureGroup [G_a]."; the emulator returns Errors=[] and serves G_a's records too.
5. Cause 2 (all-refused form): call BatchGetRecord with one Identifiers entry naming a non-existent feature group and one naming G_a with an undefined FeatureName. Expect two Errors rows; the emulator returns only the missing-group row.
