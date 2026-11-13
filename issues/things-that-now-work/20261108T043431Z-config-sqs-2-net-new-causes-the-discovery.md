Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__sqs.config-batch-get-resource-config-returns-only-the-recorded-key-of-a-mixed-batch config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted
Issue-Key: config__sqs.config-batch-get-resource-config-returns-only-the-recorded-key-of-a-mixed-batch|run#14|AssertionError|assert items[0]["configurationItemStatus"] == "ResourceDiscovered", (
Issue-Key: config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted|_await_history_items#21|AssertionError|raise AssertionError(
Issue-Commit: 8a56b7f6504363f0f0ac4b5db6de2dce2a4412d6

# config__sqs: 2 net-new causes — the discovery ConfigurationItem is stamped configurationItemStatus OK instead of ResourceDiscovered, and a restarted recorder never resumes recording

> _Note: this issue was identified against the code base at commit `8a56b7f6504363f0f0ac4b5db6de2dce2a4412d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
Two net-new root causes in the AWS Config configuration-recorder ingest path. Both are grounded in failing behavioral checks whose replay keys are NOT carried by any open issue; the open issue 20261102T094631Z-config-sqs-a-recording-configuration-recorder-never-produces.md describes the earlier state where NO item appeared at all and does not cover either cause below (both checks now get past that point and fail on the item's content / on resumption).

Cause 1 of 2 — the first ConfigurationItem written for a resource discovered under a running recorder carries configurationItemStatus 'OK' instead of 'ResourceDiscovered'.
AWS: Smithy (configservice) models ConfigurationItem.configurationItemStatus as enum ConfigurationItemStatus with values OK, ResourceDiscovered, ResourceNotRecorded, ResourceDeleted, ResourceDeletedNotRecorded. The item Config writes when it first discovers a resource while the recorder is running is stamped ResourceDiscovered; 'OK' is the status of a subsequent change/periodic item for an already-discovered resource, and ResourceDeleted the final item after the resource is gone. Which value goes on which item is recorded behavior only — the model only fixes the value set.
Emulator: GetResourceConfigHistory / BatchGetResourceConfig return the item for a queue created under a started recorder with configurationItemStatus 'OK'. Replay: "AssertionError: expected the discovery item for a queue created under a running recorder, got 'OK'".
Why: a client that classifies inventory by configurationItemStatus (compliance tooling, drift reconciliation, anything that distinguishes 'first seen' from 'changed' from 'deleted') can never see a discovery event. Every resource looks like a steady-state update, so first-appearance alerting and ResourceDiscovered-filtered queries silently return nothing.
Fix: the configuration-recorder ingest path that mints the ConfigurationItem — the status stamping for the first item of a resource — and the projection served by GetResourceConfigHistory / BatchGetResourceConfig / SelectResourceConfig.
Traces: config__sqs.config-batch-get-resource-config-returns-only-the-recorded-key-of-a-mixed-batch.

Cause 2 of 2 — a recorder that is stopped and then started again never records anything further: no ConfigurationItem is produced for a change made after the restart, and the recorder's status stays lastStatus 'Pending' forever.
AWS: after StopConfigurationRecorder, changes are not recorded; after StartConfigurationRecorder is called again, Config resumes recording for the recording group and writes a ConfigurationItem for the resource's subsequent change, and DescribeConfigurationRecorderStatus reports recording=true with lastStatus reaching 'Success' (and lastStatusChangeTime advancing). Smithy models the Start/Stop operations and the ConfigurationRecorderStatus shape but says nothing about resumption, so the resumption itself is recorded behavior only.
Emulator: the restarted recorder reports recording=true but lastStatus pinned at 'Pending' with lastErrorCode/lastErrorMessage null, and no item ever appears for the post-restart change. Replay: "AssertionError: queue '<queue url>' is recorded once the recorder runs again and it changes not met within 480s. Last result: None. Recorder status: [{'name': '<recorder>', 'recording': True, 'lastStatus': 'Pending', 'lastErrorCode': None, 'lastErrorMessage': None}]" (the recorder name/queue URL are per-run values; the requirement is that an item appears and lastStatus leaves 'Pending').
Why: a client that pauses recording for a maintenance window and restarts it gets an account that looks healthy (recording=true) but is permanently blind — history, batch-get and SelectResourceConfig stay empty for everything that happens after the restart, which reads as 'nothing changed' rather than 'not recording'.
Fix: StartConfigurationRecorder's resume path and the recorder status machine (the transition out of 'Pending' to 'Success'), plus the ingest hook that re-subscribes the recording group after a stop.
Traces: config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted.

## Evidence
Cause 1 — Expected: the first ConfigurationItem for a queue created while the recorder is running has configurationItemStatus == 'ResourceDiscovered' (ConfigurationItemStatus enum: OK | ResourceDiscovered | ResourceNotRecorded | ResourceDeleted | ResourceDeletedNotRecorded). Actual: configurationItemStatus == 'OK'. Replay detail: AssertionError: expected the discovery item for a queue created under a running recorder, got 'OK' (check_config__sqs_config_batch_get_resource_config_returns_only_the_recorded_key_of_a_mixed_batch.py:681).

Cause 2 — Expected: after StopConfigurationRecorder, a queue attribute change, then StartConfigurationRecorder, GetResourceConfigHistory returns a ConfigurationItem reflecting the change and DescribeConfigurationRecorderStatus reports lastStatus 'Success'. Actual: no item within 480 s; status stays {'recording': True, 'lastStatus': 'Pending', 'lastErrorCode': None, 'lastErrorMessage': None}. Replay detail: AssertionError: queue '<queue url>' is recorded once the recorder runs again and it changes not met within 480s. Last result: None (check_config__sqs_config_stopped_recorder_records_no_configuration_item_until_restarted.py:421).

## Steps to Reproduce
Cause 1: PutConfigurationRecorder with a recording group covering AWS::SQS::Queue (allSupported=true), PutDeliveryChannel, StartConfigurationRecorder. CreateQueue. Poll GetResourceConfigHistory (or BatchGetResourceConfig with that resource key plus an unrecorded one) and read configurationItemStatus on the first item — it is 'OK', not 'ResourceDiscovered'.
Cause 2: with the same recorder running and a queue already recorded, call StopConfigurationRecorder, change a queue attribute (SetQueueAttributes), call StartConfigurationRecorder, then poll GetResourceConfigHistory for a new item and DescribeConfigurationRecorderStatus — no item appears and lastStatus never leaves 'Pending'.
Replay the two check ids in trace_ids.
