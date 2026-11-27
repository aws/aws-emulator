Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize-events.put-users-missing-required-schema-attribute-rejected personalize-events.put-users-property-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.personalizeevents#AmazonPersonalizeEvents
Issue-Key: personalize-events.put-users-missing-required-schema-attribute-rejected|_refusal#17|AssertionError|raise AssertionError(f"the call was expected to be refused but was accepted: {result!r}")
Issue-Key: personalize-events.put-users-property-type-mismatch-rejected|_client_error#15|AssertionError|raise AssertionError(f"expected a refusal, got an accepted call: {answer!r}")
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# PutUsers does not validate user properties against the Users dataset schema: missing required columns and type-mismatched values are accepted with HTTP 200

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutUsers

## Description
PutUsers in the emulator writes whatever `properties` JSON a caller supplies without ever consulting the schema of the Users dataset named by `datasetArn`. Two distinct schema-validation rules real Personalize Events enforces are therefore missing. Both causes are net-new (the two open personalize-events issues, 20261124T193003Z-personalize-events-dataset-export-of-put-ingested-records-names-its.md and 20261125T165618Z-personalize-events-put-ingestion-mode-export-of-an-interactions-dataset.md, are about dataset-export CSV column naming and do not cover PutUsers input validation).

Cause 1 of 2: a user whose `properties` omit a non-nullable schema column is accepted instead of refused. Failing check: personalize-events.put-users-missing-required-schema-attribute-rejected.
AWS: Smithy: PutUsers input `users` -> User.properties is a `SynthesizedJsonUserProperties` whose "keys use camel case names that match the fields in the schema for the Users dataset"; PutUsers models InvalidInputException (errorKind client, httpError 400). Per the Users-import contract (https://docs.aws.amazon.com/personalize/latest/dg/importing-users.html), a schema field declared with a non-union type (e.g. {"name": "TIER", "type": "string"}) is required of every user, so a PutUsers user that omits it is refused HTTP 400 InvalidInputException with message `Schema validation failed with <property>: missing required attribute`. Fixed text: `Schema validation failed with ` and `: missing required attribute`; the single variable part is the camel-case property name of the absent column, folded to lower case, taken from the request's schema/properties (the recorded check used `tier` for the TIER column).
Emulator: the same call answers HTTP 200 with an empty body — `{'ResponseMetadata': {'HTTPStatusCode': 200, ...'content-length': '0'...}}` — and the check fails at `_refusal(...)` with `AssertionError: the call was expected to be refused but was accepted`. No error is raised at all.

Cause 2 of 2: a property value whose JSON type does not match its schema column's declared type is accepted instead of refused. Failing check: personalize-events.put-users-property-type-mismatch-rejected.
AWS: Smithy: User.properties is modeled `mediaType: application/json`, and PutUsers models InvalidInputException (400). Personalize type-checks each property against the Users schema column of the matching name, so a JSON string sent for a column declared {"name": "MEMBER_SINCE", "type": "long"} is refused HTTP 400 InvalidInputException with message `Schema validation failed with <property>: type mismatch`. Fixed text: `Schema validation failed with ` and `: type mismatch`; the variable part is the offending property name folded to lower case, taken from the request (the recorded check used `membersince` for the camel-case `memberSince` property). Hypothesis (not stated by the model): one message per first offending property, as in Cause 1.
Emulator: the same call answers HTTP 200 with an empty body (`'HTTPStatusCode': 200`, `'content-length': '0'`), and the check fails at `_client_error(...)` with `AssertionError: expected a refusal, got an accepted call`.

Why: a client that relies on PutUsers rejecting malformed user metadata gets silent data corruption instead of a 400. Rows that real AWS would never admit land in the Users dataset, so a solution trained (or a dataset exported) from the emulator's state contains users with absent required columns and wrongly typed values, and the ingestion bug surfaces only much later, far from the call that caused it. Negative-path code that catches InvalidInputException to report a bad upstream record never runs, and callers cannot distinguish a validated write from an unvalidated one.

Fix: the PutUsers handler's input validation in the personalize-events service crate. It needs to resolve `datasetArn` to its dataset, load that dataset's schema (the Avro record the personalize CreateSchema stored), and validate every user's `properties` object against it before any state mutation: (a) every schema field whose declared type is not a union containing "null" must be present, else InvalidInputException/400 with the missing-required-attribute message; (b) each supplied property's JSON value type must match the column's declared Avro type (long/int accept JSON numbers, string accepts JSON strings), else InvalidInputException/400 with the type-mismatch message. The property-name-to-schema-field mapping is the camel-case/UPPER_SNAKE_CASE fold already documented on User.properties, and the name echoed in both messages is the lower-cased property name.

Traces: personalize-events.put-users-missing-required-schema-attribute-rejected (Cause 1), personalize-events.put-users-property-type-mismatch-rejected (Cause 2). Both checks first prove the happy path works (a well-formed user is accepted with 200) and then assert the refusal, so the fix must keep valid PutUsers calls answering 200 with an empty body.

## Evidence
Check personalize-events.put-users-missing-required-schema-attribute-rejected:
  Users schema fields: USER_ID string, TIER string (non-nullable, categorical), HOME_REGION ["null","string"].
  Request: put_users(datasetArn=<Users dataset>, users=[{"userId": "reader-002", "properties": "{\"homeRegion\": \"north\"}"}])  — TIER/`tier` omitted.
  Expected (AWS): ClientError, Code="InvalidInputException", Message="Schema validation failed with tier: missing required attribute", HTTP 400.
  Actual (emulator): HTTP 200, empty body — {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '0', ...}, 'RetryAttempts': 0}}; no exception raised.
  Failure: AssertionError: the call was expected to be refused but was accepted.

