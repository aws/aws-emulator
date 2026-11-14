Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.associate-disassociate-resource-types config.external-evaluation-requires-process-check-rule config.put-external-evaluation-non-process-check-rule-rejected config.deliver-config-snapshot
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.associate-disassociate-resource-types|run#10|AssertionError|assert code_message == (
Issue-Key: config.external-evaluation-requires-process-check-rule|run#18|AssertionError|assert message == (
Issue-Key: config.put-external-evaluation-non-process-check-rule-rejected|run#12|AssertionError|assert code_message == (
Issue-Key: config.deliver-config-snapshot|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: d20f3863f60340619cdbd48f5982ebeef462a933

# config: 3 net-new behavioral root causes — AssociateResourceTypes' ValidationException message is not AWS's wording, PutExternalEvaluation's non-process-check refusal uses the generic parameter message, and DeliverConfigSnapshot's delivery never reaches a terminal status

> _Note: this issue was identified against the code base at commit `d20f3863f60340619cdbd48f5982ebeef462a933`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeliverConfigSnapshot

## Description
THREE CAUSES must be closed. All three are net-new: none of them is covered by the open Config issues that neighbour them (20261022T190340Z-config-has-no-recording-delivery-side-effects-and.md, 20261025T014224Z-cloudformation-config-s3-snapshot-delivery-never-reaches-a.md, 20261025T133755Z-config-aggregate-compliance-summary-still-never-converges-deleteevaluationresults.md, 20261025T202833Z-config-the-auto-remediation-end-to-end-flow.md, 20261025T223403Z-config-putresourceconfig-accepts-an-unregistered-cloudformation-resource-type.md), which carry other operations' replay keys.

Cause 1 of 3: AssociateResourceTypes refuses an ALL_SUPPORTED-strategy recorder with the right code but the wrong message.
AWS: AssociateResourceTypes(ConfigurationRecorderArn, ResourceTypes) is modelled with ValidationException (Smithy: AssociateResourceTypes errors = ConflictException | NoSuchConfigurationRecorderException | ValidationException, errorKind client, httpError 400). Recorded behavior: when the named recorder's recording strategy is ALL_SUPPORTED_RESOURCE_TYPES, the call is refused 400 ValidationException with the message "The operation failed. configuration recorder may have a recording strategy that does not permit association or disassociation of resource types." That whole string is FIXED text for this error class — it carries no value taken from the request or from state, so there is nothing in it to template.
Emulator: the status and the error code are right (400 / ValidationException), but the message is "Your configuraiton recorder has a recording strategy that does not allow the association or disassociation of resource types." — different wording, and it also misspells "configuration" as "configuraiton".
Why: a client that classifies this refusal on message text (or that surfaces it to an operator, or asserts on it in a test) sees a sentence that never occurs in AWS; the misspelling makes any substring match on "configuration recorder" fail as well. The code is already correct, so nothing else distinguishes this refusal.
Fix: the AssociateResourceTypes handler's validation message for the recording-strategy refusal, in services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/associate_resource_types.rs (and the identical refusal on the DisassociateResourceTypes twin, .../src/ops/disassociate_resource_types.rs, which the model gives the same ValidationException).
Traces: config.associate-disassociate-resource-types.

Cause 2 of 3: PutExternalEvaluation against an existing non-process-check rule answers the generic invalid-parameter sentence instead of the type-specific one.
AWS: PutExternalEvaluation(ConfigRuleName, ExternalEvaluation) is modelled with exactly two client errors, InvalidParameterValueException and NoSuchConfigRuleException (both errorKind client, httpError 400). Recorded behavior: the two refusal conditions are distinct. A rule that does not exist is NoSuchConfigRuleException with "The ConfigRule '<ConfigRuleName>' provided in the request is invalid. Please check the configRule name." — fixed text with the requested rule name substituted. A rule that DOES exist but is not a process check (Source Owner/SourceIdentifier other than AWS_CONFIG_PROCESS_CHECK) is InvalidParameterValueException with "This API can only be applied to rules with sourceIdentifier as AWS_CONFIG_PROCESS_CHECK." — entirely fixed text, no request or state values in it.
Emulator: the code is correct (InvalidParameterValueException) but the message is the generic "One or more of the specified parameters are not valid. Verify that your parameters are valid and try again.", which says nothing about the rule's type. The not-found leg's message is already correct, so only the type leg diverges.
Why: both refusals share one error code, so the message is the ONLY thing that tells a client whether its rule is missing or merely of the wrong type. With the generic sentence a caller cannot tell "create the process-check rule" from "you may not post external evaluations to this rule at all", and retry/remediation logic keyed on that distinction takes the wrong branch.
Fix: the PutExternalEvaluation handler's rule-type validation message, in services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_external_evaluation.rs — the branch that detects a target rule whose sourceIdentifier is not AWS_CONFIG_PROCESS_CHECK must emit the type-specific sentence rather than falling through to the crate's generic invalid-parameter text.
Traces: config.external-evaluation-requires-process-check-rule, config.put-external-evaluation-non-process-check-rule-rejected.

