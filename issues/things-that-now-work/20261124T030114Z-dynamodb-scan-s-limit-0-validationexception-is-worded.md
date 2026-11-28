Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.query-limit-zero-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.query-limit-zero-rejected|run#19|AssertionError|assert scan_message == scan_limit_message, f'unexpected Scan message {scan_message!r}'
Issue-Commit: c67fd313bd95bd11e1fae665856d19009c1b14ab

# dynamodb Scan's Limit=0 ValidationException is worded with Query's template (uppercase 'Limit', value omitted) instead of AWS's Scan wording

> _Note: this issue was identified against the code base at commit `c67fd313bd95bd11e1fae665856d19009c1b14ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Cause 1 of 1: Scan's refusal of an out-of-range Limit is serialized with the Query wording, so the member name is capitalized and the rejected value is not quoted into the sentence.

AWS: Smithy (services/service-com-amazonaws-dynamodb-dynamodb-20120810): Scan.Limit targets PositiveIntegerObject with @range min 1, so Limit=0 is refused by request validation with 400 ValidationException. Recorded behavior: for Scan, AWS words that constraint failure with the lowercase member name and the offending value quoted in the sentence — fixed text "1 validation error detected: Value '<value>' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1", where the only variable part is <value>, the Limit the request sent (0 in the replayed check). Query's wording for the same constraint on the same member differs deliberately: "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1" (capitalized member name, value not interpolated) — the emulator already serves Query correctly, and the check asserts that Query form passes.

Emulator: Scan(Limit=0) is refused with the correct status (400) and the correct code (ValidationException), but with Query's message text: "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1". The lowercase member name 'limit' is replaced by 'Limit' and the quoted rejected value ("Value '0' at") is missing.

Why: a client that parses or asserts on the refusal text — SDK test suites, retry/branching logic keyed on the member named in the validation sentence, and operator tooling that surfaces which request member AWS rejected — sees a member named 'Limit' and no offending value, so it reports a different member casing and cannot recover the rejected value from the message. Code-and-status-only clients are unaffected, which is why this is a wording-fidelity defect rather than a refusal defect.

Fix: Scan's input validation / validation-error message construction in the dynamodb service's Scan handler. It currently reuses the shared Query/Limit constraint message template; Scan needs its own template for this constraint (lowercase member name, value interpolated), leaving Query's existing wording unchanged.

Traces: dynamodb.query-limit-zero-rejected (the Query half of the check passes; the Scan assertion is the one that fails).

## Evidence
Failing check dynamodb.query-limit-zero-rejected:

AssertionError: unexpected Scan message "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1"
  (check_dynamodb_query_limit_zero_rejected.py line 77: assert scan_message == scan_limit_message)

Expected (AWS, Scan): code ValidationException, HTTP 400, message
  1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1
   - fixed text: "1 validation error detected: Value '" + <rejected Limit value> + "' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1"
   - variable part: the Limit value taken from the request (0 here)

Actual (emulator, Scan): code ValidationException, HTTP 400, message
  1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1

The same check's earlier assertions pass: Query(Limit=1) and Scan(Limit=1) each return Count=1, and Query(Limit=0) is refused with exactly AWS's Query wording ("Value at 'Limit' ..."), confirming the divergence is Scan-specific message text, not the constraint, the code or the status.

## Steps to Reproduce
1. CreateTable with KeySchema pk (HASH, S), BillingMode PAY_PER_REQUEST; wait for ACTIVE.
2. PutItem {pk: {S: 'lz-1'}}.
3. Positive controls: Query(KeyConditionExpression 'pk = :p', Limit=1) and Scan(Limit=1) -> each Count=1.
4. With client-side parameter validation disabled (botocore Config(parameter_validation=False)), call Query(TableName=t, KeyConditionExpression='pk = :p', ExpressionAttributeValues={':p':{'S':'lz-1'}}, Limit=0) -> 400 ValidationException, message "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1" (matches AWS).
5. Same raw client: Scan(TableName=t, Limit=0) -> observe the emulator returns the Query wording above; AWS returns "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1".
