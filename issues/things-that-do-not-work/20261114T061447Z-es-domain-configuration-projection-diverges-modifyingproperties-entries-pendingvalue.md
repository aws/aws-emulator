Issue-Source: trace-sweep-divergence
Issue-Trace: esdef-create-domain-gp3-server-defaults
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: esdef-create-domain-gp3-server-defaults opensearch-domain-lifecycle-and-config-inspection opensearch-domain-lifecycle-workflow opensearch-domain-tagging-roundtrip opensearch-domain-version-capacity-and-software-update-management opensearch-tag-domain-lifecycle
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.AnonymousAuthDisableDate"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.IAMFederationOptions"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.JWTOptions"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.MasterUserOptions"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.SAMLOptions"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AutomatedSnapshotPauseOptions"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="TAGS"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="TAGS"].ValueType
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*]
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].PendingValue
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].ValueType
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AdvancedSecurityOptions.Status.State
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.EncryptionAtRestOptions.Status.State
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ModifyingProperties[*]
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ModifyingProperties[*].PendingValue
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ModifyingProperties[*].ValueType
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.PendingProperties[*]
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.TotalNumberOfStages
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].ValueType
Issue-Commit: 4931c0587e4227d2df501c213f0baf3c9cfd790a

# es domain configuration projection diverges: ModifyingProperties entries, PendingValue encoding, option Status.State and the stored access policy (7 causes)

