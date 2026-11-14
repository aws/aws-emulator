Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted
Issue-Key: config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted|_await_history_items#6|AssertionError|raise AssertionError(
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# config__sqs: the stopped-recorder calibration never gets a configuration item — a running allSupported recorder records no AWS::SQS::Queue, so the absence window cannot be distinguished from a recorder that records nothing (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
ONE root cause. ONE CAUSE must be closed before submitting.

The check's service token is the composite suite token `config__sqs`; both underlying services (configservice, sqs) are bootstrapped and the defect belongs to configservice's recording pipeline.

Cause 1 of 1: the check's CALIBRATION leg — prove that a RUNNING recorder does record a queue, before stopping it and asserting that nothing is recorded while stopped — never succeeds: with a customer-managed recorder recording (allSupported=true, includeGlobalResourceTypes=false) and a delivery channel in place, GetResourceConfigHistory for a freshly created AWS::SQS::Queue stays empty for the whole 1500 s window.

AWS: recorded behavior only, with model grounding. Smithy (configservice): RecordingGroup.allSupported makes Config record configuration changes for all supported resource types; GetResourceConfigHistory{resourceType, resourceId required} -> {configurationItems, nextToken}, paginated, with ResourceNotDiscoveredException (errorKind client, httpError 400, documented "You have specified a resource that is either unknown or has not been discovered") for a key Config has recorded nothing for; ConfigurationItem carries configurationItemCaptureTime, configurationItemStatus (ConfigurationItemStatus = OK | ResourceDiscovered | ResourceNotRecorded | ResourceDeleted | ResourceDeletedNotRecorded), arn, resourceType, resourceId, resourceName, awsRegion and the JSON-encoded `configuration` string; StopConfigurationRecorder / StartConfigurationRecorder gate that recording. Recorded behavior: a queue created under a running allSupported recorder gets its first configuration item within ~1–2 minutes; a queue changed while the recorder is STOPPED gets no new item until the recorder is restarted, at which point the changed state is captured.

Emulator: the calibration item never appears. AssertionError: "the calibration configuration item for queue <queue url> not met within 1500s. Last result: []. Recorder status: [{'name': <recorder>, 'recording': True, 'lastStatus': 'Success', 'lastErrorCode': None, 'lastErrorMessage': None}]" (check line 612, helper _await_history_items at line 421). The recorder reports recording=true with lastStatus Success and no error, yet GetResourceConfigHistory returns an empty configurationItems list forever: Config's inventory only grows from an explicit client write through PutResourceConfig, so a queue created through the SQS service never enters it. Nothing is refused. Because the calibration never passes, the stopped-recorder assertions and the ResourceNotDiscoveredException-message assertion later in the same check are unreachable, and this issue claims nothing about them.

Related open issue, named for overlap but NOT relied on as cover (the suite did not mark this check OWNED by it — its replay key has changed from a 60 s harness timeout to this AssertionError at check line 612, so the key that issue carries no longer matches): 20261025T144945Z-config-the-configuration-recorder-never-records-resources-from.md. That issue itself notes this check "will still not fit the 60 s harness cap after the fix"; the check now runs to its own 1500 s budget and reports the emulator's state explicitly, which is net-new evidence that the recording side effect is still entirely absent.

Why: every negative Config assertion becomes vacuous. A stopped recorder and a recorder that records nothing are indistinguishable, so a client cannot verify that stopping recording actually suppresses capture, nor that restarting it resumes capture. Resource-inventory reads are indistinguishable from "the account has nothing" while the recorder advertises recording=true and lastStatus Success, so an inventory or compliance consumer reads empty forever with no error anywhere.

Fix: the configuration-recorder recording loop of the Config service — while a customer-managed recorder is recording and its recordingGroup covers the type, resources created/changed/deleted in the other emulated services must be captured into the recorder's inventory (the `discovered_resources` state behind services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs, today written only by the PutResourceConfig path) as configuration items with configurationItemStatus ResourceDiscovered on first capture and OK on subsequent states, and that capture must be gated on the recorder's running state so StopConfigurationRecorder genuinely suppresses it and StartConfigurationRecorder resumes it; src/ops/get_resource_config_history.rs already projects from that state.

