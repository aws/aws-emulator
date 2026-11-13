Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.ProvisionedThroughput.ReadCapacityUnits
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.ProvisionedThroughput.WriteCapacityUnits
Issue-Related: 20261109T055139Z-dynamodb-throughput-and-capacity-reporting-diverges-across-updatetable.md
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# dynamodb UpdateTable reports the requested capacity immediately instead of the throughput in effect before the asynchronous move

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
One net-new root cause in the TableDescription a successful, capacity-moving UpdateTable returns.

Cause 1 of 1: UpdateTable's response reports the capacity figures the CALL REQUESTED rather than the figures in effect before the move it started, so every throughput number in the returned TableDescription jumps ahead of the service.

AWS: Smithy: UpdateTable -> UpdateTableOutput.TableDescription; UpdateTableInput.ProvisionedThroughput is documented as "The new provisioned throughput settings for the specified table or index", and the operation doc states "UpdateTable is an asynchronous operation; while it's executing, the table status changes from ACTIVE to UPDATING. While it's UPDATING, you can't issue another UpdateTable request. Use DescribeTable to monitor the table's status." The model does not state which generation of the figures the immediate response carries, so for that the requirement is recorded behavior only: in the recording, the UpdateTable that moves a table provisioned at the minimum (ReadCapacityUnits 1 / WriteCapacityUnits 1, warm throughput 1/1) answers 200 with TableDescription.ProvisionedThroughput.ReadCapacityUnits = 1, .WriteCapacityUnits = 1 and TableDescription.WarmThroughput.ReadUnitsPerSecond = 1, .WriteUnitsPerSecond = 1 — i.e. the settings still in force at the instant the call returns, not the ones it asked for. TableStatus is not in dispute (it matches), so the divergence is purely which generation of the capacity numbers the description carries. Hypothesis: because the move is asynchronous, the new figures become observable only once the table leaves UPDATING, which is what DescribeTable is for. The rule, not the recorded numbers: the response must report the table's pre-move read/write capacity and pre-move warm throughput, whatever they are for that table; 1/1 here is just this probe's table being provisioned at the model's floor (ProvisionedThroughput.ReadCapacityUnits/WriteCapacityUnits @range min 1).

Emulator: the handler writes the requested settings into the stored table and then serializes the description from that same stored table, so the response shows the post-move configuration at once. On the replayed step it answers 200 with TableDescription.ProvisionedThroughput.ReadCapacityUnits = 3 and .WriteCapacityUnits = 3 where the recording has 1 and 1, and TableDescription.WarmThroughput.ReadUnitsPerSecond = 3 and .WriteUnitsPerSecond = 3 where the recording has 1 and 1 — all four members shifted to the figures the request named. The module doc of the handler states this as a deliberate choice ("Either way the NEW configuration is in effect from the moment the call returns"), which is the divergence.

Why: a client that reads the capacity out of UpdateTable's own response sees a move AWS has not completed. Pollers written against AWS wait for UPDATING -> ACTIVE and re-read the capacity; against the emulator they see the target figures while the table is still UPDATING, so a test that asserts "capacity has not changed yet" fails, and capacity-cadence logic (DynamoDB limits decreases per UTC day) that diffs the response against its own target concludes the change already landed and skips or double-counts the next step. Scaling/drift tooling that treats UpdateTable's TableDescription as the authoritative current state records a configuration that is not yet in effect.

Fix: UpdateTable's response path in the operation handler — services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/update_table.rs, where the requested figures are committed to the stored table (table.provisioned_throughput / table.warm_throughput) and the TableDescription is then read back from it. The committed-state write and the description the call returns must be separated: keep the pre-move read/write capacity and pre-move warm throughput for the response the asynchronous call returns, and let the clock's UPDATING -> ACTIVE transition (crate::state::STATUS_TRANSITION) be what makes the new figures visible to DescribeTable. Note: the WarmThroughput halves of this step are also consistent with the warm-throughput aliasing described as Cause 2 of open issue 20261109T055139Z-dynamodb-throughput-and-capacity-reporting-diverges-across-updatetable.md (warm throughput reported as the provisioned units), but that issue does NOT carry this replay key, so this step is unexercised by its pointer and all four members here are the capacity-generation cause above.

