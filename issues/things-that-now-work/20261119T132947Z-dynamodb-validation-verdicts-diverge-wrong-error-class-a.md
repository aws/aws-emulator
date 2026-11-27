Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-condition-and-update-expression-operand-type-and-path-conflict-refusals
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-condition-and-update-expression-operand-type-and-path-conflict-refusals dynamodb-delete-item-request-member-type-mismatches-rejected dynamodb-query-return-consumed-capacity-and-limit-json-type-variants-rejected dynamodb-execute-statement-update-from-first-clause-order-and-absent-table-rejected dynamodb-execute-transaction-select-full-hash-key-attribute-exists
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DeleteItem|4xx->4xx|$.__type
Issue-Key: dynamodb|Query|4xx->4xx|$.__type
Issue-Key: dynamodb|ExecuteStatement|2xx->4xx|$.Items
Issue-Key: dynamodb|ExecuteStatement|2xx->4xx|status
Issue-Key: dynamodb|ExecuteTransaction|4xx->2xx|$.__type
Issue-Key: dynamodb|ExecuteTransaction|4xx->2xx|$.message
Issue-Key: dynamodb|ExecuteTransaction|4xx->2xx|status
Issue-Related: 20261109T055139Z-dynamodb-validation-refusal-messages-are-emulator-authored-for.md
Issue-Related: 20261110T232401Z-dynamodb-expression-diagnostics-4-net-new-causes-operand.md
Issue-Commit: 1d39de9b4cbdf24f97bce0e468a0bf0f3f7e1cf5

# DynamoDB validation verdicts diverge: wrong error class, a valid PartiQL read refused and an invalid transaction read accepted

