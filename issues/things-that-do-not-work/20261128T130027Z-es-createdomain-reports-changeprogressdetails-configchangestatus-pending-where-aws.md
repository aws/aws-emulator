Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-domain-lifecycle-and-config-inspection
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-domain-lifecycle-and-config-inspection
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus
Issue-Commit: c3e5fb924e64874a3c7124e31a5bf7fe6194984f

# es CreateDomain reports ChangeProgressDetails.ConfigChangeStatus Pending where AWS reports Initializing

> _Note: this issue was identified against the code base at commit `c3e5fb924e64874a3c7124e31a5bf7fe6194984f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One root cause on the es (OpenSearch Service) domain-create surface: the ChangeProgressDetails the CreateDomain response carries for the configuration change the create itself starts is reported in the wrong ConfigChangeStatus. It is net-new — the replay reports the cluster unowned (owned_by empty) and unblocked (stub_path empty).

Cause 1 of 1: CreateDomain answers HTTP 200 whose DomainStatus.ChangeProgressDetails.ConfigChangeStatus is 'Pending' where AWS reports 'Initializing' for the creation change.

AWS: Smithy com.amazonaws.opensearch#CreateDomain (model_path services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json) — http {POST, /2021-01-01/opensearch/domain, code 200}; input DomainName -> DomainName @required (@length 3..28, @pattern ^[a-z][a-z0-9\-]+$) plus the option blocks; output DomainStatus -> DomainStatus; modeled errors BaseException(client, 400, defaulted), DisabledOperationException(client, 409), InvalidTypeException(client, 409), LimitExceededException(client, 409), ResourceAlreadyExistsException(client, 409), ValidationException(client, 400), InternalException(server, 500). The model declares the member being reported: DomainStatus -> ChangeProgressDetails -> ConfigChangeStatus targets the enum com.amazonaws.opensearch#ConfigChangeStatus, whose complete value set is Pending | Initializing | Validating | ValidationFailed | ApplyingChanges | Completed | PendingUserInput | Cancelled, documented as 'The current status of the configuration change.' WHICH of those values the creation change is in at the instant the create answers is recorded behavior only, and the recording is unambiguous: the CreateDomain response reports DomainStatus.ChangeProgressDetails.ConfigChangeStatus = 'Initializing'. 'Initializing' is a fixed enum token — it carries no part of the request and no generated value, so the requirement is that exact member value, not a value derived from the recorded domain name, change id or timestamps (which are per-run and are not requirements). Hypothesis (neither the model nor the recording states it): AWS distinguishes a change that has been accepted but not started ('Pending') from the creation change, whose initialization of a brand-new cluster has already begun when the create answers ('Initializing'); a replay of this corpus shows a later read of a still-provisioning domain settling on 'Pending' (fixture esdef-create-domain-gp3-server-defaults#task#2 DescribeDomain passes on that member), so the create answer and a subsequent read are not required to carry the same value.

Emulator: answers HTTP 200 with DomainStatus.ChangeProgressDetails.ConfigChangeStatus = 'Pending'. The replay reports '$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus: value_mismatch (expected \'Initializing\', got \'Pending\')' with divergence_key service=es|CreateDomain|status=200->200|keys=$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus. It cannot report 'Initializing' for any domain at all: the crate's ConfigChangeStatus vocabulary has no such constant, and the create path is hard-forced to 'Pending' whenever the change is in flight and the cluster is not up yet.

Why: ConfigChangeStatus is the machine-readable statement of where a configuration change is, and clients branch on it — a provisioning wait loop that treats 'Pending' as 'the service has not picked the change up yet' and 'Initializing'/'ApplyingChanges' as 'work has started' reads the create answer as a change that has not begun, so it retries, re-submits or reports the create as not taken up when AWS has already told it initialization is under way. Because the emulator never emits 'Initializing' for any domain, a client cannot observe that transition at all, and a state machine written against AWS's progression over the enum has one of its states permanently unreachable.

Fix: the derived-member layer of the opensearch service crate's domain-configuration module — the helper that derives a domain descriptor's ChangeProgressDetails, plus the ConfigChangeStatus value set the crate publishes. The status the create path reports for the creation change must be the 'Initializing' member of ConfigChangeStatus (a constant that does not exist in the crate today and must be added beside the existing ones), not the 'Pending' member that the unconditional 'change in flight and cluster not provisioned' override substitutes; the override must stop applying to the creation change reported on the create's own answer, while the other statuses it already produces for a change on a provisioned cluster, for a completed change and for a cancelled change stay as they are, and the read paths that currently report 'Pending' for a still-provisioning domain keep doing so.

Traces: opensearch-domain-lifecycle-and-config-inspection#task#14.

Related open issue, which does NOT cover this cluster (same fixture, different step and different member — it is about DescribeDomainChangeProgress's PendingProperties entry text and about DescribeInsightDetails, and the replay reports this cluster as unowned): 20261127T052939Z-es-2-net-new-causes-describeinsightdetails-answers-200.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 492 steps total, 372 passed, 120 mismatched, 0 errored, 0 blocked, score 75.6%, 18 divergence clusters. 17 of those clusters carry a non-empty owned_by and are skipped; this is the only cluster with owned_by empty and stub_path empty.

Cluster (verdict mismatch, service es, operation CreateDomain, fixture opensearch-domain-lifecycle-and-config-inspection, step task#14, affected_count 1, affected [opensearch-domain-lifecycle-and-config-inspection#task#14]):
  expected_status 200, actual_status 200
  detail: $.DomainStatus.ChangeProgressDetails.ConfigChangeStatus: value_mismatch (expected 'Initializing', got 'Pending')
  divergence_key: service=es|CreateDomain|status=200->200|keys=$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus
  Expected (AWS, recorded): HTTP 200, DomainStatus.ChangeProgressDetails.ConfigChangeStatus = 'Initializing' (a fixed ConfigChangeStatus enum member).
  Actual (emulator): HTTP 200, DomainStatus.ChangeProgressDetails.ConfigChangeStatus = 'Pending'. No other member of the response diverges.

Smithy grounding (model_path services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json):
  CreateDomain: http {POST, /2021-01-01/opensearch/domain, code 200}; input {DomainName -> DomainName @required length 3..28 pattern ^[a-z][a-z0-9\-]+$, EngineVersion -> VersionString length 14..18, ClusterConfig, EBSOptions, ... }; output {DomainStatus -> DomainStatus}; errors BaseException(client,400,defaulted), DisabledOperationException(client,409), InternalException(server,500), InvalidTypeException(client,409), LimitExceededException(client,409), ResourceAlreadyExistsException(client,409), ValidationException(client,400).
  com.amazonaws.opensearch#ChangeProgressDetails: structure {ChangeId -> GUID length 36..36, Message -> Message length 0..1024, ConfigChangeStatus -> ConfigChangeStatus (enum) 'The current status of the configuration change.', InitiatedBy -> InitiatedBy (enum), StartTime -> UpdateTimestamp, LastUpdatedTime -> UpdateTimestamp}.
  com.amazonaws.opensearch#ConfigChangeStatus: enum values [Pending, Initializing, Validating, ValidationFailed, ApplyingChanges, Completed, PendingUserInput, Cancelled]. Both 'Pending' and 'Initializing' are modeled members, so this is the wrong member rather than an unmodeled token; the model does not say which member the create answers with, so that is taken from the recording.

Code corroboration:
  services/service-com-amazonaws-opensearch-amazonopensearchservice/src/service/domain_configuration.rs lines 617-626: reported_change_progress_details(domain) takes the latest change's own details via change.to_details() and then, at line 622-624, overrides details.config_change_status to CONFIG_CHANGE_PENDING whenever awaits_provisioning(domain, change) holds. Lines 628-637 define awaits_provisioning as change.in_progress() && !domain.cluster_is_up(), which is exactly the state a freshly created domain is in, so every CreateDomain answer is forced to 'Pending'.
  services/service-com-amazonaws-opensearch-amazonopensearchservice/src/service/domain_configuration.rs lines 41-44 define the only 'Pending' constant: CONFIG_CHANGE_PENDING = "Pending", documented as 'the change has been accepted and the service has not begun applying it'.
  services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state/config_changes.rs lines 131-144: ConfigChange::config_change_status() can only ever return CONFIG_CHANGE_APPLYING_CHANGES, CONFIG_CHANGE_CANCELLED or CONFIG_CHANGE_COMPLETED, and line 202-215 (to_details) copies that value into ChangeProgressDetails.config_change_status.
  services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs lines 962-970 publish the crate's ConfigChangeStatus constants: CONFIG_CHANGE_APPLYING_CHANGES ("ApplyingChanges"), CONFIG_CHANGE_COMPLETED ("Completed"), CONFIG_CHANGE_CANCELLED ("Cancelled"). There is no 'Initializing' constant anywhere in the crate, so the modeled member the recording requires is unreachable by construction.
  services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs (1078 lines) contains no ConfigChangeStatus handling of its own — it reports the derived projection — which is why the fix belongs in the derived-member layer above and not in the handler body.

## Steps to Reproduce
0. git clone the repo, build and boot the emulator from this checkout, and point an opensearch/es client (boto3 client('opensearch') or client('es')) at the emulator's data listener.
1. Replay fixture opensearch-domain-lifecycle-and-config-inspection through step task#14: the step is a CreateDomain — POST /2021-01-01/opensearch/domain with that step's domain name and option blocks (boto3: opensearch.create_domain(DomainName=<the step's domain name>, **<the step's options>)).
2. Read DomainStatus.ChangeProgressDetails.ConfigChangeStatus off the 200 answer.
   Observed: 'Pending'.
   Expected: 'Initializing'.
3. Confirm it is the rule and not one recording's value: create another domain under a different name and a different cluster/EBS configuration and read the same member off the create answer — the emulator reports 'Pending' every time, because the override is keyed only on 'change in flight and cluster not provisioned'. Ignore ChangeId, StartTime and LastUpdatedTime while comparing: those are per-run values, not requirements.
4. Confirm the member is reachable at all: grep the opensearch service crate for the ConfigChangeStatus value 'Initializing' — it appears nowhere, so no request can make the emulator emit the modeled member the recording requires.
5. Confirm the fix does not trade away the statuses that already match: after the change, (a) a read of a still-provisioning domain must keep reporting 'Pending' where it does today — re-replay esdef-create-domain-gp3-server-defaults#task#2 (DescribeDomain), whose ConfigChangeStatus assertion passes now and must still pass; (b) a configuration change accepted on a provisioned cluster must still report 'ApplyingChanges' while in flight, 'Completed' once it lands and 'Cancelled' when a cancel stopped it; (c) DescribeDomainChangeProgress's own ChangeProgressStatusDetails.ConfigChangeStatus must stay consistent with the descriptor's.
6. Re-replay opensearch-domain-lifecycle-and-config-inspection#task#14 and confirm the step passes before submitting.