> _Note: this issue was identified against the code base at commit `4931c0587e4227d2df501c213f0baf3c9cfd790a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
This bundle carries SEVEN net-new causes, all in the `es` domain-configuration projection that CreateDomain / DescribeDomain / DescribeDomainConfig / DescribeDomainChangeProgress share. A fixer must close all seven before submitting.

Shared fault: the emulator builds the `ModifyingProperties` / `PendingProperties` projection from its own internal option struct rather than from AWS's property enumeration, so the entry SET, the entry ORDER, the per-entry `PendingValue` encoding and the per-entry `ValueType` all diverge; and CreateDomain additionally rewrites the submitted access policy before storing it.

Cause 1 of 7: CreateDomain's DomainStatus.ModifyingProperties encodes the AdvancedSecurityOptions sub-properties as an EMPTY PendingValue instead of the literal string "false", and mis-types the TAGS and AutomatedSnapshotPauseOptions entries.
AWS: Smithy (com.amazonaws.opensearch / com.amazonaws.elasticsearchservice) ModifyingProperties = {Name, ActiveValue, PendingValue: String, ValueType: PropertyValueType}; PropertyValueType enum = [PLAIN_TEXT, STRINGIFIED_JSON]. The ValueType doc states PLAIN_TEXT entries "contain direct values such as \"1\", \"True\", or \"c5.large.search\"". Recorded behavior: for a domain created with fine-grained access control off, AWS emits each AdvancedSecurityOptions sub-property (AnonymousAuthDisableDate, IAMFederationOptions, JWTOptions, MasterUserOptions, SAMLOptions) with PendingValue the literal "false"; the TAGS entry as ValueType PLAIN_TEXT with PendingValue "" when no tags were supplied; and AutomatedSnapshotPauseOptions as the stringified JSON {"Enabled":false}.
Emulator: those five AdvancedSecurityOptions entries carry PendingValue "" (empty); TAGS carries PendingValue "[]" with ValueType STRINGIFIED_JSON; AutomatedSnapshotPauseOptions carries {"Enabled":false,"State":"Disabled"}.
Why: a client that reads ModifyingProperties to decide which options are in flight sees an un-set/empty marker where AWS states a concrete "false", and must parse "[]" as JSON where AWS hands it plain text — so option-state inspection and any diffing of desired vs pending config is wrong.
Fix: the CreateDomain/DescribeDomain serializer that projects the option struct into ModifyingProperties — the PendingValue stringification and ValueType selection per property.
Traces: esdef-create-domain-gp3-server-defaults#task#1.

Cause 2 of 7: DescribeDomain's DomainStatus.ModifyingProperties emits a DIFFERENT entry set in a DIFFERENT order from AWS's, and CreateDomain has rewritten the stored access policy's principal.
AWS: recorded behavior — the ModifyingProperties list is positionally stable and contains one entry per AWS-enumerated domain property; the AccessPolicies entry's PendingValue is the policy document EXACTLY as submitted, so a principal submitted as the bare account id stays the bare account id. Entries AWS emits that the emulator does not include at the same positions are ClusterConfig.ColdStorageOptions, TAGS, NodeToNodeEncryptionOptions and the AdvancedSecurityOptions sub-properties; entries the emulator emits that AWS does not are AutoTuneOptions, CognitoOptions, DeploymentStrategyOptions and EngineMode.
Emulator: the list is built in the emulator's own struct order, so entries land at the wrong indices and some AWS entries are absent while emulator-only entries are extra; and the AccessPolicies PendingValue has had {"AWS":"<account-id>"} normalized to {"AWS":"arn:aws:iam::<account-id>:root"}. The empty-PendingValue defect of Cause 1 also reproduces on this read path.
Why: the normalization silently alters a security document the caller submitted, so a client that reads the policy back, edits it and re-submits it writes a policy it never authored; and a client that indexes ModifyingProperties by position, or looks for an AWS property name, reads the wrong property or finds none.
Fix: the CreateDomain state writer (store the AccessPolicies document verbatim, no principal canonicalization) and the DescribeDomain ModifyingProperties projection (property enumeration and ordering).
Traces: opensearch-domain-lifecycle-and-config-inspection#task#2.

Cause 3 of 7: DescribeDomainConfig reports per-option Status.State as "Active" while the domain's change is still in progress, where AWS reports "Processing".
AWS: Smithy OptionState enum = [RequiresIndexDocuments, Processing, Active], documented "Processing - The requested change is still in progress" / "Active - The requested change is processed and deployed". Recorded behavior: on a freshly created domain whose ChangeProgress is not yet complete, AdvancedSecurityOptions.Status.State and EncryptionAtRestOptions.Status.State are "Processing".
Emulator: both report "Active" immediately. The ModifyingProperties entry-set/order and empty-PendingValue defects of Causes 1-2 reproduce on DomainConfig.ModifyingProperties as well.
Why: a client that polls DescribeDomainConfig and waits for Processing -> Active to know the configuration has landed sees Active on the first poll and proceeds against a domain whose configuration has not been applied, so every subsequent assertion races.
Fix: DescribeDomainConfig's option-status projection — derive each OptionStatus.State from the domain's change-progress state instead of hard-coding Active.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#4.

Cause 4 of 7: DescribeDomainChangeProgress's ChangeProgressStatus.PendingProperties is a different, differently ordered list of property strings, and TotalNumberOfStages diverges.
AWS: Smithy ChangeProgressStatusDetails.PendingProperties -> StringList, TotalNumberOfStages -> Integer (default 0). Recorded behavior: each PendingProperties element is the string "<PropertyName> : <value>", the list is the same AWS property enumeration as ModifyingProperties, and the AccessPolicies element carries the policy as submitted (bare account-id principal). AWS emits elements such as "AdvancedSecurityOptions.AnonymousAuthEnabled : false", "AdvancedSecurityOptions.InternalUserDatabaseEnabled : false", "AdvancedSecurityOptions.MasterUserOptions : false", "AdvancedSecurityOptions.SAMLOptions : false", "ClusterConfig.ColdStorageOptions : {\"Enabled\":false}".
Emulator: the list is offset by the emulator's own property order from index 4 onward (e.g. where AWS has "AdvancedSecurityOptions.AnonymousAuthEnabled : false" the emulator has "AdvancedOptions : {...}"), emits emulator-only names with empty values ("AdvancedSecurityOptions.IAMFederationOptions : ", "AdvancedSecurityOptions.JWTOptions : ", "AdvancedSecurityOptions.MasterUserOptions : "), carries the rewritten iam-root principal in the AccessPolicies element, and reports a different TotalNumberOfStages.
Why: a client that scans PendingProperties for a named property to learn whether its change is still pending cannot find AWS's property names, and a progress bar driven by TotalNumberOfStages shows the wrong denominator.
Fix: DescribeDomainChangeProgress's handler/serializer — build PendingProperties from the same AWS property enumeration as ModifyingProperties and compute TotalNumberOfStages from that change.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#5.

Cause 5 of 7: CreateDomain's ModifyingProperties places AdvancedOptions at the wrong index and omits ClusterConfig.DedicatedMasterCount, NodeToNodeEncryptionOptions and AutomatedSnapshotPauseOptions while emitting AccessPolicies and AutoTuneOptions in their place.
AWS: recorded behavior — for a default-topology domain the entry at that position is {Name: "AdvancedOptions", ActiveValue: "", PendingValue: the stringified advanced options, ValueType: STRINGIFIED_JSON}, followed later by ClusterConfig.DedicatedMasterCount (PLAIN_TEXT, empty), AutomatedSnapshotPauseOptions and NodeToNodeEncryptionOptions (both STRINGIFIED_JSON {"Enabled":false}).
Emulator: emits {Name: "AccessPolicies", PendingValue: "", ValueType: PLAIN_TEXT} and {Name: "AutoTuneOptions", PendingValue: {"DesiredState":"DISABLED","UseOffPeakWindow":false}} at those indices, shifts AdvancedOptions to the AWS AdvancedSecurityOptions position, and drops the three AWS entries above.
Why: same client consequence as Cause 2, but on the CreateDomain response itself — the very first projection a client sees after creating a domain already enumerates the wrong properties.
Fix: the CreateDomain response serializer's ModifyingProperties construction.
Traces: opensearch-domain-lifecycle-workflow#setup#1.

Cause 6 of 7: CreateDomain on the version/capacity fixture reproduces the same wrong ModifyingProperties set/order AND the AccessPolicies principal rewrite on the create response.
AWS: recorded behavior — the AccessPolicies ModifyingProperties entry's PendingValue is the submitted document with Principal {"AWS":"<account-id>"}; the entry at the ClusterConfig.ColdStorageOptions position is {"Enabled":false} STRINGIFIED_JSON, and a TAGS entry (PLAIN_TEXT, empty) is present.
Emulator: PendingValue carries Principal {"AWS":"arn:aws:iam::<account-id>:root"}; AutoTuneOptions and CognitoOptions occupy the ColdStorageOptions and TAGS positions; the AdvancedSecurityOptions sub-properties are empty instead of "false"; and the entry AWS reports as AutomatedSnapshotPauseOptions {"Enabled":false} is reported as DeploymentStrategyOptions {"DeploymentStrategy":"CapacityOptimized"}.
Why: the policy a client submitted is not the policy the service echoes, so a read-modify-write of the domain access policy corrupts it; and property lookup by index or name fails as in Cause 2.
Fix: the CreateDomain handler's access-policy storage path and its ModifyingProperties projection.
Traces: opensearch-domain-version-capacity-and-software-update-management#setup#1.

Cause 7 of 7: CreateDomain with request tags does not carry those tags into the TAGS ModifyingProperties entry — it reports an unrelated boolean there.
AWS: recorded behavior — when CreateDomain is called with tags, the TAGS entry's PendingValue is the stringified tag map built from the request's TagList (for a single tag it is a one-key JSON object), and the entry at the following position is AutomatedSnapshotPauseOptions {"Enabled":false}.
Emulator: the TAGS-position entry's PendingValue is the string "false" — the request's tags are not projected at all — and the AdvancedOptions/DedicatedMasterCount/AutomatedSnapshotPauseOptions/NodeToNodeEncryptionOptions entries are displaced exactly as in Cause 5.
Why: a client that creates a tagged domain and reads back the in-flight TAGS property to confirm its tags were accepted sees a boolean instead of its tag map, so tag-on-create cannot be verified from the create response.
Fix: the CreateDomain handler — thread the request TagList into the TAGS ModifyingProperties entry (stringified map, STRINGIFIED_JSON or PLAIN_TEXT per AWS) rather than emitting a placeholder.
Traces: opensearch-tag-domain-lifecycle#setup#1.

Related but NOT covering these: open issue 20261030T062326Z-...-opensearch-domainstatus-omits-domainendpointoptions-entirely-when-createdomain-did covers a MISSING DomainEndpointOptions member, not the ModifyingProperties projection; 20261104T094756Z-es-domain-existence-and-read-paths-answer-the owns the CreateDomain 409 / DeleteDomain 404 existence clusters, not these 200-vs-200 body divergences. None of the seven causes above is owned by an open issue.

## Evidence
All status codes matched (200 -> 200); the divergences are in the response bodies.

Cause 1 — esdef-create-domain-gp3-server-defaults#task#1, CreateDomain:
  $.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.AnonymousAuthDisableDate"].PendingValue: expected 'false', got ''
  (same for AdvancedSecurityOptions.IAMFederationOptions / .JWTOptions / .MasterUserOptions / .SAMLOptions)
  $.DomainStatus.ModifyingProperties[Name="TAGS"].PendingValue: expected '', got '[]'
  $.DomainStatus.ModifyingProperties[Name="TAGS"].ValueType: expected 'PLAIN_TEXT', got 'STRINGIFIED_JSON'
  $.DomainStatus.ModifyingProperties[Name="AutomatedSnapshotPauseOptions"].PendingValue: expected '{"Enabled":false}', got '{"Enabled":false,"State":"Disabled"}'

Cause 2 — opensearch-domain-lifecycle-and-config-inspection#task#2, DescribeDomain:
  $.DomainStatus.ModifyingProperties[3].PendingValue: expected '..."Principal":{"AWS":"000000000000"}...', got '..."Principal":{"AWS":"arn:aws:iam::000000000000:root"}...'
  $.DomainStatus.ModifyingProperties[5|7|9|10|11].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[26].PendingValue: expected '', got '{"Enabled":false}'; [26].ValueType expected 'PLAIN_TEXT', got 'STRINGIFIED_JSON'
  $.DomainStatus.ModifyingProperties[27].PendingValue: expected '{"Enabled":false}', got '{"DeploymentStrategy":"CapacityOptimized"}'
  $.DomainStatus.ModifyingProperties[12]: extra — expected Name 'ClusterConfig.ColdStorageOptions', got Name 'AutoTuneOptions'
  $.DomainStatus.ModifyingProperties[26]: extra — expected Name 'TAGS', got Name 'CognitoOptions'
  (same class at [31], [34], [35], [41])
  NOTE: the account id 000000000000 and the domain name sim2real-512d6f91 are this recording's values; the requirement is that the principal is echoed in the SAME form the request supplied, not rewritten to an iam root ARN.

Cause 3 — opensearch-domain-lifecycle-and-config-inspection#task#4, DescribeDomainConfig:
  $.DomainConfig.AdvancedSecurityOptions.Status.State: expected 'Processing', got 'Active'
  $.DomainConfig.EncryptionAtRestOptions.Status.State: expected 'Processing', got 'Active'
  plus the full ModifyingProperties set/order/PendingValue divergence listed for Cause 2, on $.DomainConfig.ModifyingProperties[3,5,7,9,10,11,12,26,27,31,34,35,41].

Cause 4 — opensearch-domain-lifecycle-and-config-inspection#task#5, DescribeDomainChangeProgress:
  $.ChangeProgressStatus.PendingProperties[3]: expected 'AccessPolicies : {..."AWS":"000000000000"...}', got 'AccessPolicies : {..."AWS":"arn:aws:iam::000000000000:root"...}'
  [4]: expected 'AdvancedSecurityOptions.AnonymousAuthEnabled : false', got 'AdvancedOptions : {"override_main_response_version":"false","rest.action.multi.allow_explicit_index":"true"}'
  [5]: expected 'AdvancedSecurityOptions.InternalUserDatabaseEnabled : false', got 'AdvancedSecurityOptions.AnonymousAuthDisableDate : '
  [6]: expected 'AdvancedSecurityOptions.MasterUserOptions : false', got 'AdvancedSecurityOptions.AnonymousAuthEnabled : false'
  [7]: expected 'AdvancedSecurityOptions.SAMLOptions : false', got 'AdvancedSecurityOptions.IAMFederationOptions : '
  [8]: expected 'DomainEndpointOptions : {...}', got 'AdvancedSecurityOptions.InternalUserDatabaseEnabled : false'
  [9]: expected 'EBSOptions : {"EBSEnabled":true,"Iops":3000,"Throughput":125,"VolumeSize":10,"VolumeType":"gp3"}', got 'AdvancedSecurityOptions.JWTOptions : '
  [10]: expected 'ClusterConfig.ColdStorageOptions : {"Enabled":false}', got 'AdvancedSecurityOptions.MasterUserOptions : '
  ... divergent through [41], plus $.ChangeProgressStatus.TotalNumberOfStages.

Cause 5 — opensearch-domain-lifecycle-workflow#setup#1, CreateDomain:
  $.DomainStatus.ModifyingProperties[4].PendingValue: expected 'false', got '{"override_main_response_version":"false","rest.action.multi.allow_explicit_index":"true"}'
  $.DomainStatus.ModifyingProperties[9|10].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[25].PendingValue: expected '', got 'false'
  $.DomainStatus.ModifyingProperties[3]: extra — expected Name 'AdvancedOptions' (STRINGIFIED_JSON), got Name 'AccessPolicies' (PLAIN_TEXT, empty)
  $.DomainStatus.ModifyingProperties[12]: extra — expected Name 'ClusterConfig.DedicatedMasterCount' (PLAIN_TEXT, empty), got Name 'AutoTuneOptions'
  $.DomainStatus.ModifyingProperties[26]: extra — expected Name 'AutomatedSnapshotPauseOptions', got Name 'CognitoOptions'
  $.DomainStatus.ModifyingProperties[31]: extra — expected Name 'NodeToNodeEncryptionOptions', got Name 'EngineMode'
  (affects opensearch-domain-tagging-roundtrip#setup#1 too)

Cause 6 — opensearch-domain-version-capacity-and-software-update-management#setup#1, CreateDomain:
  $.DomainStatus.ModifyingProperties[3].PendingValue: expected '..."AWS":"000000000000"...', got '..."AWS":"arn:aws:iam::000000000000:root"...'
  $.DomainStatus.ModifyingProperties[5|7|9|10|11].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[26].PendingValue: expected '', got '{"Enabled":false}'; ValueType expected 'PLAIN_TEXT', got 'STRINGIFIED_JSON'
  $.DomainStatus.ModifyingProperties[27].PendingValue: expected '{"Enabled":false}', got '{"DeploymentStrategy":"CapacityOptimized"}'
  $.DomainStatus.ModifyingProperties[12]: expected Name 'ClusterConfig.ColdStorageOptions', got Name 'AutoTuneOptions'
  $.DomainStatus.ModifyingProperties[26]: expected Name 'TAGS', got Name 'CognitoOptions'

Cause 7 — opensearch-tag-domain-lifecycle#setup#1, CreateDomain (request supplied one tag):
  $.DomainStatus.ModifyingProperties[25].PendingValue: expected '{"Owner":"teamA"}', got 'false'
  $.DomainStatus.ModifyingProperties[4].PendingValue: expected 'false', got '{"override_main_response_version":"false",...}'
  $.DomainStatus.ModifyingProperties[9|10].PendingValue: expected 'false', got ''
  $.DomainStatus.ModifyingProperties[3]: expected Name 'AdvancedOptions', got Name 'AccessPolicies'
  $.DomainStatus.ModifyingProperties[12]: expected Name 'ClusterConfig.DedicatedMasterCount', got Name 'AutoTuneOptions'
  $.DomainStatus.ModifyingProperties[26]: expected Name 'AutomatedSnapshotPauseOptions', got Name 'CognitoOptions'
  $.DomainStatus.ModifyingProperties[31]: expected Name 'NodeToNodeEncryptionOptions', got Name 'EngineMode'
  NOTE: Owner/teamA is this recording's tag; the requirement is that the TAGS PendingValue is the stringified map of the tags the REQUEST supplied.

Smithy cited: ModifyingProperties{Name,ActiveValue,PendingValue:String,ValueType:PropertyValueType}; PropertyValueType enum [PLAIN_TEXT, STRINGIFIED_JSON]; OptionState enum [RequiresIndexDocuments, Processing, Active] ("Processing - the requested change is still in progress"); ChangeProgressStatusDetails{PendingProperties:StringList, TotalNumberOfStages:Integer default 0}.

## Steps to Reproduce
1. Clone the repo and boot the emulator from it, then replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Cause 1: replay fixture esdef-create-domain-gp3-server-defaults, step task#1 — CreateDomain for a gp3, FGAC-off, untagged domain. Compare DomainStatus.ModifyingProperties entries named AdvancedSecurityOptions.*, TAGS and AutomatedSnapshotPauseOptions against the recorded response.
3. Cause 2: replay opensearch-domain-lifecycle-and-config-inspection, step task#2 — DescribeDomain on the fixture's domain. Compare DomainStatus.ModifyingProperties element-by-element; inspect element 3's AccessPolicies PendingValue and confirm the Principal is echoed in the submitted form rather than as an iam root ARN.
4. Cause 3: replay opensearch-domain-lifecycle-and-config-inspection, step task#4 — DescribeDomainConfig on the same domain while its change is still in progress; read DomainConfig.AdvancedSecurityOptions.Status.State and DomainConfig.EncryptionAtRestOptions.Status.State (expect 'Processing').
5. Cause 4: replay opensearch-domain-lifecycle-and-config-inspection, step task#5 — DescribeDomainChangeProgress; compare ChangeProgressStatus.PendingProperties element-by-element and TotalNumberOfStages.
6. Cause 5: replay opensearch-domain-lifecycle-workflow, step setup#1 — CreateDomain; compare DomainStatus.ModifyingProperties entry names at indices 3, 12, 26 and 31.
7. Cause 6: replay opensearch-domain-version-capacity-and-software-update-management, step setup#1 — CreateDomain with an access policy; read the AccessPolicies ModifyingProperties PendingValue and the entries at indices 12, 26 and 27.
8. Cause 7: replay opensearch-tag-domain-lifecycle, step setup#1 — CreateDomain WITH a tag; read the TAGS ModifyingProperties entry and confirm its PendingValue is the stringified request tag map.
9. Each step should pass against the recorded response.
