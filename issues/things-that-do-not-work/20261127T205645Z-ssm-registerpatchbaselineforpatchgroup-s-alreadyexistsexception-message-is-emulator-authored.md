Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.patch-group-registration-lifecycle
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.patch-group-registration-lifecycle|run#19|AssertionError|assert dup_message == (
Issue-Commit: 45e339952ace655163e1a6b7e05141367a36929a

# ssm RegisterPatchBaselineForPatchGroup's AlreadyExistsException message is emulator-authored: it names the conflicting baseline id instead of the OperatingSystem AWS reports

> _Note: this issue was identified against the code base at commit `45e339952ace655163e1a6b7e05141367a36929a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterPatchBaselineForPatchGroup

## Description
One net-new root cause under this heading.

Cause 1 of 1: registering a patch group that is already mapped for an operating system to a second baseline of that same operating system is refused with the right status and error code but the wrong message (failing check: ssm.patch-group-registration-lifecycle).

AWS: Smithy (ssm): RegisterPatchBaselineForPatchGroup input {BaselineId (required, length 20..128, pattern ^[a-zA-Z0-9_\\-:/]{20,128}$), PatchGroup (required, length 1..256)}; modeled errors include AlreadyExistsException (client, httpError 400), documented as returned "if an attempt is made to register a patch group with a patch baseline that is already registered with a different patch baseline". AWS refuses the second registration with HTTP 400 AlreadyExistsException and the message "Patch Group <PatchGroup> already has a baseline registered for OperatingSystem <OperatingSystem>." Fixed text: "Patch Group ", " already has a baseline registered for OperatingSystem " and the trailing ".". Variable parts: the PatchGroup value taken from the request, and the OperatingSystem of the baseline the group is already registered with, taken from the service's state. No baseline id appears anywhere in AWS's sentence, and the existing mapping is left unchanged.

Emulator: HTTP 400 AlreadyExistsException (code and status correct) with the message "Patch group <PatchGroup> is already registered with patch baseline <BaselineId>" — different fixed text ("Patch group" lower-cased, " is already registered with patch baseline ", no trailing period) and it substitutes the already-registered BASELINE ID for the OPERATING SYSTEM. The rest of the lifecycle the check exercises (register, GetPatchBaselineForPatchGroup, GetPatchBaseline PatchGroups, DescribePatchGroups, deregister, re-register the freed group name) is only reached after this assertion, so it is not graded here.

Why: patch-group registration is scoped per operating system, and AWS's message is the only thing telling the caller WHICH OS slot is occupied — the fact needed to decide whether to deregister or to register a baseline for a different OS. A client that displays or pattern-matches the service's message sees wording AWS never emits, and remediation keyed on the documented sentence does not fire; the emulator instead leaks a baseline id AWS does not disclose in this refusal.

Fix: the RegisterPatchBaselineForPatchGroup handler's AlreadyExistsException path and the patch-group state module's message builder it calls — the refusal value must carry the operating system of the conflicting registration rather than its baseline id, and the sentence must be AWS's.

Traces: ssm.patch-group-registration-lifecycle. Open issue 20261120T214510Z-ssm-5-net-new-causes-patchbaseline-tag-reads.md states this same wording divergence as its Cause 2 (citing services/service-com-amazonaws-ssm-amazonssm/src/state/patch_groups.rs already_registered_message and src/ops/register_patch_baseline_for_patch_group.rs), but it does not carry this check's replay key, so this check is still ungated; close them together if that issue is fixed first.

## Evidence
Expected (AWS): error code AlreadyExistsException, HTTP 400, message "Patch Group <patch group> already has a baseline registered for OperatingSystem AMAZON_LINUX_2." — the check asserts exactly `f"Patch Group {patch_group} already has a baseline registered for OperatingSystem AMAZON_LINUX_2."` after already accepting the code.

Actual (emulator): AssertionError raised on the message assertion with the served text `Patch group sim2real-behav-pg-0c9cae6d8d81 is already registered with patch baseline pb-733e09a8e1c71cea5`. Here `sim2real-behav-pg-0c9cae6d8d81` is the PatchGroup from the request and `pb-733e09a8e1c71cea5` is a per-run minted baseline id — neither is part of the requirement; the requirement is the fixed text plus the PatchGroup and the registered baseline's OperatingSystem.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the behavioral suite (simparity-behavioral-aws-v1).
2. ssm CreatePatchBaseline(Name=<a>, OperatingSystem='AMAZON_LINUX_2', ApprovalRules={PatchRules:[{PatchFilterGroup:{PatchFilters:[{Key:'PRODUCT',Values:['AmazonLinux2']}]},ApproveAfterDays:7,ComplianceLevel:'UNSPECIFIED',EnableNonSecurity:false}]}) twice, giving baselines A and B.
3. RegisterPatchBaselineForPatchGroup(BaselineId=A, PatchGroup=<g>) — succeeds.
4. RegisterPatchBaselineForPatchGroup(BaselineId=B, PatchGroup=<g>) — observe HTTP 400 AlreadyExistsException with message 'Patch group <g> is already registered with patch baseline <A>' instead of 'Patch Group <g> already has a baseline registered for OperatingSystem AMAZON_LINUX_2.'
5. Re-run the check ssm.patch-group-registration-lifecycle to confirm; after the message matches, the rest of the lifecycle (deregister frees the group name, re-register to B, GetPatchBaselineForPatchGroup reports B) must also pass.