Cause 3 of 3: DeliverConfigSnapshot's snapshot delivery never reaches a terminal lastStatus and nothing is written to the delivery bucket.
AWS: DeliverConfigSnapshot(deliveryChannelName) is modelled to return configSnapshotId and to raise NoAvailableConfigurationRecorderException | NoRunningConfigurationRecorderException | NoSuchDeliveryChannelException (all client, httpError 400). Recorded behavior: with a recorder that is running and a delivery channel whose bucket policy grants config.amazonaws.com s3:GetBucketAcl, s3:ListBucket and s3:PutObject, the call returns a configSnapshotId, DescribeDeliveryChannelStatus's configSnapshotDeliveryInfo then advances to a TERMINAL lastStatus of SUCCESS, and a non-empty snapshot object whose key contains that configSnapshotId appears in the channel's bucket.
Emulator: the check never completes — it hangs to the 60 s per-check cap ("Check timed out after 60.0s"). The only unbounded waits in the check are the polls for configSnapshotDeliveryInfo.lastStatus to become SUCCESS or FAILURE and for the snapshot object to appear in the bucket; the recorder-start poll is bounded at 30 s and the preceding refusal legs are single calls. Hypothesis: DeliverConfigSnapshot's accepted call records no delivery attempt at all, so configSnapshotDeliveryInfo stays absent/non-terminal forever and no object is written.
Why: an on-demand snapshot is Config's only synchronous delivery trigger. A client that calls DeliverConfigSnapshot and then waits on DescribeDeliveryChannelStatus for a terminal lastStatus waits forever — it cannot distinguish "still in flight" from "delivery is not implemented", and it never observes the FAILURE status that a misconfigured bucket policy is supposed to produce either.
Fix: the DeliverConfigSnapshot handler and the delivery-channel status state it must update, in services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/deliver_config_snapshot.rs plus the delivery-channel status read in .../src/ops/describe_delivery_channel_status.rs and the channel's state module: the accepted call must perform (or schedule) the S3 write for the returned configSnapshotId and drive configSnapshotDeliveryInfo to a terminal lastStatus. Distinct from the already-open periodic/CloudFormation delivery issues: the operation whose replay key is unrun here is DeliverConfigSnapshot itself.
Traces: config.deliver-config-snapshot.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving; bootstrap_signal false; 65/117 checks passed, so config does answer).

Cause 1 — config.associate-disassociate-resource-types:
  check_config_associate_disassociate_resource_types.py line 249
    assert code_message == (
      "The operation failed. configuration recorder may have a recording strategy that "
      "does not permit association or disassociation of resource types."
    ), code_message
  AssertionError: Your configuraiton recorder has a recording strategy that does not allow the association or disassociation of resource types.
  Expected message: "The operation failed. configuration recorder may have a recording strategy that does not permit association or disassociation of resource types."
  Actual message:   "Your configuraiton recorder has a recording strategy that does not allow the association or disassociation of resource types."
  Status/code (400 / ValidationException) matched — the assertion on `code == "ValidationException"` on the preceding line passed.

