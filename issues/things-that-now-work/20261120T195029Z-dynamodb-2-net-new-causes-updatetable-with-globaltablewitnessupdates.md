Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.global-table-mrsc-strong-consistency-and-conditional-write-conflict dynamodb.legacy-expected-exists-value-comparator-matrix
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|UpdateTable|NotImplemented
Issue-Key: dynamodb.legacy-expected-exists-value-comparator-matrix|run#6|AssertionError|assert message == outcome[1], f'{label}: expected AWS exact ValidationException message, got {message!r}'
Issue-Commit: a3c668f6bb53d78c1d1232fe39d40072881572f4

# dynamodb: 2 net-new causes — UpdateTable with GlobalTableWitnessUpdates answers an unmodeled NotImplemented (the whole MRSC surface is unreachable), and legacy-Expected validation refusals drop AWS's "1 validation error detected: " envelope

> _Note: this issue was identified against the code base at commit `a3c668f6bb53d78c1d1232fe39d40072881572f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
Two net-new root causes in the `dynamodb` service. Each needs its own fix.

Cause 1 of 2: UpdateTable refuses the modeled `GlobalTableWitnessUpdates` member with an unmodeled NotImplemented scaffold error, so a multi-Region strong-consistency (MRSC) global table can never be formed.
AWS: Smithy — UpdateTableInput.GlobalTableWitnessUpdates targets GlobalTableWitnessGroupUpdateList (@length min 1, max 1) and is a first-class input member alongside ReplicaUpdates and MultiRegionConsistency; UpdateTable's modeled errors are InternalServerError, InvalidEndpointException, LimitExceededException, ResourceInUseException, ResourceNotFoundException (plus service-common ValidationException). There is no NotImplemented error and no 501 anywhere in the DynamoDB model. AWS applies the witness create (returning TableDescription) and DescribeTable then reports GlobalTableWitnesses[].WitnessStatus reaching ACTIVE, Replicas[].ReplicaStatus ACTIVE and MultiRegionConsistency 'STRONG' from every full-replica Region.
Emulator: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateTable operation: operation UpdateTable with GlobalTableWitnessUpdates is scaffolded but not implemented in this emulator` — an error code and status that exist nowhere in the model, so no SDK error class matches and no branch or retry fires. The refusal is raised on the first UpdateTable that names a witness, so everything the MRSC contract is made of is unreachable behind it: STRONG reported from the primary and the replica Region, a strongly consistent cross-Region read of a just-completed write, a stale cross-Region conditional write failing with ConditionalCheckFailedException 'The conditional request failed' against the cross-Region-latest version, and UpdateTimeToLive being refused with ValidationException 'Time To Live is not supported for global tables with MultiRegionConsistency set to STRONG.'
Why: a caller cannot create, describe or tear down an MRSC global table at all, and the 501/NotImplemented it gets back is not a DynamoDB error — error handling written against AWS falls through to an unhandled exception instead of a modeled 400-class branch.
This is the same shape as cause 3 of the open issue 20261029T180026Z-dynamodb-global-tables-replica-auto-scaling-indexed-query.md (UpdateTable with ReplicaUpdates), but that issue names only the ReplicaUpdates branch and does not carry this replay key; the witness branch is a separate code path and a separate fix.
Fix: the UpdateTable handler's GlobalTableWitnessUpdates branch and the witness state it maintains (DescribeTable's GlobalTableWitnesses projection and the MultiRegionConsistency/TTL rules that hang off it).
Traces: dynamodb.global-table-mrsc-strong-consistency-and-conditional-write-conflict.

Cause 2 of 2: a legacy `Expected` parameter-shape refusal is served without AWS's leading '1 validation error detected: ' envelope.
AWS: ValidationException (HTTP 400) is the refusal class for a malformed legacy Expected entry, and the wording is the whole contract (the model carries no message text). AWS words these as `1 validation error detected: One or more parameter values were invalid: <rule sentence> for Attribute: <attribute name>` — the envelope and the 'One or more parameter values were invalid: ' clause and the rule sentence are FIXED text; only the attribute name is taken from the request (the key of the Expected map). The rule sentences in this family are 'Value must be provided when Exists is true', 'Value cannot be used when Exists is false', 'Exists and ComparisonOperator cannot be used together', 'Value and AttributeValueList cannot be used together' and 'Value must be provided when Exists is null'.
Emulator: the code (ValidationException) and status are right and the sentence is right, but the envelope is missing: for PutItem with Expected={'attr': {'Exists': True}} it serves 'One or more parameter values were invalid: Value must be provided when Exists is true for Attribute: attr' where AWS serves '1 validation error detected: One or more parameter values were invalid: Value must be provided when Exists is true for Attribute: attr'. Note this is the INVERSE of the open issue 20261111T170613Z-dynamodb-non-constraint-validationexception-messages-are-wrapped-in.md (which reports the envelope being ADDED where AWS omits it) and is not covered by it: here AWS DOES use the envelope and the emulator omits it. The related fixed issue 20261111T221143Z-dynamodb-legacy-condition-and-operand-validation-reaches-the.md fixed other legacy-Expected verdicts and quotes the same envelope for them; this parameter-shape family still omits it.
Why: callers, CLIs and log-scrapers surface these strings verbatim and some match on them; a message that is not AWS's is a behavioural difference even when the status and code match.
Fix: the legacy Expected parameter-shape validation's message assembly (shared by PutItem, UpdateItem and DeleteItem) — render AWS's envelope for this refusal family.
Traces: dynamodb.legacy-expected-exists-value-comparator-matrix.