Traces: config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted (check line 612, helper _await_history_items at line 421).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 70/113 checks passed, 29 mismatched / 14 errored). config__sqs is 0/4 (0%); the other three are OWNED and skipped, this one is marked neither ALREADY FILED nor OWNED.

[mismatch] config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted
  File "check_config__sqs_config_stopped_recorder_records_no_configuration_item_until_restarted.py", line 612, in run
    calib_items = _await_history_items(
  File "...", line 421, in _await_history_items
    raise AssertionError(
  AssertionError: the calibration configuration item for queue 'https://<host>/000000000000/sim2real-behav-cfg-q-3689269e4417' not met within 1500s. Last result: []. Recorder status: [{'name': 'sim2real-behav-cfg-recorder-c6472da7fed6', 'recording': True, 'lastStatus': 'Success', 'lastErrorCode': None, 'lastErrorMessage': None}]

(The queue URL host/port, the account id and the generated queue/recorder names are per-run values; the requirement is that a configuration item exists at all for a queue created under a running recorder.)

Check setup (from the replayed source): _DEFAULT_RECORDING_GROUP = {"allSupported": True, "includeGlobalResourceTypes": False}; the delivery bucket carries the AWSConfigBucketPermissionsCheck / AWSConfigBucketExistenceCheck / AWSConfigBucketDelivery statements; the recorder role has AWS_ConfigRole attached; the check first CALIBRATES that a running recorder records a created queue (budget 1500 s), then stops the recorder, mutates the queue, asserts no new item appears across an absence window, restarts the recorder and asserts the changed state is captured, and separately asserts GetResourceConfigHistory's ResourceNotDiscoveredException message for a never-created key.

Expected (AWS, recorded): the calibration item appears within ~1–2 minutes with configurationItemStatus ResourceDiscovered, arn equal to the queue's QueueArn and resourceName equal to the queue name. Actual (emulator): configurationItems stays [] for the whole 1500 s calibration budget while DescribeConfigurationRecorderStatus reports recording=true / lastStatus Success / no error. Nothing is refused.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): GetResourceConfigHistory{resourceType, resourceId required} -> {configurationItems, nextToken}, paginated, errors include ResourceNotDiscoveredException (client, 400); RecordingGroup{allSupported, includeGlobalResourceTypes, resourceTypes, ...}; ConfigurationItem{configurationItemCaptureTime, configurationItemStatus, arn, resourceType, resourceId, resourceName, awsRegion, configuration, relationships}.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-ga-prod-v1 and re-replay config__sqs.config-stopped-recorder-records-no-configuration-item-until-restarted.
3. Manually: iam create-role trusting config.amazonaws.com with arn:aws:iam::aws:policy/service-role/AWS_ConfigRole attached; s3 create-bucket for delivery and put-bucket-policy with the three AWSConfigBucket* statements; config put-configuration-recorder --configuration-recorder name=<rec>,roleARN=<role>,recordingGroup={allSupported=true,includeGlobalResourceTypes=false}; config put-delivery-channel --delivery-channel name=<chan>,s3BucketName=<bucket>; config start-configuration-recorder --configuration-recorder-name <rec>; confirm config describe-configuration-recorder-status reports recording=true and lastStatus Success.
4. sqs create-queue --queue-name <q>, then poll config get-resource-config-history --resource-type AWS::SQS::Queue --resource-id <queue url>: on AWS a configuration item appears within ~1–2 minutes; the emulator returns an empty configurationItems list (or ResourceNotDiscoveredException) indefinitely.
5. The rest of the scenario is then unreachable: config stop-configuration-recorder --configuration-recorder-name <rec>; sqs set-queue-attributes --attributes VisibilityTimeout=<new>; observe no new item (vacuous, since none ever appears); config start-configuration-recorder --configuration-recorder-name <rec> and observe the changed state is still never captured.
