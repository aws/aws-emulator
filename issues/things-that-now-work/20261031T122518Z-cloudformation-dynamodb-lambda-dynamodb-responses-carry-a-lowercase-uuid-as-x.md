Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid
Issue-Key: cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid|_assert_request_id#3|AssertionError|assert pattern.match(request_id), (
Issue-Commit: 1167ed563a70a94febca1f8f14340d372d95daee

# DynamoDB responses carry a lowercase UUID as x-amzn-RequestId instead of DynamoDB's uppercase alphanumeric request-id token

> _Note: this issue was identified against the code base at commit `1167ed563a70a94febca1f8f14340d372d95daee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
This issue carries 1 root cause.

Cause 1 of 1: every DynamoDB response (success envelope, and by construction the error envelope too) is stamped with a lowercase RFC-4122 UUID as its request id, whereas DynamoDB mints a long uppercase alphanumeric token. Failing check id: cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid.

AWS: recorded behavior only — the Smithy model does not model a request-id member for DynamoDB. smithy_query service_info on services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json reports sdkId "DynamoDB", endpointPrefix "dynamodb", protocol awsJson1_0; the request id therefore rides the HTTP response header x-amzn-RequestId only, and botocore surfaces it as ResponseMetadata.RequestId. The recorded AWS behavior the check measures (its own comment says the formats are "measured, not assumed") is that request-id FORMAT is per service: CloudFormation (awsQuery/XML) and Lambda (restJson) mint RFC-4122 UUIDs, while DynamoDB (awsJson1_0) mints a token of 40–64 characters drawn only from uppercase letters and digits (regex ^[A-Z0-9]{40,64}$). The fixed part of that requirement is the character class and the length band; the token value itself is a per-request value and is NOT part of the requirement — only its shape is. Real DynamoDB also mints a fresh id per response, never an empty string and never the all-zeros placeholder 00000000-0000-0000-0000-000000000000.

Emulator: ListTables(Limit=1) answers 200 with ResponseMetadata.RequestId = a lowercase UUID (the replay observed '22d901ea-b8f9-47a6-a9ae-071b92e73352'); that value is one run's id and is quoted only to show the SHAPE served — a 36-character lowercase hyphenated UUID. The emulator is evidently using one generic UUID request-id generator for every service rather than DynamoDB's own format, so no DynamoDB response can satisfy the uppercase-token shape. The check aborts on this first leg, so the remaining legs it would have judged (DynamoDB's error-envelope request id via DescribeTable on a non-existent table, and the "six calls, six distinct ids" assertion across cloudformation/dynamodb/lambda success and error surfaces) were never reached and are unproven either way.

Why: a client that keys on the request id gets a value no real DynamoDB endpoint would produce. Support-escalation plumbing, log correlation and test fixtures that validate or pattern-match DynamoDB request ids (uppercase/alphanumeric, 40–64 chars) reject or mis-parse every emulated response, and any code that distinguishes DynamoDB's token form from the UUID form used by CloudFormation/Lambda sees the wrong service identity.

Fix: belongs in the DynamoDB service crate's response serialization / response-metadata path — the x-amzn-RequestId header stamped on both the success and the error envelope for service-com-amazonaws-dynamodb-dynamodb-20120810 — or in the shared request-id minting the emulator's HTTP layer applies to all services, which must become per-service rather than a single UUID generator. The id must stay freshly generated per response (distinct ids across calls), non-empty, and never the all-zeros placeholder.

Traces: cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid (verdict: mismatch), assertion raised in _assert_request_id for the [dynamodb/success] leg.

## Evidence
Failing check: cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid [mismatch].

Assertion from the replay:
  AssertionError: [dynamodb/success] RequestId must be a 40-64 char uppercase alphanumeric token for this service; got '22d901ea-b8f9-47a6-a9ae-071b92e73352'. Formats differ per service -- measured, not assumed

Expected (AWS, recorded): DynamoDB ListTables returns 200 and its x-amzn-RequestId / ResponseMetadata.RequestId matches ^[A-Z0-9]{40,64}$ — uppercase letters and digits only, 40 to 64 characters, a different value on every response.
Actual (emulator): ListTables returns 200 with ResponseMetadata.RequestId in lowercase hyphenated UUID form (36 chars, e.g. the single run's '22d901ea-b8f9-47a6-a9ae-071b92e73352'), which fails the uppercase-token shape for every request regardless of value.

Not asserted here (the check stopped at the first leg): the DynamoDB error-surface request id, and that all six success/error request ids across cloudformation, dynamodb and lambda are distinct.

## Steps to Reproduce
1. Boot the emulator and point a boto3/botocore client at it with region us-west-2.
2. Call dynamodb.list_tables(Limit=1) and read resp["ResponseMetadata"]["RequestId"] (equivalently, the x-amzn-RequestId response header).
3. Observe a lowercase hyphenated UUID; real DynamoDB returns a 40-64 character token matching ^[A-Z0-9]{40,64}$.
4. For contrast, call cloudformation.list_stacks() and lambda.list_functions(MaxItems=1): those two services DO use the UUID form on AWS, so only the DynamoDB format is wrong.
5. Also call dynamodb.describe_table(TableName="no-such-table-<random>") and inspect ClientError.response["ResponseMetadata"]["RequestId"]: the error envelope must carry a fresh id in the same uppercase-token form, non-empty and never 00000000-0000-0000-0000-000000000000.
6. Replay check cloudformation__dynamodb__lambda.response-request-id-is-a-real-uuid to confirm the fix; it additionally requires that all six ids (three success, three error) are distinct.
