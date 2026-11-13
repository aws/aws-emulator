Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to
Issue-Key: config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to|_await_history_items#13|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# config__iam: the configuration recorder produces no configuration item for an IAM managed policy, so a global-resource-type recording never reports attachmentCount or the "Is attached to Role" relationship (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
ONE root cause. ONE CAUSE must be closed before submitting.

The check's service token is the composite suite token `config__iam`; both underlying services (configservice, iam) are bootstrapped and the defect belongs to configservice's recording pipeline for GLOBAL resource types.

Cause 1 of 1: a running customer-managed configuration recorder whose recordingGroup has allSupported=true AND includeGlobalResourceTypes=true never produces a configuration item for an AWS::IAM::Policy, so neither the policy's configuration (attachmentCount) nor its relationship to the roles it is attached to is ever observable.

AWS: recorded behavior only, with model grounding. Smithy (configservice): RecordingGroup.includeGlobalResourceTypes governs whether Config records global resource types such as IAM users, groups, roles and customer managed policies; GetResourceConfigHistory{resourceType, resourceId required} -> {configurationItems, nextToken}, paginated, with ResourceNotDiscoveredException (errorKind client, httpError 400, documented "You have specified a resource that is either unknown or has not been discovered") for a key Config has recorded nothing for. ConfigurationItem carries configurationItemCaptureTime, configurationItemStatus (ConfigurationItemStatus = OK | ResourceDiscovered | ResourceNotRecorded | ResourceDeleted | ResourceDeletedNotRecorded), arn, resourceType, resourceId, resourceName, awsRegion, the JSON-encoded `configuration` string and a `relationships` list whose entries carry resourceType, resourceId, resourceName and relationshipName. Recorded flow: with such a recorder running, creating a customer managed policy yields a configuration item for resourceType AWS::IAM::Policy whose resourceId is the policy's IAM id (the ANPA… policy id), whose configuration JSON reports attachmentCount, and — after iam:AttachRolePolicy — a LATER configuration item reporting attachmentCount 1 and a relationships entry with relationshipName "Is attached to Role" naming the role. The policy name, policy id and role name are values from the request and the emulator's state; the fixed requirements are that the item exists, that attachmentCount tracks the attachments, and the relationship wording "Is attached to Role".

Emulator: no configuration item for the managed policy is ever produced. The check fails with TimeoutError: "a configuration item for managed policy <policy name> (<policy id>) reporting attachmentCount 1: the run budget ended first; not graded" (check line 619, helper _await_history_items at line 471), after a _SECOND_CI_BUDGET_S of 600 s inside the check. The recorder is created with _GLOBAL_RECORDING_GROUP = {allSupported: true, includeGlobalResourceTypes: true}, is started, and reports recording; nothing is refused — GetResourceConfigHistory for the policy simply stays empty / not-discovered forever. The run never reaches the relationship assertion, so this issue claims nothing about the relationships projection beyond its absence.

Related open issue, named for overlap but NOT relied on as cover (the suite did not mark this check OWNED by it and it carries none of this check's replay keys; it is scoped to AWS::SQS::Queue under a recorder with includeGlobalResourceTypes=false): 20261025T144945Z-config-the-configuration-recorder-never-records-resources-from.md. This cause is the GLOBAL-resource-type leg: recording an IAM resource additionally requires honouring includeGlobalResourceTypes and projecting the IAM-specific configuration/relationships.

Why: a client cannot use Config for IAM inventory or IAM compliance at all. Reads against an IAM managed policy are indistinguishable from "the account has no such policy" while the recorder reports recording=true, so a dashboard that enumerates over-attached or unattached managed policies, or that follows the "Is attached to Role" relationship to map privilege, sees nothing forever with no error to distinguish a stub from an empty account. Any Config rule scoped to AWS::IAM::Policy is also permanently un-evaluable.

Fix: the configuration-recorder recording path of the Config service — while a customer-managed recorder is recording with includeGlobalResourceTypes=true, global IAM resources (AWS::IAM::Policy and siblings) created/changed in the IAM service must be captured as configuration items into the recorder's inventory (the `discovered_resources` state behind services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs, today reachable only from src/ops/put_resource_config.rs), with configurationItemStatus ResourceDiscovered on first capture and OK on later states, a `configuration` JSON carrying attachmentCount, and a `relationships` entry relationshipName "Is attached to Role" per attached role, from which src/ops/get_resource_config_history.rs already projects.

Traces: config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to (check line 619, helper _await_history_items at line 471).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 70/113 checks passed, 29 mismatched / 14 errored). config__iam is 0/1 (0%). The check is marked neither ALREADY FILED nor OWNED.

