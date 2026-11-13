Issue-Source: trace-sweep-divergence
Issue-Trace: esdef-create-domain-gp3-server-defaults
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: esdef-create-domain-gp3-server-defaults esdef-create-domain-gp2-server-defaults opensearch-domain-lifecycle-and-config-inspection opensearch-domain-lifecycle-workflow opensearch-domain-tagging-roundtrip opensearch-domain-version-capacity-and-software-update-management opensearch-tag-domain-lifecycle
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.AnonymousAuthDisableDate"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.AnonymousAuthEnabled"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.IAMFederationOptions"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.InternalUserDatabaseEnabled"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.JWTOptions"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.MasterUserOptions"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AdvancedSecurityOptions.SAMLOptions"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="AutomatedSnapshotPauseOptions"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.ColdStorageOptions"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.DedicatedMasterCount"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.DedicatedMasterEnabled"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.DedicatedMasterType"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.InstanceCount"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.InstanceType"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.MultiAZWithStandbyEnabled"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.WarmCount"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.WarmEnabled"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.WarmStorage"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.WarmType"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="ClusterConfig.ZoneAwarenessEnabled"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="OffPeakWindowOptions"].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[Name="TAGS"]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.OffPeakWindowOptions.OffPeakWindow.WindowStartTime.Hours
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.Endpoint
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.OffPeakWindowOptions.OffPeakWindow.WindowStartTime.Hours
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.CurrentVersion
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*]
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].PendingValue
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].ValueType
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AutoTuneOptions.Options.MaintenanceSchedules
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.AutoTuneOptions.Options.RollbackOnDisable
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.CognitoOptions.Status.State
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.IdentityCenterOptions.Status.State
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ModifyingProperties[*]
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ModifyingProperties[*].PendingValue
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ModifyingProperties[*].ValueType
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.OffPeakWindowOptions.Options.OffPeakWindow.WindowStartTime.Hours
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.VPCOptions.Status.State
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.PendingProperties[*]
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.TotalNumberOfStages
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.Endpoint
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.OffPeakWindowOptions.OffPeakWindow.WindowStartTime.Hours
Issue-Key: es|DeleteDomain|2xx->2xx|$.DomainStatus.ServiceSoftwareOptions.CurrentVersion
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].PendingValue
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].ValueType
Issue-Commit: 8f194cc1fc794f910c53fc884e414c1833919c6c

# es: DomainStatus/DomainConfig projection diverges — off-peak default, ModifyingProperties, AccessPolicies, endpoint, software version (7 causes)

