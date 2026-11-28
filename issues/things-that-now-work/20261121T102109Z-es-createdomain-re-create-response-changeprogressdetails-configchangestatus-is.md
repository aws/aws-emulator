Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-domain-lifecycle-and-config-inspection
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-domain-lifecycle-and-config-inspection
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus
Issue-Related: 20261114T061447Z-es-domain-configuration-projection-diverges-modifyingproperties-entries-pendingvalue.md
Issue-Commit: 41dd8320c6bf5491a3e0066243f821165de79b27

# es CreateDomain re-create response: ChangeProgressDetails.ConfigChangeStatus is Pending and the ModifyingProperties projection diverges in set, order, PendingValue and access-policy principal

> _Note: this issue was identified against the code base at commit `41dd8320c6bf5491a3e0066243f821165de79b27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
This bundle carries TWO net-new causes, both observed on the SAME replayed step: fixture opensearch-domain-lifecycle-and-config-inspection, step task#14, a CreateDomain that re-creates the fixture's domain after its earlier lifecycle steps. Both responses are HTTP 200 -> 200; the divergences are in the response body. A fixer must close both before submitting.

Cause 1 of 2: the CreateDomain response reports ChangeProgressDetails.ConfigChangeStatus as "Pending" where AWS reports "Initializing" for the change the create itself just started.
AWS: Smithy (com.amazonaws.opensearch / com.amazonaws.elasticsearchservice) DomainStatus.ChangeProgressDetails -> ChangeProgressDetails{ChangeId: GUID, Message, ConfigChangeStatus: ConfigChangeStatus, InitiatedBy, StartTime, LastUpdatedTime}; ConfigChangeStatus enum = [Pending, Initializing, Validating, ValidationFailed, ApplyingChanges, Completed, PendingUserInput, Cancelled], documented "The current status of the configuration change". Recorded behavior: the change a CreateDomain creates is already past queueing when the create answers, so the create response's ConfigChangeStatus is "Initializing"; "Pending" is the state of a change that has not been picked up yet.
Emulator: the create response carries ConfigChangeStatus "Pending".
Why: a client that polls the domain's change progress and treats Pending as "not started yet" (e.g. waits for Initializing/ApplyingChanges before it starts timing, or distinguishes a queued change from a running one) classifies the running change wrongly and can wait on, or re-submit, a change that is already in flight.
Fix: the CreateDomain handler's change-progress state (the ChangeProgressDetails the create writes into domain state and projects into DomainStatus) — seed the new change at Initializing rather than Pending.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#14.

Cause 2 of 2: on the same create response, DomainStatus.ModifyingProperties is built from the emulator's internal option struct instead of AWS's property enumeration, so the entry SET and ORDER diverge, five AdvancedSecurityOptions sub-properties carry an empty PendingValue instead of the literal "false", the TAGS entry is mis-typed, and the AccessPolicies entry's PendingValue has had the submitted principal rewritten.
AWS: Smithy ModifyingProperties{Name: String, ActiveValue: String, PendingValue: String, ValueType: PropertyValueType}; PropertyValueType enum = [PLAIN_TEXT, STRINGIFIED_JSON], with PLAIN_TEXT documented as "direct values such as \"1\", \"True\", or \"c5.large.search\"" and STRINGIFIED_JSON as "content in JSON format, such as {\"Enabled\":\"True\"}". Recorded behavior: the list is one positionally-stable entry per AWS-enumerated domain property; each AdvancedSecurityOptions sub-property of a domain created with fine-grained access control off has PendingValue the literal string "false"; the TAGS entry is ValueType PLAIN_TEXT with an empty PendingValue when the request supplied no tags; ClusterConfig.ColdStorageOptions and AutomatedSnapshotPauseOptions are present as STRINGIFIED_JSON {"Enabled":false}; and the AccessPolicies entry's PendingValue is the policy document EXACTLY as submitted, so a principal submitted as the bare account id stays the bare account id.
Emulator: the AdvancedSecurityOptions sub-property entries carry PendingValue "" (empty); the entry at the AWS TAGS position is CognitoOptions with PendingValue {"Enabled":false} and ValueType STRINGIFIED_JSON instead of TAGS/PLAIN_TEXT/""; the entry at the AWS ClusterConfig.ColdStorageOptions position is AutoTuneOptions; the entry AWS reports as AutomatedSnapshotPauseOptions {"Enabled":false} is reported as DeploymentStrategyOptions {"DeploymentStrategy":"CapacityOptimized"}; further entries diverge the same way at the tail of the list; and the AccessPolicies PendingValue has the submitted Principal {"AWS":"<account-id>"} normalized to {"AWS":"arn:aws:iam::<account-id>:root"}.
Why: the normalization silently alters a security document the caller submitted, so a client that reads the policy back, edits it and re-submits it writes a policy it never authored; and a client that indexes ModifyingProperties by position, or looks up an AWS property by name to learn whether that option is in flight, reads the wrong property or finds none and sees an empty marker where AWS states a concrete "false".
Fix: the CreateDomain handler's access-policy storage path (store the submitted document verbatim, no principal canonicalization) and the CreateDomain response serializer's ModifyingProperties projection (property enumeration, ordering, PendingValue stringification and ValueType selection).
Traces: opensearch-domain-lifecycle-and-config-inspection#task#14.

Related but NOT covering this step: open issue 20261114T061447Z-es-domain-configuration-projection-diverges-modifyingproperties-entries-pendingvalue.md describes the same class of ModifyingProperties fault on OTHER fixture steps (esdef-create-domain-gp3-server-defaults#task#1, opensearch-domain-lifecycle-and-config-inspection#task#2/#task#4/#task#5, opensearch-domain-lifecycle-workflow#setup#1, opensearch-domain-version-capacity-and-software-update-management#setup#1, opensearch-tag-domain-lifecycle#setup#1) and carries no ChangeProgressDetails.ConfigChangeStatus key at all; the replay reports this cluster as owned by no open issue. Issue 20261104T094756Z-es-domain-existence-and-read-paths-answer-the.md owns the CreateDomain 409 / DeleteDomain 404 existence clusters, not this 200-vs-200 body divergence.

## Evidence
Replayed step: fixture opensearch-domain-lifecycle-and-config-inspection, step task#14, service es, operation CreateDomain. Status matched (expected 200, got 200); verdict mismatch on the body.

Cause 1 — ChangeProgressDetails:
  $.DomainStatus.ChangeProgressDetails.ConfigChangeStatus: value_mismatch (expected 'Initializing', got 'Pending')
Both are members of the modeled ConfigChangeStatus enum; the requirement is that a change a CreateDomain just started is reported as Initializing, not as the not-yet-picked-up Pending.

Cause 2 — ModifyingProperties:
  $.DomainStatus.ModifyingProperties[3].PendingValue: expected '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"000000000000"},"Action":"es:*","Resource":"arn:aws:es:us-west-2:000000000000:domain/sim2real-0a1ada7a/*"}]}', got the same document with "Principal":{"AWS":"arn:aws:iam::000000000000:root"}
  $.DomainStatus.ModifyingProperties[5].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[7].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[9].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[10].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[11].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[26].PendingValue: expected '', got '{"Enabled":false}'
  $.DomainStatus.ModifyingProperties[26].ValueType: expected 'PLAIN_TEXT', got 'STRINGIFIED_JSON'
  $.DomainStatus.ModifyingProperties[27].PendingValue: expected '{"Enabled":false}', got '{"DeploymentStrategy":"CapacityOptimized"}'
  $.DomainStatus.ModifyingProperties[12]: extra — expected {Name 'ClusterConfig.ColdStorageOptions', ActiveValue '', PendingValue '{"Enabled":false}', ValueType 'STRINGIFIED_JSON'}, got {Name 'AutoTuneOptions', ActiveValue '', PendingValue '{"DesiredState":"DISABLED","UseOffPeakWindow":false}', ValueType 'STRINGIFIED_JSON'}
  $.DomainStatus.ModifyingProperties[26]: extra — expected {Name 'TAGS', ActiveValue '', PendingValue '', ValueType 'PLAIN_TEXT'}, got {Name 'CognitoOptions', ActiveValue '', PendingValue '{"Enabled":false}', ...}
  same class of entry-set/order divergence reported at $.DomainStatus.ModifyingProperties[31], [34], [35] and [41].

NOTE on per-run values: the account id 000000000000 and the domain name sim2real-0a1ada7a are this recording's values and are NOT the requirement. The requirements are: the AccessPolicies PendingValue echoes the principal in the SAME form the request supplied (bare account id stays a bare account id); each FGAC-off AdvancedSecurityOptions sub-property carries the literal "false"; the TAGS entry is PLAIN_TEXT with an empty PendingValue when the request supplied no tags; and the entry names/positions follow AWS's property enumeration.

Smithy cited: com.amazonaws.opensearch#ChangeProgressDetails{ChangeId: GUID, Message, ConfigChangeStatus, InitiatedBy, StartTime, LastUpdatedTime}; ConfigChangeStatus enum [Pending, Initializing, Validating, ValidationFailed, ApplyingChanges, Completed, PendingUserInput, Cancelled]; ModifyingProperties{Name, ActiveValue, PendingValue: String, ValueType: PropertyValueType}; PropertyValueType enum [PLAIN_TEXT, STRINGIFIED_JSON].

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it, then replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Drive fixture opensearch-domain-lifecycle-and-config-inspection through to step task#14 — the CreateDomain that re-creates the fixture's domain after the earlier lifecycle/config-inspection steps, with an access policy whose Principal is the bare account id, fine-grained access control off and no request tags.
3. Cause 1: read $.DomainStatus.ChangeProgressDetails.ConfigChangeStatus on that create response — it must be 'Initializing', not 'Pending'.
4. Cause 2: compare $.DomainStatus.ModifyingProperties element-by-element against the recorded response: the AccessPolicies entry's PendingValue must echo the submitted principal unrewritten; the AdvancedSecurityOptions sub-property entries must carry PendingValue 'false'; the TAGS entry must be present at its AWS position as PLAIN_TEXT with an empty PendingValue; and ClusterConfig.ColdStorageOptions / AutomatedSnapshotPauseOptions must be present instead of AutoTuneOptions / CognitoOptions / DeploymentStrategyOptions at those indices.
5. The step should pass against the recorded response.

## Disposition

Cause 2 is served by this code base and is now pinned on the axis this report is about (a create
under a name the service has held before); cause 1 is DECLINED as a per-run value.

* Cause 2 — the `ModifyingProperties` projection of the re-create matches the recorded entry set,
  order, per-entry `PendingValue`/`ValueType` and the submitted access-policy principal: the
  projection is built from the service's property enumeration and the document is stored verbatim
  (`src/service/domain_configuration.rs`), and a replay of the bound step reports no
  `ModifyingProperties` key for it. What was missing was a test on the RE-CREATE axis — every
  existing suite measures the projection on a name the service never held — so
  `tests/all/re_created_domain_change_projection.rs` now holds the projection of a create under a
  re-used name to a first create's, property for property, over a table of name histories and two
  create bodies.
* Cause 1 — `$.DomainStatus.ChangeProgressDetails.ConfigChangeStatus`, this report's only replay
  key — DECLINED. Which state of its own apply workflow the service has reached when it serialises
  a create response is a per-run race rather than a rule, and the corpus records both sides of it
  for the IDENTICAL key: `Pending` on five creates (see
  `20261029T190009Z-es-domainstatus-and-domainconfig-responses-omit-modeled-option.md`, cause 1 and
  its evidence line "expected 'Pending', got 'ApplyingChanges'"), `Completed` on another re-create
  (`20261104T094756Z-es-domain-existence-and-read-paths-answer-the.md`, cause 1) and `Initializing`
  on this one step. The emulator reports a modelled `ConfigChangeStatus` member for the state the
  change is really in, the same value on every path that reports that change, advancing it as the
  clock makes the change due; answering `Initializing` on every create would fit this one recording
  and break the five that record `Pending`. The invariants the value does owe — a modelled member,
  one status per change per instant, moving only forward, `Completed` once the change has landed,
  and independent of whether the name was held before — are pinned by the new suite and by
  `tests/all/change_progress/change_status_reported_consistently.rs`.