## Evidence
Verbatim from the replay:

Cause 1 — dynamodb.global-table-mrsc-strong-consistency-and-conditional-write-conflict:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateTable operation: operation UpdateTable with GlobalTableWitnessUpdates is scaffolded but not implemented in this emulator
  The call was UpdateTable(TableName=<table>, MultiRegionConsistency='STRONG', ReplicaUpdates=[{'Create': {'RegionName': <replica region>}}], GlobalTableWitnessUpdates=[{'Create': {'RegionName': <witness region>}}]) on an ACTIVE PAY_PER_REQUEST table. Expected: a TableDescription response, then DescribeTable reporting MultiRegionConsistency 'STRONG' with the replica ACTIVE and the witness ACTIVE. (The region names and table name are this run's values; the requirement is that a modeled UpdateTable member is honoured, or refused with a modeled error, never with NotImplemented/501.)

Cause 2 — dynamodb.legacy-expected-exists-value-comparator-matrix (case exists_true_no_value / PutItem):
  AssertionError: exists_true_no_value/Put: expected AWS exact ValidationException message, got 'One or more parameter values were invalid: Value must be provided when Exists is true for Attribute: attr'
  Expected: '1 validation error detected: One or more parameter values were invalid: Value must be provided when Exists is true for Attribute: attr'. Fixed text: '1 validation error detected: One or more parameter values were invalid: Value must be provided when Exists is true for Attribute: '; variable part: the attribute name ('attr' here, taken from the request's Expected map key). Code ValidationException and status 400 matched.

## Steps to Reproduce
Cause 1:
1. CreateTable(PAY_PER_REQUEST, pk HASH) in an MRSC-documented Region and wait for ACTIVE.
2. UpdateTable(TableName=<table>, MultiRegionConsistency='STRONG', ReplicaUpdates=[{Create:{RegionName:<second region>}}], GlobalTableWitnessUpdates=[{Create:{RegionName:<third region>}}]) — it must be accepted (or refused with a MODELED error), never with NotImplemented.
3. Poll DescribeTable until TableStatus ACTIVE, the replica ReplicaStatus ACTIVE and GlobalTableWitnesses[].WitnessStatus ACTIVE; assert MultiRegionConsistency == 'STRONG' from both the primary and the replica Region.
4. PutItem in the primary, then GetItem(ConsistentRead=True) in the replica Region: the write must be visible with no propagation wait. A stale conditional UpdateItem from the replica Region must fail ConditionalCheckFailedException 'The conditional request failed'.
5. UpdateTimeToLive on the table must be refused ValidationException 'Time To Live is not supported for global tables with MultiRegionConsistency set to STRONG.'
6. Tear down with UpdateTable(ReplicaUpdates=[{Delete:{...}}], GlobalTableWitnessUpdates=[{Delete:{...}}]).

Cause 2:
1. CreateTable(PAY_PER_REQUEST, pk HASH); PutItem an item carrying attribute `attr`.
2. PutItem (and likewise UpdateItem and DeleteItem) with Expected={'attr': {'Exists': True}} and no Value: the ValidationException message must be '1 validation error detected: One or more parameter values were invalid: Value must be provided when Exists is true for Attribute: attr'.
3. Repeat for the sibling parameter-shape refusals in the same family: {'Exists': False, 'Value': ...} ('Value cannot be used when Exists is false'), {'Exists': True/False, 'ComparisonOperator': 'EQ', 'AttributeValueList': [...]} ('Exists and ComparisonOperator cannot be used together'), {'Value': ..., 'ComparisonOperator': 'EQ', 'AttributeValueList': [...]} ('Value and AttributeValueList cannot be used together') and {} ('Value must be provided when Exists is null') — each with the same envelope, and the rejected call must leave the item untouched.
