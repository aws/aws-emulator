Issue-Source: trace-sweep-divergence
Issue-Trace: dlm-image-management-policy
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dlm-image-management-policy
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.Parameters
Issue-Commit: 1b60b876111feda85f3c849517eb9e6584eedb74

# GetLifecyclePolicy invents a Parameters structure on a custom AMI policy that stated none

> _Note: this issue was identified against the code base at commit `1b60b876111feda85f3c849517eb9e6584eedb74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
A read of a custom IMAGE_MANAGEMENT (EBS-backed AMI) lifecycle policy whose configuration stated no PolicyDetails$Parameters comes back carrying a Parameters structure the request never supplied, because the write path stamps a materialized NoReboot onto the stored configuration.

Cause 1 of 1: Policy.PolicyDetails.Parameters is present with NoReboot=true on a read of an AMI policy created without Parameters.

AWS: GetLifecyclePolicy returns LifecyclePolicy$PolicyDetails, "the configuration of the lifecycle policy" (Smithy: GetLifecyclePolicy -> LifecyclePolicy -> PolicyDetails; PolicyDetails$Parameters targets Parameters, documented "[Custom snapshot and AMI policies only] A set of optional parameters ... The set of valid parameters depends on the combination of policy type and target resource type"; Parameters$NoReboot is documented "[Custom AMI policies only] ... The default is true (instances are not rebooted)"). Parameters and all of its members are OPTIONAL and non-defaulted in the model: nothing in the model marks Parameters, NoReboot, ExcludeBootVolume or ExcludeDataVolumeTags @default or @required. The recorded response for this fixture's read of an IMAGE_MANAGEMENT policy omits PolicyDetails.Parameters entirely (recorded behavior only for the absence; HTTP 200, no error involved). That is, NoReboot's documented "default is true" describes how Data Lifecycle Manager BEHAVES when a policy states no NoReboot (it does not reboot targeted instances); it is not a value the control plane writes into the policy document, so a read reports only the optional parameters the caller actually configured.

Emulator: the read answers HTTP 200 with the same body except that $.Policy.PolicyDetails.Parameters is present as {"NoReboot": true} where AWS returns no Parameters member at all (replay detail: "$.Policy.PolicyDetails.Parameters: type_mismatch (expected None, got {'NoReboot': True})"). This is deliberate materialization at write time, not a serializer accident: crate::state::materialized_parameters stamps DEFAULT_NO_REBOOT (true) onto any custom policy whose PolicyType manages AMIs, creating the Parameters structure when the request carried none, and the stamped structure is then stored and replayed by every later read (Get/GetLifecyclePolicies). The same stamping reaches a custom snapshot policy that targets instances via DEFAULT_EXCLUDE_BOOT_VOLUME, so the identical over-reporting is expected for ExcludeBootVolume on that combination.

Why: a client cannot distinguish a policy that explicitly configured NoReboot from one that left the optional parameter unset, so round-tripping a read into UpdateLifecyclePolicy or into infrastructure-as-code silently converts an unset optional parameter into an explicitly stated one. Config-drift detection and response-shape assertions (exact-body comparisons, SDK equality checks against a locally built expected PolicyDetails) see a member AWS never sends and report a difference that does not exist on AWS, and any diff of "desired vs actual" policy document shows a permanent spurious change.

Fix: the policy-state materialization, not the read path — crate::state::materialized_parameters and its DEFAULT_NO_REBOOT / DEFAULT_EXCLUDE_BOOT_VOLUME constants, invoked from materialized_policy_details on the CreateLifecyclePolicy and UpdateLifecyclePolicy handlers. The optional-parameter structure must be stored exactly as the request stated it (absent when absent, and with only the members stated when present); a documented runtime default such as NoReboot belongs to the emulator's execution semantics, not to the stored configuration a read echoes back.

Traces: dlm-image-management-policy#task#3 (GetLifecyclePolicy on an IMAGE_MANAGEMENT policy created without Parameters).

## Evidence
Fixture dlm-image-management-policy, step task#3, GetLifecyclePolicy.
Expected (recorded AWS): HTTP 200, $.Policy.PolicyDetails carries no Parameters member.
Actual (emulator): HTTP 200, $.Policy.PolicyDetails.Parameters = {"NoReboot": true}.
Replay verdict: mismatch, divergence_key service=dlm|GetLifecyclePolicy|status=200->200|keys=$.Policy.PolicyDetails.Parameters, detail "$.Policy.PolicyDetails.Parameters: type_mismatch (expected None, got {'NoReboot': True})".
Source of the invented member: services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:667 (materialized_parameters) with the constant at state.rs:174 (DEFAULT_NO_REBOOT = true), reached from state.rs:634 (materialized_policy_details) which the create handler calls at services/service-com-amazonaws-dlm-dlm-20180112/src/ops/create_lifecycle_policy.rs:444.
Model: Parameters, Parameters$NoReboot and PolicyDetails$Parameters are all optional with no @default; Parameters$NoReboot doc "[Custom AMI policies only] ... The default is true (instances are not rebooted)" describes policy execution, and nothing in the model makes it part of the stored/returned document.

## Steps to Reproduce
1. CreateLifecyclePolicy with a custom (STANDARD) PolicyDetails whose PolicyType is IMAGE_MANAGEMENT — ResourceTypes, TargetTags and Schedules stated, and NO Parameters member anywhere in the request.
2. GetLifecyclePolicy with the returned PolicyId.
3. Inspect $.Policy.PolicyDetails in the 200 response: AWS returns no Parameters member; the emulator returns Parameters = {"NoReboot": true}.
4. Replay fixture dlm-image-management-policy, step task#3, to observe the same mismatch on the recorded transcript.
