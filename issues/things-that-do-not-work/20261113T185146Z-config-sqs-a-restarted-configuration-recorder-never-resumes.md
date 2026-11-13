Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted
Issue-Key: config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted|_await_history_items#21|AssertionError|raise AssertionError(
Issue-Commit: 2ea9db7899fab28ca5d17dc0ef7cfb8f8ea8eca4

# config__sqs: a restarted configuration recorder never resumes recording — no ConfigurationItem for a post-restart change and lastStatus stays 'Pending'

> _Note: this issue was identified against the code base at commit `2ea9db7899fab28ca5d17dc0ef7cfb8f8ea8eca4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartConfigurationRecorder

## Description
One net-new root cause in the AWS Config configuration-recorder resume path. NOTE: the resolved issue 20261108T043431Z-config-sqs-2-net-new-causes-the-discovery.md claimed this as its Cause 2, but the check still fails with the identical assertion and the identical recorder status, so the resume path is still broken; a resolved issue does not cover a still-failing check. The open issues 20261102T094631Z-config-sqs-a-recording-configuration-recorder-never-produces.md, 20261029T065729Z-config-sqs-the-stopped-recorder-calibration-never-gets.md and 20261029T184030Z-config-sqs-a-running-allsupported-recorder-still-produces.md describe the earlier state in which NO item appeared at all for a RUNNING recorder; they do not cover this cause — this check gets past that point (the calibration item for a queue created under the running recorder is produced, and the absence window under the stopped recorder refuses GetResourceConfigHistory with ResourceNotDiscoveredException and the exact AWS message) and fails only after the recorder is started again.

Cause 1 of 1 — after StopConfigurationRecorder followed by StartConfigurationRecorder, the recorder never records anything again: a subsequent change to a resource in the recording group produces no ConfigurationItem, and DescribeConfigurationRecorderStatus reports recording=true with lastStatus pinned at 'Pending'.

AWS: StopConfigurationRecorder suspends recording; StartConfigurationRecorder resumes it for the recorder's recording group, so a change made to a resource after the restart (here SetQueueAttributes on an AWS::SQS::Queue under an allSupported recording group) is captured as a new ConfigurationItem that GetResourceConfigHistory then returns, and DescribeConfigurationRecorderStatus reports recording=true with lastStatus leaving 'Pending' for 'Success' (lastStatusChangeTime advancing) once a recording event has been processed. Smithy (configservice): ConfigurationRecorderStatus.lastStatus targets enum RecorderStatus with values Pending | Success | Failure | NotApplicable, and ConfigurationItem.configurationItemStatus targets enum ConfigurationItemStatus (OK | ResourceDiscovered | ResourceNotRecorded | ResourceDeleted | ResourceDeletedNotRecorded); GetResourceConfigHistory models ResourceNotDiscoveredException. The model fixes those value sets but says nothing about resumption itself, so the resume behaviour and which status value lands when are recorded behaviour only: the recording shows an item for the post-restart change and a recorder whose lastStatus is no longer 'Pending'.

Emulator: the restarted recorder answers DescribeConfigurationRecorderStatus with {recording: true, lastStatus: 'Pending', lastErrorCode: null, lastErrorMessage: null} and no ConfigurationItem ever appears for the changed queue — GetResourceConfigHistory keeps behaving as if the resource were never discovered for the whole 480 s wait. Replay detail: "AssertionError: queue '<queue url>' is recorded once the recorder runs again and it changes not met within 480s. Last result: None. Recorder status: [{'name': '<recorder name>', 'recording': True, 'lastStatus': 'Pending', 'lastErrorCode': None, 'lastErrorMessage': None}]". The queue URL, recorder name and the 480 s budget are per-run values; the requirement is that (a) an item for the post-restart change becomes readable and (b) lastStatus leaves 'Pending' once the restarted recorder processes a recording event. The 'Pending' status is observable independently of how long the harness waited, since it is read at the moment of failure from the emulator's own DescribeConfigurationRecorderStatus response.

Why: a client that pauses recording for a maintenance window and then restarts it gets an account that looks healthy (recording=true) but is permanently blind: history, BatchGetResourceConfig and SelectResourceConfig stay empty for everything that changes after the restart, which a consumer reads as 'nothing changed' rather than 'not recording'. Anything gating on lastStatus == 'Success' to confirm the recorder came back also hangs forever on 'Pending'.

Fix: StartConfigurationRecorder's resume path — re-subscribing the recording group to the change-ingest hook after a stop — plus the recorder status machine that advances lastStatus/lastStatusChangeTime out of 'Pending' when a recording event is processed, and the ConfigurationItem the ingest path must mint for a change to an already-existing resource.

Traces: config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted.

## Evidence
Expected (AWS): StopConfigurationRecorder; CreateQueue (undiscovered, GetResourceConfigHistory refused ResourceNotDiscoveredException with message 'Resource <queue url> of resourceType:AWS::SQS::Queue is unknown or has not been discovered' — this leg passes); StartConfigurationRecorder; SetQueueAttributes(VisibilityTimeout) — GetResourceConfigHistory then returns a ConfigurationItem for the queue reflecting the changed attributes (and nothing backfilled for the stopped window), and DescribeConfigurationRecorderStatus reports lastStatus 'Success' (RecorderStatus enum: Pending | Success | Failure | NotApplicable).
Actual (emulator): no ConfigurationItem for the queue ever becomes readable after the restart and the change; DescribeConfigurationRecorderStatus keeps returning {'recording': True, 'lastStatus': 'Pending', 'lastErrorCode': None, 'lastErrorMessage': None}. Replay: "AssertionError: queue '<queue url>' is recorded once the recorder runs again and it changes not met within 480s. Last result: None. Recorder status: [{'name': '<recorder>', 'recording': True, 'lastStatus': 'Pending', 'lastErrorCode': None, 'lastErrorMessage': None}]" (check_config__sqs_config_stopped_recorder_records_no_configuration_item_until_restarted.py:421, raised from run:673). The calibration leg of the same check shows the RUNNING recorder does produce an item for a freshly created queue, so this is specifically the resume path, not recording in general.

## Steps to Reproduce
1. PutConfigurationRecorder with a service role and recording group {allSupported: true, includeGlobalResourceTypes: false}; PutDeliveryChannel against a bucket carrying the Config delivery policy; StartConfigurationRecorder.
2. CreateQueue and poll GetResourceConfigHistory(resourceType='AWS::SQS::Queue', resourceId=<queue url>) until an item appears (calibration — this succeeds).
3. StopConfigurationRecorder and wait for recording=false. CreateQueue a second queue; GetResourceConfigHistory for it is refused ResourceNotDiscoveredException throughout the absence window (correct).
4. StartConfigurationRecorder again and wait for recording=true. Call SetQueueAttributes on that second queue (e.g. VisibilityTimeout 151).
5. Poll GetResourceConfigHistory for the second queue: no ConfigurationItem ever appears. Call DescribeConfigurationRecorderStatus: lastStatus stays 'Pending' with no lastErrorCode/lastErrorMessage.
Replay the check id in trace_ids.