> _Note: this issue was identified against the code base at commit `1d39de9b4cbdf24f97bce0e468a0bf0f3f7e1cf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteTransaction

## Description
Four root causes, all in the data plane's request-validation verdicts: the emulator reaches a different outcome than AWS for the same request — the wrong error class, an emulator-authored refusal of a request AWS accepts, or a 200 for a request AWS refuses.

Related open issues that do NOT cover these fixture steps (the replay reports these clusters as unowned): 20261117T221515Z-dynamodb-refusal-wording-and-validation-precedence-diverge-across.md, 20261111T221143Z-dynamodb-condition-key-and-filter-expression-defects-are.md, 20261111T170613Z-dynamodb-partiql-semantic-validation-index-qualified-select-update.md.

Cause 1 of 4: a ConditionExpression whose operand type is illegal for the function is evaluated instead of validated, so the caller gets a conditional failure rather than a validation error.
AWS: Smithy — DeleteItem models both ConditionalCheckFailedException and the service-common ValidationException; expression syntax/typing is validated before the condition is evaluated. begins_with() requires a String or Binary operand, so begins_with(attr, :v) with a Number operand is refused with HTTP 400, __type 'com.amazon.coral.validate#ValidationException' and the message '1 validation error detected: Invalid ConditionExpression: Incorrect operand type for operator or function; operator or function: <fn>, operand type: <T>' — the counted preamble and the two labelled clauses are fixed text, while <fn> is the offending operator/function from the expression and <T> is the AttributeValue type tag of the operand (N here).
Emulator: answers HTTP 400 with __type 'com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException' and message 'The conditional request failed' — i.e. it type-checks nothing and simply evaluates begins_with over a Number to false.
Why: the two outcomes mean opposite things to a client. A ConditionalCheckFailedException is the documented, expected, retry-with-new-state outcome of an optimistic write, so a caller treats a malformed expression as lost-race business logic and never surfaces the bug; code that distinguishes validation errors from condition failures (and that reads Item/attributes out of a ConditionalCheckFailedException) takes the wrong path.
Fix: the condition-expression compiler/validator used by the item-write handlers — operand type checking must run before evaluation and raise ValidationException.
Traces: dynamodb-condition-and-update-expression-operand-type-and-path-conflict-refusals#task#4 (also dynamodb-delete-item-request-member-type-mismatches-rejected#task#11).

Cause 2 of 4: a fractional Query Limit is refused by the deserializer with a Rust-internal message and an unqualified __type, pre-empting the table lookup AWS performs.
AWS: Smithy — QueryInput.Limit targets PositiveIntegerObject (integer, @range min 1); Query models ResourceNotFoundException (client, HTTP 400). A Query carrying a fractional Limit against a table that does not exist is answered with __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' and the message 'Requested resource not found' (Query's fixed, value-free wording): the number is coerced by the deserializer and the table-existence check decides the response.
Emulator: answers HTTP 400 with __type 'ValidationException' — not even a namespace-qualified coral/dynamodb type id — and the message 'float 23.5 cannot be represented as i64', an internal Rust numeric-conversion diagnostic that interpolates the request value.
Why: the client's not-found branch never fires, so a typo'd or deleted table looks like a request-shape problem; an unqualified __type breaks SDK error-shape resolution (the SDK cannot map it to a modeled exception and surfaces an unknown error); and the message leaks implementation internals instead of AWS's wording.
Fix: the awsJson1.0 request deserializer's number coercion for integer members (accept a fractional literal the way AWS does rather than failing the parse) and the error serializer's __type qualification, so the Query handler's table lookup runs and reports ResourceNotFoundException.
Traces: dynamodb-query-return-consumed-capacity-and-limit-json-type-variants-rejected#task#6.

Cause 3 of 4: a PartiQL ExecuteStatement that AWS answers with an empty result set is refused.
AWS: Smithy — ExecuteStatementOutput.Items is an ItemList; a SELECT that matches nothing is a success, answered HTTP 200 with Items []. Recorded behavior only for the specific statement: the recording shows the statement being accepted with an empty Items list and no error.
Emulator: answers HTTP 400 and no Items member at all, so the read is reported as a failure.
Why: a client doing a PartiQL read gets an exception where AWS returns 'no rows'; paging/aggregation loops abort, and an empty result is indistinguishable from a bad statement.
Fix: the ExecuteStatement handler's statement validation/planning — the empty-match case must fall through to a 200 with an empty Items list rather than a refusal.
Traces: dynamodb-execute-statement-update-from-first-clause-order-and-absent-table-rejected#task#15.

Cause 4 of 4: a SELECT inside ExecuteTransaction that does not pin the full primary key is accepted.
AWS: Smithy — ExecuteTransaction takes TransactStatements (1..100) and models ValidationException among the service-common errors; a transactional read must address a single item, so a SELECT whose WHERE clause does not specify the primary key is refused with HTTP 400, __type 'com.amazon.coral.validate#ValidationException' and the message 'Validation failed in TransactStatements[<i>]: Select statements within ExecuteTransaction must specify the primary key in the where clause.' — fixed text, with <i> the zero-based index of the offending statement taken from the request.
Emulator: answers HTTP 200 and executes the statement, with no __type/message at all.
Why: the emulator admits transactional reads AWS rejects, so a client's statement is never corrected and behaves differently against AWS; worse, the emulator can return multiple items for a slot AWS guarantees is a single-item read.
Fix: the ExecuteTransaction handler's per-statement semantic validation (full-primary-key requirement for SELECT), reporting the indexed ValidationException.
Traces: dynamodb-execute-transaction-select-full-hash-key-attribute-exists#task#4.

## Evidence
Wire replay of simparity-wire-aws-v1; expected = recorded AWS response, actual = emulator response.

Cause 1 — DeleteItem, fixture dynamodb-condition-and-update-expression-operand-type-and-path-conflict-refusals step task#4 (status 400 -> 400):
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException'
  $.message: expected '1 validation error detected: Invalid ConditionExpression: Incorrect operand type for operator or function; operator or function: begins_with, operand type: N', got 'The conditional request failed'
  (cluster also: dynamodb-delete-item-request-member-type-mismatches-rejected#task#11)

Cause 2 — Query, fixture dynamodb-query-return-consumed-capacity-and-limit-json-type-variants-rejected step task#6 (status 400 -> 400):
  $.__type: expected 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException', got 'ValidationException'
  $.message: expected 'Requested resource not found', got 'float 23.5 cannot be represented as i64'

Cause 3 — ExecuteStatement, fixture dynamodb-execute-statement-update-from-first-clause-order-and-absent-table-rejected step task#15:
  status: expected 200, got 400
  $.Items: missing (expected [], got None)

Cause 4 — ExecuteTransaction, fixture dynamodb-execute-transaction-select-full-hash-key-attribute-exists step task#4:
  status: expected 400, got 200
  $.__type: missing (expected 'com.amazon.coral.validate#ValidationException', got None)
  $.message: missing (expected 'Validation failed in TransactStatements[0]: Select statements within ExecuteTransaction must specify the primary key in the where clause.', got None)

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, and replay the wire corpus (replay_wire_corpus).
2. Cause 1: PutItem an item, then DeleteItem it with ConditionExpression 'begins_with(#a, :v)' where :v is {'N':'1'}. AWS -> 400 ValidationException '1 validation error detected: Invalid ConditionExpression: Incorrect operand type for operator or function; operator or function: begins_with, operand type: N'; the emulator -> 400 ConditionalCheckFailedException 'The conditional request failed'.
3. Cause 2: Query a table name that does not exist with Limit sent as a fractional JSON number. AWS -> 400 ResourceNotFoundException 'Requested resource not found'; the emulator -> 400 __type 'ValidationException' with 'float <n> cannot be represented as i64'.
4. Cause 3: replay dynamodb-execute-statement-update-from-first-clause-order-and-absent-table-rejected and compare step task#15: AWS -> 200 {'Items': []}, the emulator -> 400.
5. Cause 4: ExecuteTransaction with TransactStatements [{'Statement': "SELECT * FROM \"<table>\" WHERE <hashkey> = '<v>'"}] that does not pin the full primary key. AWS -> 400 ValidationException 'Validation failed in TransactStatements[0]: Select statements within ExecuteTransaction must specify the primary key in the where clause.'; the emulator -> 200.
6. Re-replay the four fixture#step bindings in trace_ids / Issue-Trace to confirm each cause closes.
