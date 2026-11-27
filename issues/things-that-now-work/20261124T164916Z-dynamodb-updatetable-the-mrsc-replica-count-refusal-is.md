Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.global-table-mrsc-unsupported-configuration-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.global-table-mrsc-unsupported-configuration-rejected|run#9|AssertionError|assert message == _WRONG_REPLICA_COUNT_MESSAGE, f"unexpected message {message!r}"
Issue-Commit: 9d6dbefbeeb79c5752051e10ea3d5ba278a8ab35

# dynamodb UpdateTable: the MRSC replica-count refusal is emulator-worded, not AWS's "Unsupported replica count for global tables with MultiRegionConsistency set to STRONG."

> _Note: this issue was identified against the code base at commit `9d6dbefbeeb79c5752051e10ea3d5ba278a8ab35`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
Cause 1 of 1: UpdateTable's refusal of a MultiRegionConsistency=STRONG request that does not span three Regions carries emulator-authored text instead of AWS's sentence.
Failing check: dynamodb.global-table-mrsc-unsupported-configuration-rejected.

AWS:      ValidationException (HTTP 400) is the refusal class, and for a message-only contract the wording IS the requirement (Smithy carries no message text). Smithy (dynamodb): UpdateTableInput.MultiRegionConsistency targets the MultiRegionConsistency enum and GlobalTableWitnessUpdates targets GlobalTableWitnessGroupUpdateList (@length min 1, max 1), both first-class members alongside ReplicaUpdates; UpdateTable's modeled errors are InternalServerError, InvalidEndpointException, LimitExceededException, ResourceInUseException and ResourceNotFoundException, plus the service-common ValidationException. An MRSC global table must span three Regions — three full replicas, or two replicas and one witness — so UpdateTable(MultiRegionConsistency='STRONG', ReplicaUpdates=[{Create:{RegionName:<one region>}}]) with no witness is refused outright, before any cross-Region provisioning starts, with Error.Message "Unsupported replica count for global tables with MultiRegionConsistency set to STRONG." (recorded behavior: the whole sentence is fixed text and carries no value from the request). The same table, with one replica AND one witness, is then accepted — so the refusal is the replica count, not MRSC itself. The sibling refusal in this family, a table carrying a local secondary index, is worded "Local secondary indexes are not supported for global tables with MultiRegionConsistency set to STRONG." (also fixed text); the check never reaches that leg, so it is unverified but belongs to the same message assembly.

Emulator: the code (ValidationException) and status are right and the refusal is correctly raised before anything is provisioned, but the text is the emulator's own: "One or more parameter values were invalid: A MRSC global table can be configured with either three replicas, or with two replicas and one witness" — it adds the generic 'One or more parameter values were invalid: ' clause AWS does not use for this refusal and restates the rule in its own words.

Why:      callers, CLIs and log scrapers surface these strings verbatim and some branch on them; a refusal whose message is not AWS's is a behavioural difference even when the status and code match, and an operator comparing emulator output to AWS documentation cannot match the error up. Because the check stops at the first wrong message, the accepted replica+witness leg and the LSI refusal behind it are also left unexercised.

Fix:      the MRSC validation's message assembly in the UpdateTable handler (the MultiRegionConsistency / GlobalTableWitnessUpdates branch) — serve AWS's sentences for this refusal family verbatim and without the 'One or more parameter values were invalid: ' clause.

Not covered by the open issue 20261120T195029Z-dynamodb-2-net-new-causes-updatetable-with-globaltablewitnessupdates.md, which reports the witness branch answering an unmodeled NotImplemented (the branch now answers a modeled ValidationException) and does not carry this check's replay key.

Traces:   dynamodb.global-table-mrsc-unsupported-configuration-rejected.

## Evidence
Failing check: [mismatch] dynamodb.global-table-mrsc-unsupported-configuration-rejected

The call: on an ACTIVE PAY_PER_REQUEST table with key schema pk HASH (no LSI, no replicas), UpdateTable(TableName=<table>, MultiRegionConsistency='STRONG', ReplicaUpdates=[{'Create': {'RegionName': <one other MRSC region>}}]) — one additional Region, no witness.

Expected: HTTP 400, Error.Code 'ValidationException', Error.Message 'Unsupported replica count for global tables with MultiRegionConsistency set to STRONG.' (entirely fixed text), and DescribeTable still showing Replicas == [] and no MultiRegionConsistency member.

Actual: AssertionError: unexpected message 'One or more parameter values were invalid: A MRSC global table can be configured with either three replicas, or with two replicas and one witness'. The code assertion (ValidationException) passed before it, so only the message diverges.

Unreached legs of the same check, behind that assertion: (a) on a table carrying a local secondary index, UpdateTable(MultiRegionConsistency='STRONG', ReplicaUpdates=[{Create}], GlobalTableWitnessUpdates=[{Create}]) must be refused ValidationException 'Local secondary indexes are not supported for global tables with MultiRegionConsistency set to STRONG.' with no replica or witness provisioned; (b) the first, LSI-free table must then ACCEPT STRONG with one replica plus one witness, and DescribeTable must report MultiRegionConsistency 'STRONG' with that replica and that witness listed.

The table name and the Region names are per-run values (any three of the MRSC-documented Regions may form the table); neither is part of the message.

## Steps to Reproduce
1. In an MRSC-documented Region, CreateTable(TableName=<t>, KeySchema [pk HASH], AttributeDefinitions [pk S], BillingMode PAY_PER_REQUEST); wait until ACTIVE.
2. UpdateTable(TableName=<t>, MultiRegionConsistency='STRONG', ReplicaUpdates=[{'Create': {'RegionName': <a second MRSC region>}}]) with no GlobalTableWitnessUpdates. Read Error.Code/Error.Message: expected ValidationException / 'Unsupported replica count for global tables with MultiRegionConsistency set to STRONG.'; the emulator serves 'One or more parameter values were invalid: A MRSC global table can be configured with either three replicas, or with two replicas and one witness'.
3. DescribeTable(<t>): Replicas must be [] and MultiRegionConsistency must be absent (this part already holds).
4. CreateTable a second table with pk HASH + sk RANGE and one LocalSecondaryIndex; once ACTIVE, UpdateTable(MultiRegionConsistency='STRONG', ReplicaUpdates=[{Create:{RegionName:<second region>}}], GlobalTableWitnessUpdates=[{Create:{RegionName:<third region>}}]) — expect ValidationException 'Local secondary indexes are not supported for global tables with MultiRegionConsistency set to STRONG.' with no replica or witness created.
5. On the first table, UpdateTable with one ReplicaUpdates Create and one GlobalTableWitnessUpdates Create: it must be accepted and DescribeTable must converge on MultiRegionConsistency 'STRONG' with that replica and witness.
6. Re-replay dynamodb.global-table-mrsc-unsupported-configuration-rejected.