Check personalize-events.put-users-property-type-mismatch-rejected:
  Users schema fields: USER_ID string, MEMBER_SINCE long.
  Control request (accepted on both): properties = {"memberSince": 1700000000} -> HTTP 200.
  Diverging request: put_users(datasetArn=<Users dataset>, users=[{"userId": "patron-b", "properties": "{\"memberSince\": \"last spring\"}"}]).
  Expected (AWS): ClientError, Code="InvalidInputException", Message="Schema validation failed with membersince: type mismatch", HTTP 400.
  Actual (emulator): HTTP 200, empty body — {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '0', ...}, 'RetryAttempts': 0}}; no exception raised.
  Failure: AssertionError: expected a refusal, got an accepted call.

Model evidence (Smithy, com-amazonaws-personalizeevents-amazonpersonalizeevents.json):
  PutUsers: POST /users, success code 200, errors = InvalidInputException (client, 400), ResourceInUseException (409), ResourceNotFoundException (404).
  User.properties: target SynthesizedJsonUserProperties, mediaType application/json, length 1..24000, sensitive; doc: "The keys use camel case names that match the fields in the schema for the Users dataset."
  InvalidInputException: client error, httpError 400, member `message`.

Note: the error codes/messages above are the error class's wording; the property names (`tier`, `membersince`) are the variable part filled from the request's schema and properties, not fixed text.

## Steps to Reproduce
Cause 1 (missing required attribute):
1. personalize.CreateSchema with a Users record whose fields are USER_ID (string), TIER (string, categorical, non-nullable), HOME_REGION (["null","string"]).
2. personalize.CreateDatasetGroup; poll DescribeDatasetGroup to ACTIVE.
3. personalize.CreateDataset(datasetType="Users", schemaArn=<above>); poll DescribeDataset to ACTIVE.
4. personalize-events.PutUsers(datasetArn=<Users dataset>, users=[{userId:"reader-001", properties:'{"tier":"gold"}'}]) -> expect HTTP 200 (control; passes today).
5. personalize-events.PutUsers(datasetArn=<Users dataset>, users=[{userId:"reader-002", properties:'{"homeRegion":"north"}'}]).
   Expect: HTTP 400 InvalidInputException, Message "Schema validation failed with tier: missing required attribute".
   Observed: HTTP 200, empty body.
6. Re-issue step 5 with properties '{"homeRegion":"north","tier":"silver"}' -> HTTP 200 (must keep passing).

Cause 2 (type mismatch):
1. personalize.CreateSchema with a Users record whose fields are USER_ID (string) and MEMBER_SINCE (long).
2. personalize.CreateDatasetGroup -> ACTIVE; personalize.CreateDataset(datasetType="Users", schemaArn=<above>) -> ACTIVE.
3. personalize-events.PutUsers(datasetArn=<Users dataset>, users=[{userId:"patron-a", properties:'{"memberSince":1700000000}'}]) -> expect HTTP 200 (control; passes today).
4. personalize-events.PutUsers(datasetArn=<Users dataset>, users=[{userId:"patron-b", properties:'{"memberSince":"last spring"}'}]).
   Expect: HTTP 400 InvalidInputException, Message "Schema validation failed with membersince: type mismatch".
   Observed: HTTP 200, empty body.
5. Re-issue step 4 with properties '{"memberSince":1600000000}' -> HTTP 200 (must keep passing).

Replay: run_behavioral_suite and confirm both checks in trace_ids pass.