Traces: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected#task#10 (UpdateTable, 200 -> 200; 1 step in the cluster).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (415 fixtures, 4399 steps, score 88.5%, emulator_up true, bootstrap_signal false). This is the only net-new divergence cluster — every other cluster is owned by an already-open issue.

Cluster: service=dynamodb|UpdateTable|status=200->200|keys=$.TableDescription.ProvisionedThroughput.ReadCapacityUnits,$.TableDescription.ProvisionedThroughput.WriteCapacityUnits,$.TableDescription.WarmThroughput.ReadUnitsPerSecond,$.TableDescription.WarmThroughput.WriteUnitsPerSecond
Fixture: dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected, step task#10, verdict mismatch, affected_count 1.

Expected (recorded AWS response, HTTP 200):
  $.TableDescription.ProvisionedThroughput.ReadCapacityUnits  = 1
  $.TableDescription.ProvisionedThroughput.WriteCapacityUnits = 1
  $.TableDescription.WarmThroughput.ReadUnitsPerSecond        = 1
  $.TableDescription.WarmThroughput.WriteUnitsPerSecond       = 1

Actual (emulator, HTTP 200):
  $.TableDescription.ProvisionedThroughput.ReadCapacityUnits  = 3
  $.TableDescription.ProvisionedThroughput.WriteCapacityUnits = 3
  $.TableDescription.WarmThroughput.ReadUnitsPerSecond        = 3
  $.TableDescription.WarmThroughput.WriteUnitsPerSecond       = 3

No status or error-code divergence on this step: both answer 200 and no other member of TableDescription (TableStatus included) is flagged, so only the generation of the four capacity numbers differs. Earlier steps of the same fixture (task#5 CreateTable refusal message, task#8 UpdateTable null-capacity validation message) diverge only in refusal wording and are owned by open issues 20261109T055139Z-dynamodb-validation-refusal-messages-are-emulator-authored-for.md and 20261110T232401Z-dynamodb-validation-and-not-found-refusal-messages-are.md.

Model evidence: smithy_query query=operation operation=UpdateTable, query=doc shape=UpdateTable, query=doc shape=UpdateTableInput member=ProvisionedThroughput, and query=shape shape=ProvisionedThroughput,WarmThroughput,ProvisionedThroughputDescription,TableWarmThroughputDescription on services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json.

Code evidence: services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/update_table.rs lines 504-513 (warm throughput resolved to follow the capacity this update sets) and lines 648-657 (table.provisioned_throughput / table.warm_throughput overwritten with the requested values before the description is read back); module doc lines 9-22 ("Either way the NEW configuration is in effect from the moment the call returns").

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from that checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Go to fixture dynamodb-create-and-update-table-provisioned-throughput-bounds-rejected, step task#10: a table provisioned at the minimum read/write capacity (and the same minimum warm throughput) receives an UpdateTable that raises its provisioned capacity; the call answers 200.
4. Inspect the returned TableDescription: AWS reports ProvisionedThroughput.ReadCapacityUnits/WriteCapacityUnits and WarmThroughput.ReadUnitsPerSecond/WriteUnitsPerSecond as the values in effect before the move (1/1 and 1/1 in the recording) while the table is UPDATING; the emulator reports the requested values (3/3 and 3/3).
5. Equivalent by hand: CreateTable with BillingMode PROVISIONED, ProvisionedThroughput {ReadCapacityUnits:1, WriteCapacityUnits:1} and WarmThroughput {ReadUnitsPerSecond:1, WriteUnitsPerSecond:1}; wait for ACTIVE; call UpdateTable raising ProvisionedThroughput to {3,3}; read the UpdateTable response's TableDescription before any DescribeTable poll.
