Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-patch-baseline-delete-id-charset-grammar
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-patch-baseline-delete-id-charset-grammar ssm-patch-baseline-delete-id-length-bound ssm-patch-baseline-global-filter-value-count-bounds ssm-patch-group-default-baseline-fallback-and-filtered-mapping ssm-patch-group-deregister-length-bounds ssm-describe-available-patches-filter-key-length-bounds
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DeletePatchBaseline|4xx->4xx|$.message
Issue-Key: ssm|DeletePatchBaseline|4xx->2xx|$.__type
Issue-Key: ssm|DeletePatchBaseline|4xx->2xx|$.message
Issue-Key: ssm|DeletePatchBaseline|4xx->2xx|status
Issue-Key: ssm|DeletePatchBaseline|2xx->2xx|$.BaselineId
Issue-Key: ssm|GetPatchBaseline|2xx->2xx|$.AvailableSecurityUpdatesComplianceStatus
Issue-Key: ssm|DescribePatchGroups|2xx->2xx|$.Mappings[*]
Issue-Key: ssm|DescribeAvailablePatches|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# Patch baseline identity, group mappings and patch filter refusals diverge from AWS

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePatchBaseline

## Description
Six net-new causes on the Patch Manager control plane: the baseline identifier grammar and the identity returned for a baseline, a missing group mapping, a missing compliance member, and the available-patches filter refusal. The already-open patch issues (20261125T060148Z-ssm-seeded-patch-and-document-reference-catalogs-are.md, 20261126T165753Z-ssm-describeeffectivepatchesforpatchbaseline-does-not-compute-a-baseline-s.md, 20261126T103859Z-ssm-deletepatchbaseline-s-resourceinuseexception-message-is-emulator-authored.md) own other clusters, not these.

Cause 1 of 6: DeletePatchBaseline quotes the wrong baselineId grammar.
AWS: "1 validation error detected: Value '<value>' at 'baselineId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(pb-[0-9a-f]{17})|(arn.*)$" — the service's grammar is a pb- id or an ARN.
Emulator: quotes '[a-zA-Z0-9_\\-:/]{20,128}' (the on-disk Smithy BaselineId pattern). The recorded service grammar is the wire requirement.
Why: the caller is told a grammar that is not the one enforced.
Fix: the baselineId constraint used by validation plus its message.
Traces: ssm-patch-baseline-delete-id-charset-grammar#task#2.

Cause 2 of 6: DeletePatchBaseline accepts a baselineId AWS refuses.
AWS: 400, __type ValidationException, message "1 validation error detected: Value '<value>' at 'baselineId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(pb-[0-9a-f]{17})|(arn.*)$" for an id such as pb-<17 hex>_x.
Emulator: 200.
Why: an id outside AWS's grammar is resolved and a delete is attempted, so a client typo silently targets something.
Fix: DeletePatchBaseline input validation using the grammar from cause 1.
Traces: ssm-patch-baseline-delete-id-charset-grammar#task#4.

Cause 3 of 6: DeletePatchBaseline returns a bare id instead of the baseline ARN.
AWS: BaselineId in the response is the baseline's ARN, arn:aws:ssm:<region>:<account>:patchbaseline/<pb-id>.
Emulator: returns the bare pb- id.
Why: a client that stores the returned identity (or compares it with an ARN from another call) holds a different identifier than AWS issued.
Fix: the DeletePatchBaseline response projection.
Traces: ssm-patch-baseline-delete-id-charset-grammar#task#5.

Cause 4 of 6: GetPatchBaseline omits AvailableSecurityUpdatesComplianceStatus.
AWS: the response carries AvailableSecurityUpdatesComplianceStatus (recorded: 'NON_COMPLIANT'). Smithy: GetPatchBaselineResult$AvailableSecurityUpdatesComplianceStatus (PatchComplianceStatus enum).
Emulator: member absent.
Why: a client reading the baseline's security-update compliance posture gets null for a modeled member.
Fix: the GetPatchBaseline response projection / baseline state.
Traces: ssm-patch-baseline-global-filter-value-count-bounds#task#2.

Cause 5 of 6: DescribePatchGroups returns no mapping for a registered group.
AWS: Mappings carries one entry per patch-group registration — {'BaselineIdentity': {'BaselineId', 'BaselineName', 'DefaultBaseline', 'OperatingSystem'}, 'PatchGroup': '<group>'}.
Emulator: the expected mapping is absent.
Why: a client cannot discover which baseline a patch group resolves to, so patch-group administration reads as unconfigured.
Fix: the DescribePatchGroups projection over patch-group registration state (including the filtered form).
Traces: ssm-patch-group-default-baseline-fallback-and-filtered-mapping#task#7.

Cause 6 of 6: DescribeAvailablePatches re-words the unknown-filter-key refusal and lists the wrong keys.
AWS: 400 with message "<key> is not a known filter key, allowed keys are PATCH_SET,PRODUCT,PRODUCT_FAMILY,CLASSIFICATION,MSRC_SEVERITY,PATCH_ID,SECTION,PRIORITY,SEVERITY,SECURITY,NAME,EPOCH,VERSION,RELEASE,ARCH,REPOSITORY,ADVISORY_ID,BUGZILLA_ID,CVE_ID" — fixed text with the submitted key; the key list is AWS's and includes SECTION, PRIORITY and SECURITY.
Emulator: "The following filter key is not valid: <key>. Valid filter keys include: [PATCH_SET, PRODUCT, ...]." — a different sentence, bracketed list, and a set missing SECTION, PRIORITY and SECURITY.
Why: the message is not AWS's and tells the caller three legal keys do not exist.
Fix: the DescribeAvailablePatches filter-key validation and message.
Traces: ssm-describe-available-patches-filter-key-length-bounds#task#1.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 ssm-patch-baseline-delete-id-charset-grammar#task#2: $.message expected pattern '^(pb-[0-9a-f]{17})|(arn.*)$', got '[a-zA-Z0-9_\\-:/]{20,128}'.
Cause 2 #task#4: expected 400 ValidationException with that pattern, got 200.
Cause 3 #task#5: $.BaselineId expected 'arn:aws:ssm:us-west-2:000000000000:patchbaseline/pb-<id>', got 'pb-<id>'.
Cause 4 ssm-patch-baseline-global-filter-value-count-bounds#task#2: $.AvailableSecurityUpdatesComplianceStatus missing (expected 'NON_COMPLIANT').
Cause 5 ssm-patch-group-default-baseline-fallback-and-filtered-mapping#task#7: $.Mappings[0] missing (expected the BaselineIdentity/PatchGroup pair).
Cause 6 ssm-describe-available-patches-filter-key-length-bounds#task#1: $.message sentence and allowed-key list both diverge (emulator list omits SECTION, PRIORITY, SECURITY).

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Causes 1-3: fixture ssm-patch-baseline-delete-id-charset-grammar steps task#2, task#4, task#5.
3. Cause 4: fixture ssm-patch-baseline-global-filter-value-count-bounds step task#2.
4. Cause 5: fixture ssm-patch-group-default-baseline-fallback-and-filtered-mapping step task#7 (and ssm-patch-group-deregister-length-bounds step task#6).
5. Cause 6: fixture ssm-describe-available-patches-filter-key-length-bounds step task#1.
