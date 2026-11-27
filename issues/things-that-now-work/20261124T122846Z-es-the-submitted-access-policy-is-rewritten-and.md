Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-domain-lifecycle-and-config-inspection
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-domain-lifecycle-and-config-inspection opensearch-domain-version-capacity-and-software-update-management
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DescribeDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].PendingValue
Issue-Key: es|DescribeDomainConfig|2xx->2xx|$.DomainConfig.ModifyingProperties[*].PendingValue
Issue-Key: es|DescribeDomainChangeProgress|2xx->2xx|$.ChangeProgressStatus.PendingProperties[*]
Issue-Key: es|CreateDomain|2xx->2xx|$.DomainStatus.ModifyingProperties[*].PendingValue
Issue-Related: 20261121T102109Z-es-createdomain-re-create-response-changeprogressdetails-configchangestatus-is.md
Issue-Commit: 5834ad3c3ba75550acae8591eeb4264fb69879c4

# es: the submitted access policy is rewritten and the domain change-progress projection diverges (5 causes)

> _Note: this issue was identified against the code base at commit `5834ad3c3ba75550acae8591eeb4264fb69879c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
This bundle carries FIVE net-new causes on the es domain configuration/change-progress read surface. All five answer the modelled status (200 -> 200); every divergence is in the response body. A fixer must close all five before submitting.

Shared fault: the emulator does not project the domain's pending configuration the way AWS does - it stores a canonicalized copy of the access policy the caller submitted, enumerates pending properties from its own internal option struct, and seeds a new change in the wrong ConfigChangeStatus.

Cause 1 of 5 - DescribeDomain (fixture opensearch-domain-lifecycle-and-config-inspection, step task#2): DomainStatus.ModifyingProperties entry for AccessPolicies reports a PendingValue whose IAM principal has been rewritten.
AWS: Smithy com.amazonaws.opensearch#ModifyingProperties{Name: String, ActiveValue: String, PendingValue: String, ValueType: PropertyValueType[PLAIN_TEXT|STRINGIFIED_JSON]}; the member is a plain String and the model prescribes no normalization of its contents. Recorded behavior: the AccessPolicies PendingValue is the policy document EXACTLY as the request supplied it, so a Statement[].Principal.AWS submitted as a bare account id is read back as that bare account id.
Emulator: the same PendingValue comes back with Principal {"AWS":"<account-id>"} expanded to {"AWS":"arn:aws:iam::<account-id>:root"}; the rest of the document is unchanged.
Why: the emulator silently alters a security document the caller authored. A client that reads the policy back, edits one statement and re-submits it writes a policy it never wrote, and a client that diffs the policy it submitted against the policy the service reports sees a spurious change on every poll.
Fix: the domain access-policy storage path (store the submitted PolicyDocument verbatim - no principal canonicalization) and the ModifyingProperties projection in the DescribeDomain response serializer.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#2.

Cause 2 of 5 - DescribeDomainConfig (same fixture, step task#4): the identical rewrite on the DomainConfig projection, i.e. the defect is in the stored/serialized policy and not in one read path.
AWS: same Smithy ModifyingProperties shape, reached through DomainConfig.ModifyingProperties; recorded behavior is again the submitted document, principal unrewritten.
Emulator: DomainConfig.ModifyingProperties[AccessPolicies].PendingValue carries the arn:aws:iam::<account-id>:root form.
Why: as cause 1 - and because DescribeDomainConfig is the read a config-management client uses to decide whether its desired policy is already applied, that client applies the policy again forever.
Fix: as cause 1, plus the DescribeDomainConfig response serializer's ModifyingProperties projection.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#4.

Cause 3 of 5 - DescribeDomainChangeProgress (same fixture, step task#5): ChangeProgressStatus.PendingProperties lists properties AWS does not report, so the list's membership and ordering diverge from the AccessPolicies entry onward.
AWS: Smithy com.amazonaws.opensearch#ChangeProgressStatusDetails.PendingProperties: StringList, documented "The list of properties in the domain configuration change that are still pending". Recorded behavior: one entry per AWS-enumerated domain property, formatted "<PropertyName> : <value>", in AWS's enumeration order - after the AccessPolicies entry AWS lists AdvancedSecurityOptions.AnonymousAuthEnabled, AdvancedSecurityOptions.InternalUserDatabaseEnabled, AdvancedSecurityOptions.MasterUserOptions, AdvancedSecurityOptions.SAMLOptions, DomainEndpointOptions, EBSOptions, ClusterConfig.ColdStorageOptions, ...
Emulator: it inserts entries AWS does not enumerate (AdvancedOptions, AdvancedSecurityOptions.AnonymousAuthDisableDate, AdvancedSecurityOptions.IAMFederationOptions, AdvancedSecurityOptions.JWTOptions) and drops/defers the ones AWS lists, so every position from index 4 on holds a different property; the AccessPolicies entry additionally carries the rewritten principal of cause 1.
Why: a client that looks a property up in PendingProperties to learn whether that option is still in flight finds a property AWS never reports and misses the one it asked about, so it concludes a pending change has completed (or has not started).
Fix: the DescribeDomainChangeProgress handler's pending-property enumeration (drive it from AWS's property set and order, not from the internal option struct).
Traces: opensearch-domain-lifecycle-and-config-inspection#task#5.

Cause 4 of 5 - CreateDomain (same fixture, step task#14): the create response reports ChangeProgressDetails.ConfigChangeStatus as the not-yet-picked-up state instead of the state of the change the create just started, and carries the cause-1 policy rewrite.
AWS: Smithy ConfigChangeStatus enum = [Pending, Initializing, Validating, ValidationFailed, ApplyingChanges, Completed, PendingUserInput, Cancelled], documented "The current status of the configuration change". Recorded behavior: the change a CreateDomain starts is already picked up when the create answers, so the create response reports "Initializing"; "Pending" is the state of a change that has not been picked up.
Emulator: ConfigChangeStatus is "Pending", and DomainStatus.ModifyingProperties[AccessPolicies].PendingValue is rewritten as in cause 1.
Why: a client that treats Pending as "not started" waits on, or re-submits, a change that is already in flight.
Fix: the CreateDomain handler's change-progress state - seed the new change at Initializing - plus the shared policy-storage fix of cause 1.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#14.

Cause 5 of 5 - CreateDomain (fixture opensearch-domain-version-capacity-and-software-update-management, step setup#1): the create response for a different fixture's domain shows the same AccessPolicies PendingValue rewrite, confirming it is the create path's storage and not one fixture's shape.
AWS: as cause 1.
Emulator: DomainStatus.ModifyingProperties[AccessPolicies].PendingValue carries the arn:aws:iam::<account-id>:root principal where the request submitted the bare account id.
Why: as cause 1.
Fix: as cause 1 (CreateDomain access-policy storage).
Traces: opensearch-domain-version-capacity-and-software-update-management#setup#1.

Related open issues that do NOT cover these clusters (the replay reports all five as owned by no open issue): 20261121T102109Z-es-createdomain-re-create-response-changeprogressdetails-configchangestatus-is.md describes the ConfigChangeStatus and ModifyingProperties faults on step task#14 but its replay key no longer matches the cluster, and it says nothing about task#2/#4/#5 or the version-capacity fixture; 20261114T061447Z-es-domain-configuration-projection-diverges-modifyingproperties-entries-pendingvalue.md describes the ModifyingProperties PendingValue class on other steps. The principal-rewrite and PendingProperties-enumeration causes above are what still diverges today.

## Evidence
Replayed corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 492 steps, 139 mismatched). All five clusters: verdict mismatch, expected status == actual status (200).

Cause 1 - fixture opensearch-domain-lifecycle-and-config-inspection step task#2, service es, operation DescribeDomain:
  $.DomainStatus.ModifyingProperties[3].PendingValue: value_mismatch
    expected '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"000000000000"},"Action":"es:*","Resource":"arn:aws:es:us-west-2:000000000000:domain/sim2real-b48d3a5e/*"}]}'
    got      the same document with "Principal":{"AWS":"arn:aws:iam::000000000000:root"}

Cause 2 - same fixture step task#4, operation DescribeDomainConfig:
  $.DomainConfig.ModifyingProperties[3].PendingValue: value_mismatch, identical expected-vs-got pair as cause 1.

Cause 3 - same fixture step task#5, operation DescribeDomainChangeProgress:
  $.ChangeProgressStatus.PendingProperties[3]: expected 'AccessPolicies : {..."AWS":"000000000000"...}', got the same entry with the rewritten principal
  $.ChangeProgressStatus.PendingProperties[4]: expected 'AdvancedSecurityOptions.AnonymousAuthEnabled : false', got 'AdvancedOptions : {"override_main_response_version":"false","rest.action.multi.allow_explicit_index":"true"}'
  $.ChangeProgressStatus.PendingProperties[5]: expected 'AdvancedSecurityOptions.InternalUserDatabaseEnabled : false', got 'AdvancedSecurityOptions.AnonymousAuthDisableDate : false'
  $.ChangeProgressStatus.PendingProperties[6]: expected 'AdvancedSecurityOptions.MasterUserOptions : false', got 'AdvancedSecurityOptions.AnonymousAuthEnabled : false'
  $.ChangeProgressStatus.PendingProperties[7]: expected 'AdvancedSecurityOptions.SAMLOptions : false', got 'AdvancedSecurityOptions.IAMFederationOptions : false'
  $.ChangeProgressStatus.PendingProperties[8]: expected 'DomainEndpointOptions : {"CustomEndpointEnabled":false,"EnforceHTTPS":false,"TLSSecurityPolicy":"Policy-Min-TLS-1-2-2019-07"}', got 'AdvancedSecurityOptions.InternalUserDatabaseEnabled : false'
  $.ChangeProgressStatus.PendingProperties[9]: expected 'EBSOptions : {"EBSEnabled":true,"Iops":3000,"Throughput":125,"VolumeSize":10,"VolumeType":"gp3"}', got 'AdvancedSecurityOptions.JWTOptions : false'
  $.ChangeProgressStatus.PendingProperties[10]: expected 'ClusterConfig.ColdStorageOptions : {"Enabled":false}', got 'AdvancedSecurityOptions.MasterUserOptions : false'
  ... the same shift is reported through index [35].

Cause 4 - same fixture step task#14, operation CreateDomain:
  $.DomainStatus.ChangeProgressDetails.ConfigChangeStatus: value_mismatch (expected 'Initializing', got 'Pending')
  $.DomainStatus.ModifyingProperties[3].PendingValue: the cause-1 principal rewrite.

Cause 5 - fixture opensearch-domain-version-capacity-and-software-update-management step setup#1, operation CreateDomain:
  $.DomainStatus.ModifyingProperties[3].PendingValue: the cause-1 principal rewrite (domain sim2real-9c5dab9f).

Per-run values: the account id 000000000000, the domain names sim2real-b48d3a5e / sim2real-9c5dab9f and the element indices of this recording are NOT the requirement. The requirements are (a) the AccessPolicies PendingValue echoes the submitted policy document byte-for-byte, principal in the form the request used; (b) PendingProperties contains exactly AWS's enumerated properties, formatted '<Name> : <value>', in AWS's order; (c) the change a CreateDomain starts is reported with ConfigChangeStatus 'Initializing'.

Smithy cited: com.amazonaws.opensearch#ModifyingProperties{Name, ActiveValue, PendingValue: String, ValueType: PropertyValueType[PLAIN_TEXT|STRINGIFIED_JSON]}; com.amazonaws.opensearch#ChangeProgressStatusDetails.PendingProperties: StringList; ConfigChangeStatus enum [Pending, Initializing, Validating, ValidationFailed, ApplyingChanges, Completed, PendingUserInput, Cancelled].

## Steps to Reproduce
1. git_clone, then replay_wire_corpus against the emulator built from the checkout.
2. Cause 1/2: drive fixture opensearch-domain-lifecycle-and-config-inspection to steps task#2 (DescribeDomain) and task#4 (DescribeDomainConfig) - the domain is created with an AccessPolicies document whose Statement[].Principal.AWS is the bare account id. Read ModifyingProperties, find the AccessPolicies entry, and compare its PendingValue byte-for-byte with the submitted document: the principal must not be expanded to arn:aws:iam::<account-id>:root.
3. Cause 3: step task#5 (DescribeDomainChangeProgress) - compare ChangeProgressStatus.PendingProperties element by element with the recorded response; no entry for AdvancedOptions, AdvancedSecurityOptions.AnonymousAuthDisableDate, AdvancedSecurityOptions.IAMFederationOptions or AdvancedSecurityOptions.JWTOptions may appear, and DomainEndpointOptions / EBSOptions / ClusterConfig.ColdStorageOptions must appear at AWS's positions.
4. Cause 4: step task#14 (CreateDomain re-create) - $.DomainStatus.ChangeProgressDetails.ConfigChangeStatus must be 'Initializing'.
5. Cause 5: drive fixture opensearch-domain-version-capacity-and-software-update-management step setup#1 (CreateDomain) and check the AccessPolicies PendingValue the same way.
6. All five steps must pass against the recorded responses.

## Disposition

* Causes 1, 2 and 5 (the AccessPolicies `PendingValue`) are GONE from this code base: a
  submitted policy document is held with only the insignificant whitespace between its JSON
  tokens dropped, and a change's property reports an account principal as the account
  itself, so a principal submitted as a bare account id is read back as that bare account
  id. Replay reports the `DescribeDomain` and `DescribeDomainConfig` keys of this report as
  fixed and the `CreateDomain` one past this cause.
  `tests/all/change_property_account_principal.rs` and
  `tests/all/change_property_enumeration.rs` pin it over a table of principal spellings.
* Cause 3 (`PendingProperties`) is FIXED on the property set and on every position the
  recorded response discloses here: the change's own enumeration is now separate from the
  descriptor's, reports no property for the advanced options map nor for the security
  block's anonymous-auth date, IAM-federation and JWT settings, and enumerates the cluster's
  topology after the endpoint options and the EBS volume — so `AccessPolicies`, the four
  security properties, `DomainEndpointOptions`, `EBSOptions` and
  `ClusterConfig.ColdStorageOptions` land where AWS reports them. The ORDER of the
  properties past that disclosed prefix is not a published rule — the member is a plain
  `StringList` and the same run has the service ordering one change's properties differently
  on its two surfaces — and replay still reports those later positions as divergent.
* Cause 4 (`ChangeProgressDetails.ConfigChangeStatus` on the create response) is DECLINED as
  a per-run value, on the same evidence as
  `issues/things-that-now-work/20261118T145127Z-es-createdomain-re-create-step-reports-configchangestatus-pending.md`:
  the same key is recorded as `Pending` on other creates of this corpus (see
  `issues/things-that-now-work/20261029T190009Z-es-domainstatus-and-domainconfig-responses-omit-modeled-option.md`,
  cause 1), so which early state of its own change workflow the service has reached when it
  serialises a create response is a race rather than a rule. The emulator reports the state
  the change is really in, the same value on every path that reports that change.