Cause 2 — config.put-external-evaluation-non-process-check-rule-rejected:
  check_config_put_external_evaluation_non_process_check_rule_rejected.py line 81
    assert code_message == (
      "This API can only be applied to rules with sourceIdentifier as AWS_CONFIG_PROCESS_CHECK."
    ), code_message
  AssertionError: One or more of the specified parameters are not valid. Verify that your parameters are valid and try again.
  and config.external-evaluation-requires-process-check-rule:
  check_config_external_evaluation_requires_process_check_rule.py line 296, same expected string
  AssertionError: One or more of the specified parameters are not valid. Verify that your parameters are valid and try again.
  In that second check the NoSuchConfigRuleException leg (absent rule, message "The ConfigRule '<name>' provided in the request is invalid. Please check the configRule name.") passed first, so the emulator does separate the two codes — only the type-refusal message is wrong.

Cause 3 — config.deliver-config-snapshot:
  Error: Check timed out after 60.0s
  The check: _create_recorder → _create_delivery_bucket → _create_delivery_channel → DeliverConfigSnapshot refused NoRunningConfigurationRecorderException → StartConfigurationRecorder → _poll_until(recording is True, timeout_s=30) → DeliverConfigSnapshot → assert configSnapshotId → _poll_until(configSnapshotDeliveryInfo.lastStatus in ("SUCCESS","FAILURE"), timeout_s=300, description="snapshot delivery reaches a terminal lastStatus") → assert lastStatus == "SUCCESS" → _poll_until(bucket keys contain the snapshot id, timeout_s=120).

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json):
  AssociateResourceTypes / DisassociateResourceTypes: errors ConflictException | NoSuchConfigurationRecorderException | ValidationException (client, httpError 400 defaulted).
  PutExternalEvaluation: input ConfigRuleName (required, length 1..128, pattern \\S), ExternalEvaluation (required); errors InvalidParameterValueException | NoSuchConfigRuleException (client, httpError 400 defaulted); empty output.
  DeliverConfigSnapshot: input deliveryChannelName (required, length 1..256); output configSnapshotId; errors NoAvailableConfigurationRecorderException | NoRunningConfigurationRecorderException | NoSuchDeliveryChannelException (client, httpError 400 defaulted).

## Steps to Reproduce
1. git_clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite and re-replay exactly the check ids in trace_ids.
3. Cause 1: PutConfigurationRecorder with recordingGroup {allSupported: true} (ALL_SUPPORTED_RESOURCE_TYPES strategy), read its ARN from DescribeConfigurationRecorders, then call AssociateResourceTypes(ConfigurationRecorderArn=<that arn>, ResourceTypes=["AWS::EC2::Volume"]). The call is correctly refused 400 ValidationException, but Error.Message is "Your configuraiton recorder has a recording strategy that does not allow the association or disassociation of resource types." instead of "The operation failed. configuration recorder may have a recording strategy that does not permit association or disassociation of resource types." Repeat with DisassociateResourceTypes for the same refusal.
4. Cause 2: PutConfigRule for an AWS managed rule (Source Owner=AWS, any SourceIdentifier other than AWS_CONFIG_PROCESS_CHECK), then call PutExternalEvaluation(ConfigRuleName=<that rule>, ExternalEvaluation={ComplianceResourceType, ComplianceResourceId, ComplianceType: NON_COMPLIANT, Annotation, OrderingTimestamp}). Error.Code is InvalidParameterValueException as expected, but Error.Message is the generic "One or more of the specified parameters are not valid. Verify that your parameters are valid and try again." instead of "This API can only be applied to rules with sourceIdentifier as AWS_CONFIG_PROCESS_CHECK." For contrast, call it with a rule name that does not exist: that leg already returns NoSuchConfigRuleException with the correct not-found message.
5. Cause 3: create an S3 bucket with the AWSConfigBucketPermissionsCheck (s3:GetBucketAcl), AWSConfigBucketExistenceCheck (s3:ListBucket) and AWSConfigBucketDelivery (s3:PutObject with s3:x-amz-acl=bucket-owner-full-control) statements for config.amazonaws.com; PutConfigurationRecorder + PutDeliveryChannel at that bucket; StartConfigurationRecorder and wait until DescribeConfigurationRecorderStatus reports recording=true; call DeliverConfigSnapshot(deliveryChannelName=<channel>) and keep the returned configSnapshotId; then poll DescribeDeliveryChannelStatus(DeliveryChannelNames=[<channel>])[0].configSnapshotDeliveryInfo.lastStatus — it never becomes SUCCESS or FAILURE, and ListObjectsV2 on the bucket never shows a key containing the configSnapshotId.