[error] config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to
  File "check_config__iam_config_managed_policy_item_relates_to_the_roles_it_is_attached_to.py", line 619, in run
    items = _await_history_items(
  File "...", line 471, in _await_history_items
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: a configuration item for managed policy 'sim2real-behav-cfg-policy-5b4f52b818e6' ('ANPABGXEQES5NRCCJJWAN') reporting attachmentCount 1: the run budget ended first; not graded

Check setup (from the replayed source): _GLOBAL_RECORDING_GROUP = {"allSupported": True, "includeGlobalResourceTypes": True}; an IAM role 'sim2real-behav-config-role' trusting config.amazonaws.com with AWS_ConfigRole attached; a delivery bucket with the three AWSConfigBucket* statements; PutConfigurationRecorder + PutDeliveryChannel + StartConfigurationRecorder; iam:CreatePolicy of a customer managed policy; iam:AttachRolePolicy attaching it to a role; then _await_history_items polling GetResourceConfigHistory(resourceType='AWS::IAM::Policy', resourceId=<policy id>) with _CI_BUDGET_S = 600 s / _SECOND_CI_BUDGET_S = 600 s, treating ResourceNotDiscoveredException as an empty history, and asserting the item's configuration reports attachmentCount 1 and relationships carries relationshipName _ATTACHED_TO_ROLE = "Is attached to Role".

Expected (AWS, recorded): a configuration item for the policy appears within minutes; after the attach, a later item reports attachmentCount 1 and a relationships entry "Is attached to Role" naming the role. Actual (emulator): the history stays empty for the whole 600 s budget; nothing is refused and the recorder reports recording. The policy name 'sim2real-behav-cfg-policy-5b4f52b818e6' and the policy id 'ANPABGXEQES5NRCCJJWAN' in the message are per-run values; the requirement is that the item exists and that attachmentCount tracks the attachments.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): GetResourceConfigHistory{resourceType, resourceId required} -> {configurationItems, nextToken}, paginated, errors include ResourceNotDiscoveredException (client, 400); RecordingGroup{allSupported, includeGlobalResourceTypes, resourceTypes, exclusionByResourceTypes, recordingStrategy}; ConfigurationItem{configurationItemCaptureTime, configurationItemStatus, arn, resourceType, resourceId, resourceName, awsRegion, configuration, relationships, supplementaryConfiguration}.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-ga-prod-v1 and re-replay config__iam.config-managed-policy-item-relates-to-the-roles-it-is-attached-to.
3. Manually: iam create-role --role-name sim2real-behav-config-role --assume-role-policy-document trusting config.amazonaws.com; iam attach-role-policy --policy-arn arn:aws:iam::aws:policy/service-role/AWS_ConfigRole; s3 create-bucket for delivery and put-bucket-policy with the AWSConfigBucketPermissionsCheck / AWSConfigBucketExistenceCheck / AWSConfigBucketDelivery statements.
4. config put-configuration-recorder --configuration-recorder name=<rec>,roleARN=<role arn>,recordingGroup={allSupported=true,includeGlobalResourceTypes=true}; config put-delivery-channel --delivery-channel name=<chan>,s3BucketName=<bucket>; config start-configuration-recorder --configuration-recorder-name <rec> (describe-configuration-recorder-status reports recording=true).
5. iam create-policy --policy-name <p> --policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}' and note the returned PolicyId (ANPA…).
6. Poll config get-resource-config-history --resource-type AWS::IAM::Policy --resource-id <PolicyId>: on AWS a configuration item appears within minutes; the emulator answers ResourceNotDiscoveredException / an empty configurationItems list indefinitely.
7. iam attach-role-policy --role-name <any role> --policy-arn <the new policy>, then poll the same history again: no later item reporting attachmentCount 1 and no relationships entry with relationshipName "Is attached to Role" is ever produced.