> _Note: this issue was identified against the code base at commit `8f194cc1fc794f910c53fc884e414c1833919c6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
The emulator's OpenSearch/Elasticsearch (`es` endpoint prefix) domain projection — the DomainStatus it returns from CreateDomain / DescribeDomain / DescribeDomains / DeleteDomain, the DomainConfig it returns from DescribeDomainConfig, and the ChangeProgressStatus it returns from DescribeDomainChangeProgress — diverges from AWS in seven independent ways. All seven are read-path/representation defects on a domain the emulator did create; the HTTP status is 200 in every case. This bundle carries SEVEN causes and all seven must be closed before it is submitted.

Cause 1 of 7: off-peak maintenance window default start hour.
AWS: when CreateDomain omits OffPeakWindowOptions, OpenSearch Service enables the off-peak window and assigns a service-chosen UTC start time; every recording in this corpus shows DomainStatus.OffPeakWindowOptions.OffPeakWindow.WindowStartTime = {Hours: 5, Minutes: 0} (a fixed 10-hour window, per the Smithy doc on com.amazonaws.opensearch#OffPeakWindow: "The window length will always be 10 hours"). Smithy: WindowStartTime.Hours is @required, @range min 0 max 23; its modeled zero default is the shape default, not the service-assigned window. Recorded behavior shows the service default start hour is 5, not 0.
Emulator: reports WindowStartTime.Hours = 0 on every created domain, in CreateDomain, DescribeDomain, DescribeDomainConfig and DeleteDomain responses alike.
Why: a client that reads back the off-peak window to decide when service-software updates and blue/green Auto-Tune changes may run is told the window opens at 00:00 UTC rather than the window the service actually assigned, so maintenance scheduling and any assertion on the defaulted window are wrong.
Fix: the default applied when OffPeakWindowOptions is absent, in the CreateDomain handler's domain-defaulting path (the stored domain record, so every read agrees).
Traces: esdef-create-domain-gp2-server-defaults#task#2, esdef-create-domain-gp3-server-defaults#task#1, esdef-create-domain-gp3-server-defaults#task#2, opensearch-domain-lifecycle-and-config-inspection#task#2.

Cause 2 of 7: ModifyingProperties / PendingProperties are reported per top-level option instead of per leaf property, with the wrong ValueType.
AWS: ModifyingProperties entries name LEAF properties with dotted paths — e.g. "AdvancedSecurityOptions.AnonymousAuthEnabled", "AdvancedSecurityOptions.MasterUserOptions", "ClusterConfig.InstanceType", "ClusterConfig.ColdStorageOptions", "ClusterConfig.DedicatedMasterEnabled", "TAGS" — and classify each by Smithy's com.amazonaws.opensearch#PropertyValueType: PLAIN_TEXT for "direct values such as \"1\", \"True\"" (so the scalar leaves carry PendingValue 'false'/'1'/'t3.small.search' with ValueType PLAIN_TEXT) and STRINGIFIED_JSON for "content in JSON format" (so ClusterConfig.ColdStorageOptions carries '{"Enabled":false}' and AccessPolicies carries the policy document, both STRINGIFIED_JSON). DescribeDomainChangeProgress.ChangeProgressStatus.PendingProperties carries the same leaf names rendered as "<Name> : <value>" strings, and TotalNumberOfStages is reported alongside them.
Emulator: emits ONE entry per top-level option — Name 'AdvancedSecurityOptions', 'AutoTuneOptions', 'AutomatedSnapshotPauseOptions', 'ClusterConfig', 'AdvancedOptions' — whose PendingValue is the whole option serialized as JSON and whose ValueType is always STRINGIFIED_JSON, and inverts the classification for AccessPolicies (ValueType 'PLAIN_TEXT' for a JSON policy document, where AWS says STRINGIFIED_JSON). The leaf entries AWS lists are therefore absent at every index, which shifts the whole list and makes every positional read diverge; PendingProperties has the same coarse shape and TotalNumberOfStages does not match.
Why: a client that inspects which individual settings are in flight (or diffs ModifyingProperties/PendingProperties between polls to drive a wait) can never find a leaf name such as ClusterConfig.InstanceType, and a client that parses PendingValue by ValueType mis-parses a JSON policy as plain text.
Fix: the ModifyingProperties/PendingProperties builder in the domain-config projection (shared by CreateDomain, DescribeDomain, DescribeDomainConfig and DescribeDomainChangeProgress) — expand each option to its leaf properties and set ValueType per leaf.
Traces: esdef-create-domain-gp3-server-defaults#task#1, opensearch-domain-lifecycle-and-config-inspection#task#2, opensearch-domain-lifecycle-and-config-inspection#task#4, opensearch-domain-lifecycle-and-config-inspection#task#5, opensearch-domain-lifecycle-workflow#setup#1, opensearch-domain-version-capacity-and-software-update-management#setup#1.

Cause 3 of 7: the submitted AccessPolicies document is rewritten before it is stored/echoed.
AWS: AccessPolicies is an opaque policy document (Smithy: target com.amazonaws.opensearch#PolicyDocument, @length min 0 max 102400, @pattern ".*"); the service echoes the document as submitted. The recordings show the principal exactly as the request wrote it (Principal.AWS = the bare 12-digit account id).
Emulator: canonicalises the principal when it stores the policy, so every read-back (DomainStatus.AccessPolicies, the AccessPolicies ModifyingProperties entry, and the PendingProperties entry) shows Principal.AWS rewritten from the bare account id to the account-root IAM ARN 'arn:aws:iam::<account>:root' form.
Why: a client that round-trips its policy (read, edit, re-put) or compares the stored document byte-for-byte with what it submitted — a common drift check in IaC — sees a document it never wrote and reports permanent drift.
Fix: store and echo AccessPolicies verbatim in the CreateDomain/UpdateDomainConfig handlers; do not normalise principals in the state write.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#2, opensearch-domain-lifecycle-and-config-inspection#task#4, opensearch-domain-lifecycle-and-config-inspection#task#5, opensearch-domain-version-capacity-and-software-update-management#setup#1.

Cause 4 of 7: DomainStatus.Endpoint is not a domain endpoint hostname, and it survives deletion.
AWS: DomainStatus.Endpoint is a bare DNS hostname for the domain's search endpoint — the 'search-<domain-name>-<opaque-suffix>.<region>.es.amazonaws.com' form, with no scheme, no port and no path. In a DeleteDomain response for a domain being deleted the recordings carry no Endpoint at all (null).
Emulator: returns a host:port plus an HTTP path — of the form '<host>:<port>/_aws/es/<domain-name>' — in DescribeDomain, and still returns that value in the DeleteDomain response where AWS reports none. (The concrete host and port are per-environment and are NOT the requirement; the requirement is that the member is a hostname in the documented form, and that it is absent once the domain is deleted.) Related, differently-scoped open issue: 20261101T140814Z-cloudformation-iam-opensearch-sts-aws-opensearchservice-domain-s.md.
Why: a client that builds a URL as 'https://' + Endpoint + '/_cluster/health', or that parses the hostname to derive the region/domain, produces a malformed URL; a client that keys "domain is gone" off the absence of Endpoint in the DeleteDomain response never observes deletion.
Fix: the Endpoint projection for the stored domain, and the DeleteDomain handler's response projection (omit Endpoint for a domain entering deletion).
Traces: esdef-create-domain-gp3-server-defaults#task#2, opensearch-domain-lifecycle-and-config-inspection#teardown#17.

Cause 5 of 7: ServiceSoftwareOptions.CurrentVersion is a hard-coded constant.
AWS: Smithy documents ServiceSoftwareOptions.CurrentVersion as "The current service software version present on the domain"; the recordings show an engine-derived service-software release label for the domain's engine version (an 'OpenSearch_<major>_<minor>_R<release>' shaped string for an OpenSearch domain), and an EMPTY string in the DeleteDomain response for a domain entering deletion.
Emulator: returns the fixed string 'R20250101' for every domain regardless of EngineVersion, including in the DeleteDomain response where AWS reports ''.
Why: a client that gates a service-software update on the current release label, or that compares CurrentVersion with NewVersion to decide whether an update is available, reads a label that belongs to no engine version and never changes with the domain's engine.
Fix: derive CurrentVersion from the domain's engine version in the ServiceSoftwareOptions projection; clear it in the DeleteDomain response.
Traces: esdef-create-domain-gp3-server-defaults#task#2, opensearch-domain-lifecycle-and-config-inspection#teardown#17.

Cause 6 of 7: DescribeDomainConfig reports unchanged options as Processing and omits defaulted AutoTuneOptions members.
AWS: each DomainConfig member is an OptionStatus wrapper whose Status.State is 'Active' for an option that is not being changed; the recordings show CognitoOptions.Status.State, IdentityCenterOptions.Status.State and VPCOptions.Status.State = 'Active' on a settled domain. DomainConfig.AutoTuneOptions.Options also carries the defaulted MaintenanceSchedules (empty list) and RollbackOnDisable = 'NO_ROLLBACK'.
Emulator: reports Status.State = 'Processing' for those untouched options, and omits AutoTuneOptions.Options.MaintenanceSchedules and AutoTuneOptions.Options.RollbackOnDisable entirely (null).
Why: a client that polls DescribeDomainConfig and waits for every option State to reach 'Active' before continuing waits forever on options it never changed, and a client reading the Auto-Tune rollback/maintenance defaults gets null instead of the service defaults.
Fix: the OptionStatus State computation and the AutoTuneOptions defaulting in the DescribeDomainConfig projection.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#4.

Cause 7 of 7: DescribeDomainChangeProgress reports the wrong TotalNumberOfStages for the change it is describing.
AWS: ChangeProgressStatus.TotalNumberOfStages is the number of stages the described configuration change has; the recording's value accompanies the leaf-level PendingProperties list for that change.
Emulator: returns a TotalNumberOfStages that does not match the described change (it is derived from the emulator's own coarse pending-property list, see Cause 2).
Why: a client that renders progress as completed-stages/TotalNumberOfStages, or that waits until the stage count is exhausted, computes progress against a denominator the service never reported.
Fix: compute TotalNumberOfStages from the change's stage list in the DescribeDomainChangeProgress handler.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#5.

## Evidence
Replayed against the emulator booted from this checkout; service `es`, all steps returned HTTP 200 as expected, bodies diverge.

Cause 1 — esdef-create-domain-gp2-server-defaults#task#2 (CreateDomain): $.DomainStatus.OffPeakWindowOptions.OffPeakWindow.WindowStartTime.Hours: expected 5, got 0. Same key diverges in esdef-create-domain-gp3-server-defaults#task#1/#task#2, opensearch-domain-lifecycle-and-config-inspection#task#2/#teardown#17, opensearch-domain-lifecycle-workflow#setup#1, opensearch-domain-version-capacity-and-software-update-management#setup#1.

Cause 2 — esdef-create-domain-gp3-server-defaults#task#1 (CreateDomain): expected entries such as {'Name': 'AdvancedSecurityOptions.AnonymousAuthEnabled', 'ActiveValue': '', 'PendingValue': 'false', 'ValueType': 'PLAIN_TEXT'} and {'Name': 'ClusterConfig.ColdStorageOptions', 'PendingValue': '{"Enabled":false}', 'ValueType': 'STRINGIFIED_JSON'} are missing; got instead {'Name': 'AdvancedSecurityOptions', 'PendingValue': '{"AnonymousAuthEnabled":false,"Enabled":false,"InternalUserDatabaseEnabled":false}', 'ValueType': 'STRINGIFIED_JSON'} and {'Name': 'ClusterConfig', 'PendingValue': '{"ColdStorageOptions":{"Enabled":false},...}', 'ValueType': 'STRINGIFIED_JSON'}. opensearch-domain-lifecycle-and-config-inspection#task#2 adds $.DomainStatus.ModifyingProperties[3].ValueType: expected 'STRINGIFIED_JSON', got 'PLAIN_TEXT' (the AccessPolicies entry). opensearch-domain-lifecycle-and-config-inspection#task#5 (DescribeDomainChangeProgress): $.ChangeProgressStatus.PendingProperties[4]: expected 'AdvancedSecurityOptions.AnonymousAuthEnabled : false', got 'AdvancedOptions : {"override_main_response_version":"false",...}'.

Cause 3 — opensearch-domain-lifecycle-and-config-inspection#task#2 (DescribeDomain): $.DomainStatus.ModifyingProperties[3].PendingValue: expected a policy whose Principal is {"AWS":"<account-id>"}, got the same policy with {"AWS":"arn:aws:iam::<account-id>:root"}. Same rewrite in #task#4 (DescribeDomainConfig), #task#5 (PendingProperties[3]) and opensearch-domain-version-capacity-and-software-update-management#setup#1.

Cause 4 — esdef-create-domain-gp3-server-defaults#task#2 (DescribeDomain): $.DomainStatus.Endpoint: expected a 'search-<domain>-<suffix>.<region>.es.amazonaws.com' hostname, got '<host>:<port>/_aws/es/<domain>'. opensearch-domain-lifecycle-and-config-inspection#teardown#17 (DeleteDomain): $.DomainStatus.Endpoint: type_mismatch, expected None, got '<host>:<port>/_aws/es/<domain>'.

Cause 5 — esdef-create-domain-gp3-server-defaults#task#2: $.DomainStatus.ServiceSoftwareOptions.CurrentVersion: expected an engine-derived 'OpenSearch_2_19_R<release>' label, got 'R20250101'. opensearch-domain-lifecycle-and-config-inspection#teardown#17: expected '', got 'R20250101'.

Cause 6 — opensearch-domain-lifecycle-and-config-inspection#task#4 (DescribeDomainConfig): $.DomainConfig.CognitoOptions.Status.State: expected 'Active', got 'Processing'; $.DomainConfig.IdentityCenterOptions.Status.State and $.DomainConfig.VPCOptions.Status.State likewise; $.DomainConfig.AutoTuneOptions.Options.MaintenanceSchedules: expected [], got None; $.DomainConfig.AutoTuneOptions.Options.RollbackOnDisable: expected 'NO_ROLLBACK', got None.

Cause 7 — opensearch-domain-lifecycle-and-config-inspection#task#5 (DescribeDomainChangeProgress): $.ChangeProgressStatus.TotalNumberOfStages diverges together with the PendingProperties list.

Model references: services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/com-amazonaws-opensearch-amazonopensearchservice.json — CreateDomain (POST /2021-01-01/opensearch/domain), DeleteDomain (DELETE /2021-01-01/opensearch/domain/{DomainName}), DescribeDomainChangeProgress (GET /2021-01-01/opensearch/domain/{DomainName}/progress); shapes OffPeakWindow, WindowStartTime, ModifyingProperties, PropertyValueType, ServiceSoftwareOptions, PolicyDocument.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay esdef-create-domain-gp2-server-defaults#task#2 — CreateDomain with no OffPeakWindowOptions, then read DomainStatus.OffPeakWindowOptions.OffPeakWindow.WindowStartTime.Hours (0, expected the service default 5).
3. Cause 2: replay esdef-create-domain-gp3-server-defaults#task#1 and opensearch-domain-lifecycle-and-config-inspection#task#5 — inspect DomainStatus.ModifyingProperties / ChangeProgressStatus.PendingProperties for leaf names (ClusterConfig.InstanceType, AdvancedSecurityOptions.AnonymousAuthEnabled) and for the AccessPolicies entry's ValueType.
4. Cause 3: replay opensearch-domain-lifecycle-and-config-inspection#task#2 — CreateDomain with AccessPolicies whose Principal.AWS is the bare account id, then DescribeDomain and compare the echoed document with the submitted one.
5. Cause 4 and 5: replay esdef-create-domain-gp3-server-defaults#task#2 (DescribeDomain) and opensearch-domain-lifecycle-and-config-inspection#teardown#17 (DeleteDomain) — check DomainStatus.Endpoint's form/presence and ServiceSoftwareOptions.CurrentVersion.
6. Cause 6: replay opensearch-domain-lifecycle-and-config-inspection#task#4 (DescribeDomainConfig) — check CognitoOptions/IdentityCenterOptions/VPCOptions Status.State and AutoTuneOptions.Options.
7. Cause 7: replay opensearch-domain-lifecycle-and-config-inspection#task#5 — check ChangeProgressStatus.TotalNumberOfStages.
