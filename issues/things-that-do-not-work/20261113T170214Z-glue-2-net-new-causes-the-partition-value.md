Issue-Source: trace-sweep-divergence
Issue-Trace: glue-gtr-partition-reads-on-the-partitioned-table
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-gtr-partition-reads-on-the-partitioned-table glue-tsd-partition-and-version-side-reads glue-partition-index-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetPartitions|4xx->4xx|$.Message
Issue-Key: glue|GetPartitionIndexes|2xx->2xx|$.PartitionIndexDescriptorList
Issue-Related: 20261105T074444Z-glue-partition-indexes-never-become-visible-getpartitionindexes-always.md
Issue-Commit: 277514097c80218b4615433c58b2aa2bb65c3a6a

# glue: 2 net-new causes — the partition-value type refusal Message is emulator-authored, and a deleted partition index stays ACTIVE in GetPartitionIndexes

> _Note: this issue was identified against the code base at commit `277514097c80218b4615433c58b2aa2bb65c3a6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPartitions, GetPartitionIndexes

## Description
Two net-new root causes in the glue service, both on the table-partition surface of the Data Catalog. Neither is marked ALREADY FILED (no stub_path) nor OWNED by an open issue in the replay report, so neither carries a wire replay key today.

Cause 1 of 2 — GetPartitions refuses a partition-key comparison against a value that is not of the key's declared type with an emulator-authored Message instead of the text real Glue returns. Fixture glue-gtr-partition-reads-on-the-partitioned-table, step task#5 (cluster also covers task#6 of the same fixture and glue-tsd-partition-and-version-side-reads#task#5).
AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) GetPartitions takes Expression -> PredicateString (@length min 0, max 2048) and models InvalidInputException (errorKind client, httpError 400 defaulted). Recorded behavior: when the Expression compares a partition key whose declared Type is a temporal/numeric type against a literal that cannot be read as that type, Glue answers HTTP 400 with __type InvalidInputException and a Message that is the SERVER-SIDE exception class name followed by the offending literal and the type it failed to parse as: 'com.amazonaws.daylight.core.exceptions.InvalidPartitionValueException: <literal> is not a <type>.' The fixed text of that message is the class prefix 'com.amazonaws.daylight.core.exceptions.InvalidPartitionValueException: ', the connective ' is not a ', and the terminating '.'; the VARIABLE parts are the literal taken verbatim from the request's Expression and the partition key's declared type as the table stores it. Hypothesis: for key types other than the recorded 'date' the same template is used with that type name substituted (only the date case is recorded). Note this is a DIFFERENT refusal class from a malformed or non-partition-column Expression, which real Glue refuses with the fixed 'Invalid partition expression!'; that class is the subject of open issue 20261113T104338Z-glue-2-net-new-causes-the-partition-expression.md, which is bound to behavioral check ids only, does not carry these wire fixture#step keys, and does not describe this per-value type refusal — so it does not cover this cause, though both land in the same validator.
Emulator: status 400 and code InvalidInputException are right, but the Message is the emulator's own sentence describing the rule rather than AWS's refusal: 'Expression compares partition key <key> against a value that is not of its type <type>' (observed with key 'eventdate' and type 'date'; those two are per-run values from the request and the stored table, the sentence template is the defect). No class-name prefix, the offending literal is not named, and the wording is not AWS's.
Why: a client that classifies Glue refusals by Message — error handlers that distinguish a bad partition VALUE from a bad partition EXPRESSION, retry logic, log and alarm matchers, and tooling that surfaces the server exception class to the user — sees a string AWS never produces, so a correctly refused call is reported as an unknown failure and the specific InvalidPartitionValueException condition cannot be detected at all.
Fix: the shared partition-filter validator reached from the GetPartitions handler — the bind-time refusal in crate::state::ParsedPartitionFilter (its Refusal::message) whose result services/service-com-amazonaws-glue-awsglue/src/ops/get_partitions.rs raises as InvalidInputException. It must emit AWS's class-prefixed InvalidPartitionValueException message, with the request's literal and the key's declared type substituted, for the value-not-of-type case, and keep the expression-level refusal separate.
Traces: glue-gtr-partition-reads-on-the-partitioned-table#task#5, glue-gtr-partition-reads-on-the-partitioned-table#task#6, glue-tsd-partition-and-version-side-reads#task#5; code: services/service-com-amazonaws-glue-awsglue/src/ops/get_partitions.rs lines 400-416 (bind refusal lifted to InvalidInputException).

Cause 2 of 2 — a partition index that has been deleted is still reported by GetPartitionIndexes, which keeps listing it with IndexStatus ACTIVE forever. Fixture glue-partition-index-lifecycle, step task#7.
AWS: Smithy DeletePartitionIndex is documented 'Deletes a specified partition index from an existing table', takes required DatabaseName, TableName and IndexName, returns an empty output and models EntityNotFoundException/InvalidInputException/ConflictException (all httpError 400). GetPartitionIndexes returns PartitionIndexDescriptorList of PartitionIndexDescriptor{IndexName, Keys, IndexStatus} where IndexStatus is the PartitionIndexStatus enum including DELETING ('The index is deleting'). Recorded behavior: after the index registered on a table is deleted, GetPartitionIndexes(DatabaseName, TableName) for that table settles to an EMPTY PartitionIndexDescriptorList — the descriptor disappears (optionally passing through DELETING first), which is what the lifecycle fixture polls for.
Emulator: GetPartitionIndexes answers HTTP 200 and keeps returning one descriptor for the deleted index with IndexStatus 'ACTIVE' for the whole poll (21 attempts, 60s cap) — expected {'PartitionIndexDescriptorList': []}, observed the index descriptor still present. The read path and the store are fine (services/service-com-amazonaws-glue-awsglue/src/ops/get_partition_indexes.rs reads crate::state::partition_indexes::PartitionIndexStore); what is missing is the REMOVAL: services/service-com-amazonaws-glue-awsglue/src/ops/delete_partition_index.rs is still the scaffold (its handle() ends in Scaffolded::not_implemented("DeletePartitionIndex") at line 167) and writes nothing, and crate::state::partition_indexes models only CREATING/ACTIVE/FAILED with no delete/DELETING transition, so nothing can ever take a record out of the store short of dropping the whole table (forget_tables).
Why: the partition-index lifecycle cannot be completed against the emulator. A client that removes an index and then waits for it to disappear (teardown, index replacement under the same IndexName, query-planning logic that must see no index) waits forever and reads a deleted resource as ACTIVE; re-registering the same IndexName will also look like a conflict against an index that no longer exists. The emulator reports a resource it was told to delete as healthy, which is a false success.
Fix: implement the DeletePartitionIndex handler (services/service-com-amazonaws-glue-awsglue/src/ops/delete_partition_index.rs) so an accepted delete removes the named record from crate::state::partition_indexes::PartitionIndexStore (adding the removal, and the DELETING status, to that store), refuse an unknown catalog/database/table/IndexName with EntityNotFoundException, and delete the SCAFFOLDED marker; the existing GetPartitionIndexes read path then reports the empty list with no change. Related, for the fixer: open issue 20261105T074444Z-glue-partition-indexes-never-become-visible-getpartitionindexes-always.md covers the CREATE side of the same store (indexes never appearing) and does not carry this fixture#step key — the index now appears, which is why this delete-side gap is newly visible, and that issue does not cover it.
Traces: glue-partition-index-lifecycle#task#7; code: services/service-com-amazonaws-glue-awsglue/src/ops/delete_partition_index.rs line 167, services/service-com-amazonaws-glue-awsglue/src/state/partition_indexes.rs lines 42-60.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false, service glue).

Cause 1 — glue-gtr-partition-reads-on-the-partitioned-table#task#5, GetPartitions, expected 400 / observed 400, __type InvalidInputException on both sides, only the Message diverges:
  $.Message: value_mismatch
    expected 'com.amazonaws.daylight.core.exceptions.InvalidPartitionValueException: yesterday is not a date.'
    got      'Expression compares partition key eventdate against a value that is not of its type date'
  Fixed text of the requirement: 'com.amazonaws.daylight.core.exceptions.InvalidPartitionValueException: ' + ' is not a ' + '.'. Variable parts: the literal from the request's Expression ('yesterday' in this run) and the partition key's declared type ('date' in this run, from the stored table). The partition key name ('eventdate') appears only in the emulator's text, not in AWS's.
  Same cluster (divergence_key service=glue|GetPartitions|status=400->400|keys=$.Message), affected_count 3:
    glue-gtr-partition-reads-on-the-partitioned-table#task#5 (representative)
    glue-gtr-partition-reads-on-the-partitioned-table#task#6
    glue-tsd-partition-and-version-side-reads#task#5

Cause 2 — glue-partition-index-lifecycle#task#7, GetPartitionIndexes (polled), expected 200 / observed 200:
  $: poll capped at 60s (expected {'PartitionIndexDescriptorList': []}, got {'PartitionIndexDescriptorList': [{'IndexName': 'yearidx', 'Keys': [{'Name': 'year', 'Type': 'string'}], 'IndexStatus': 'ACTIVE'}]})
  $.PartitionIndexDescriptorList: poll: 1 item(s), not 0 after 21 attempt(s)
  $.PartitionIndexDescriptorList[0]: extra (expected None, got the descriptor above)
  The index name 'yearidx' and key 'year' are the fixture's per-run values; the requirement is that after the index is deleted the list for that table becomes empty.
  Supporting code evidence: services/service-com-amazonaws-glue-awsglue/src/ops/delete_partition_index.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and returns crate::scaffolded::Scaffolded::not_implemented("DeletePartitionIndex") (line 167), so the delete never reaches crate::state::partition_indexes::PartitionIndexStore, which itself exposes no removal and no DELETING status (only INDEX_STATUS_CREATING / ACTIVE / FAILED, lines 42-60).

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).

Cause 1:
1. Inspect glue-gtr-partition-reads-on-the-partitioned-table#task#5 (and #task#6, and glue-tsd-partition-and-version-side-reads#task#5).
2. Minimal reproduction: glue CreateDatabase; CreateTable on it with PartitionKeys=[{Name:'eventdate', Type:'date'}] and at least one partition registered; then GetPartitions(DatabaseName=<db>, TableName=<table>, Expression="eventdate = 'yesterday'").
3. Expect HTTP 400, __type InvalidInputException, Message 'com.amazonaws.daylight.core.exceptions.InvalidPartitionValueException: yesterday is not a date.' — i.e. the class prefix, then the request's literal, ' is not a ', the key's declared type and '.'.
4. Observe instead 'Expression compares partition key eventdate against a value that is not of its type date'.
5. Control that must stay correct: a well-formed comparison on the same key (e.g. "eventdate = '2024-01-01'") must still return the matching partitions with 200, and a malformed or non-partition-column Expression must stay its own refusal class.
6. Re-replay glue-gtr-partition-reads-on-the-partitioned-table#task#5 to confirm.

Cause 2:
1. Inspect glue-partition-index-lifecycle#task#7 — after the fixture registers a partition index on the table, polls it to ACTIVE and then deletes it, GetPartitionIndexes must settle to an empty PartitionIndexDescriptorList; observe the deleted index still listed with IndexStatus 'ACTIVE' for all 21 attempts until the 60s cap.
2. Minimal reproduction: CreateDatabase; CreateTable with PartitionKeys=[{Name:'year', Type:'string'}]; register a partition index named 'yearidx' on it; GetPartitionIndexes -> one descriptor, IndexStatus reaching 'ACTIVE'; DeletePartitionIndex(DatabaseName=<db>, TableName=<table>, IndexName='yearidx') -> expect 200 with an empty body (observe 501 NotImplemented from the scaffold); then GetPartitionIndexes -> expect PartitionIndexDescriptorList == [] (observe the index still present and ACTIVE).
3. Also check DeletePartitionIndex for an IndexName the table does not hold: expect HTTP 400 EntityNotFoundException.
4. Control that must stay correct: deleting the TABLE must keep dropping its index records (PartitionIndexStore::forget_tables), and an index that was never deleted must still be listed.
5. Re-replay glue-partition-index-lifecycle#task#7 to confirm.
