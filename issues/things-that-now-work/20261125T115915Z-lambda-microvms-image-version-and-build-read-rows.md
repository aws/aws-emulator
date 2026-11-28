Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-customer-image-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-customer-image-lifecycle
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|ListMicrovmImageVersions|2xx->2xx|$.items[*].baseImageVersion
Issue-Key: lambda-microvms|GetMicrovmImageBuild|2xx->2xx|$.buildId
Issue-Commit: f89783cb4c796b94ed6e2a4b91d09cf066e599c6

# lambda-microvms image-version and build read rows publish emulator-chosen values instead of the resolution and identifier form AWS serves

> _Note: this issue was identified against the code base at commit `f89783cb4c796b94ed6e2a4b91d09cf066e599c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMicrovmImageVersions

## Description
This issue bundles 2 distinct root causes in the lambda-microvms emulator, both observed on replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, /__admin/health 200 and the data listener accepting; 13 fixtures replayed, 104 steps, 97 passed, 7 mismatched, 0 errored, 0 blocked, score 93.3%, 7 divergence clusters, bootstrap_signal false). Both causes are of one defect class: a member of a MicroVM-image READ response whose value the SERVICE supplies (the base-image pedigree an unpinned build resolved to, and the identifier a build is minted under) is answered from a constant or a counter this emulator chose for itself rather than from the resolution AWS performs / the form AWS mints. A fixer must close BOTH (Cause 1 of 2 and Cause 2 of 2) before submitting; each cause names the fixture step it must be re-replayed on, and both clusters are reported by the replay with an EMPTY stub_path and an EMPTY owned_by.

=== Cause 1 of 2: the version LISTING row reports baseImageVersion as the constant initial version instead of the base-image release the unpinned create resolved to ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#7 (ListMicrovmImageVersions, HTTP 200 on both sides).

AWS: Smithy ListMicrovmImageVersions is GET /2025-09-09/microvm-images/{imageIdentifier}/versions -> 200, output items @required MicrovmImageVersionSummaryList. MicrovmImageVersionSummary$baseImageVersion is optional, targets Version (@length 1..2048, pattern ^[^\s]+$), documented "The specific version of the base MicroVM image." — i.e. the base-image release THIS image version was actually built from: the release the request pinned, or, for a create that pinned none, the release the service RESOLVES for the named baseImageArn. For the concrete label the model fixes nothing, so this is recorded behavior only: for a create against the AWS-managed base image arn:aws:lambda:<region>:aws:microvm-image:al2023-1 with no baseImageVersion, the recording serves baseImageVersion '2.0' on the version LISTING row (the same resolved pedigree the create response and the single-version read report in the same fixture). The requirement is that a listing row report the release resolved from the named base image — for a managed reference, the release label the catalogue's newest generation ships, in the label form that catalogue publishes it in — not a per-service constant. The recorded literal '2.0' is this run's resolved value and not itself the contract; what is contract is that the row carry the SAME resolved pedigree the create accepted, rather than a value independent of the base image named.

Emulator: the row answers baseImageVersion '1.0' — replay reports `$.items[0].baseImageVersion: value_mismatch (expected '2.0', got '1.0')` with status 200 on both sides. The row projection is faithful: ops/list_microvm_image_versions.rs payload_of copies the stored field verbatim (base_image_version: version.base_image_version.clone()). The stored value comes from the resolution, which for a managed base-image reference returns the release label the emulator's own catalogue publishes for the newest generation — MANAGED_BASE_IMAGE_VERSIONS publishes generation "1" shipping release "1.0" (and generation "0" shipping "0.1"), and the table's own comment states the release labels are chosen rather than AWS's — so every unpinned create on the managed base image is recorded with pedigree '1.0', which happens to coincide with FIRST_IMAGE_VERSION and is indistinguishable from "nothing was resolved".

Why: baseImageVersion is how a client audits and pins the base layer of an image it built. A listing is the bulk read a fleet-wide sweep uses, so a reproducible-build gate, a CVE re-base sweep or a promotion check that enumerates an image's versions is told every version was built on base release 1.0 whichever managed base image it actually named, and so never learns a re-base is due; a client that reads the create response and then re-reads the same pedigree from the listing cannot detect the difference either, because both report the same constant.

Fix: the base-image version RESOLUTION in the service state — ComAmazonawsLambdamicrovmsLambdamicrovmsState::resolve_base_image_version, plus the catalogue release labels it resolves from (MANAGED_BASE_IMAGE_VERSIONS) — must resolve an unpinned reference to the release the named base image publishes, and the version record must store that resolved value so the create response, the single-version read and this listing row all report one pedigree. Do NOT patch the listing projection: it is faithful and rewriting it there would make the listing disagree with the other two read paths.

Related, NOT covering this cluster: open '20261117T040517Z-lambda-microvms-answers-microvm-and-image-version-reads.md' carries this same resolution defect for CreateMicrovmImage (#setup#2) and GetMicrovmImageVersion (#task#6) ONLY — this replay reports those two clusters as owned by it and reports THIS listing cluster (task#7) with an EMPTY owned_by, so closing that issue does not re-replay this step.

Traces: lambdamicrovms-customer-image-lifecycle#task#7 (ListMicrovmImageVersions, 200 vs 200).

=== Cause 2 of 2: the single-build read echoes a buildId minted as a zero-padded global counter, which is not the build-request-identifier form the recording carries ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#9 (GetMicrovmImageBuild, HTTP 200 on both sides).

AWS: Smithy GetMicrovmImageBuild is GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds/{buildId} -> 200; output buildId is @required NonBlankString (@length 1..2048, pattern ^[^\s]+$), documented "The build request ID", alongside imageArn, imageVersion, buildState, architecture, chipset, chipsetGeneration (all @required), stateReason, createdAt and snapshotBuild. The model fixes no concrete FORM for the identifier, so this is recorded behavior only: the corpus records a build identifier as a shaped value, and the requirement is that the service mint build request ids in the form AWS mints them in and echo the addressed one verbatim on this read. Note the replay's rendering of this diff: `$.buildId: value_mismatch (expected 'build-0000000000000000', got 'build-0000000000000000')` — the two render identically, which is the signature of a form divergence BELOW the rendering (byte length, digit count or encoding), so a fixer must compare the recorded identifier's raw bytes against the minted one rather than compare by eye.

Emulator: the handler itself is faithful — ops/get_microvm_image_build.rs payload_of echoes the stored field (build_id: build.build_id.clone()) for the build the path addressed. The value it echoes is minted from a single global, zero-padded hexadecimal counter: ComAmazonawsLambdamicrovmsLambdamicrovmsState::next_build_id formats it as 'build-{seq:016x}' and mint_builds inserts each fan-out build under that id, so the first build of every store is always 'build-0000000000000000' and the counter restarts with the store. The replay rejects that value on this read, exactly as it rejects the same minted ids on the builds LISTING of the same fixture.

Why: buildId is the only handle a client has on one build of one image version — it is the path label for this read, the correlation key between a listing row and a build read back, and the value a pipeline persists to re-poll a build later. A client that validates the identifier's form before re-sending it in a URI path, that joins a stored id to a build read back, or that distinguishes two builds created in separate runs gets a value AWS never mints (and, because the counter restarts per store, two different builds share one id across runs) — so the build-tracking branch of the client is unreachable in emulation while it works against AWS.

Fix: the build identifier minted into the build record in the service state (crate::state::next_build_id, the single place that formats it, as consumed by mint_builds) must produce the identifier form AWS records, and must remain the single source for this read, for the listing row projection and for the builds path label so the three cannot disagree. Do not special-case this read to rewrite the id, which would hide the minting form from the other paths.

Related, NOT covering this cluster: open '20261122T120113Z-lambda-microvms-listmicrovmimagebuilds-build-rows-carry-a-minted.md' reports the same minting form but is keyed on lambdamicrovms-customer-image-lifecycle#task#8 (ListMicrovmImageBuilds) only; this replay reports THIS cluster (task#9, GetMicrovmImageBuild $.buildId) with an EMPTY owned_by, so closing that issue does not re-replay this step. '20261110T022430Z-lambda-microvms-getmicrovmimagebuild-does-not-echo-the-addressed.md' named this read but is RESOLVED (status fixed) and so covers nothing — and its concern (echoing the ADDRESSED id verbatim) is confirmed served here; what is left is the minted FORM.

Traces: lambdamicrovms-customer-image-lifecycle#task#9 (GetMicrovmImageBuild, 200 vs 200).

Not claimed by either cause, and a fixer must not try to make them equal: the per-run createdAt instants and the shaped chipsetGeneration values of the build rows (per-run / already-settled recorded values), and the literal '2.0' as a hard-coded constant — Cause 1 requires a resolution, not a second constant.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (trace_set_id simparity-wire-aws-v1, emulator_up true, detail "/__admin/health answered 200 and the :34565 data listener accepts"; fixtures_replayed 13, steps_total 104, steps_passed 97, steps_mismatched 7, steps_errored 0, steps_blocked 0, divergence_count 7, score_pct 93.3, bootstrap_signal false). Expected = recorded AWS response, Actual = emulator response. Both clusters below carry stub_path EMPTY and owned_by EMPTY.

Cause 1 — lambdamicrovms-customer-image-lifecycle#task#7, ListMicrovmImageVersions, expected_status 200 vs actual_status 200, verdict mismatch, affected ['lambdamicrovms-customer-image-lifecycle#task#7'] (affected_count 1):
  detail: "$.items[0].baseImageVersion: value_mismatch (expected '2.0', got '1.0')"
  divergence_key: service=lambda-microvms|ListMicrovmImageVersions|status=200->200|keys=$.items[0].baseImageVersion
  Every other member of the row matches; the row is the version created in the same fixture by CreateMicrovmImage with NO baseImageVersion against the managed base image, which the recording resolves to '2.0' on the create response, the single-version read and this listing row alike.

Cause 2 — lambdamicrovms-customer-image-lifecycle#task#9, GetMicrovmImageBuild, expected_status 200 vs actual_status 200, verdict mismatch, affected ['lambdamicrovms-customer-image-lifecycle#task#9'] (affected_count 1):
  detail: "$.buildId: value_mismatch (expected 'build-0000000000000000', got 'build-0000000000000000')"
  divergence_key: service=lambda-microvms|GetMicrovmImageBuild|status=200->200|keys=$.buildId
  The expected and actual values RENDER identically, so the divergence is in the identifier's form below the rendering (byte length / digit count / encoding) — compare raw bytes, not the printed text. The same minted ids ('build-0000000000000000', 'build-0000000000000001') are rejected on the builds listing of this fixture at task#8.

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json):
- ListMicrovmImageVersions — GET /2025-09-09/microvm-images/{imageIdentifier}/versions, 200, @paginated, @readonly; output items @required MicrovmImageVersionSummaryList, nextToken String. MicrovmImageVersionSummary$baseImageVersion: optional, target Version, @length 1..2048, pattern ^[^\s]+$, doc "The specific version of the base MicroVM image."; $baseImageArn @required NonBlankString doc "The ARN of the base MicroVM image used.". Errors: AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400.
- GetMicrovmImageBuild — GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds/{buildId}, 200, @readonly; input imageIdentifier/imageVersion/buildId all @required httpLabel; output buildId @required NonBlankString @length 1..2048 pattern ^[^\s]+$ doc "The build request ID", plus imageArn, imageVersion, buildState, architecture, chipset, chipsetGeneration @required, stateReason, createdAt @required, snapshotBuild. Same five modeled errors.
- Protocol restJson1; no timestampFormat override on these members.

Code sightings (orientation only, this checkout):
- services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state/regions.rs:251-264 — resolve_base_image_version: returns the own image's latest_active_image_version, else latest_managed_base_image_version_label for a managed reference, else FIRST_IMAGE_VERSION.
- src/state.rs:603-609 — MANAGED_BASE_IMAGE_VERSIONS: generation "1" ships release "1.0", generation "0" ships "0.1"; the doc comment states "The release labels are chosen (this emulator does not hold AWS's own release labels)", so the managed resolution can only ever produce '1.0'.
- src/ops/list_microvm_image_versions.rs:309-337 — payload_of, base_image_version: version.base_image_version.clone() (faithful echo of the stored pedigree).
- src/state.rs:2606-2613 — next_build_id, format!("build-{seq:016x}") off the single global next_build_seq; src/state.rs:2621-2654 — mint_builds, the per-target fan-out inserting each build under that id.
- src/ops/get_microvm_image_build.rs:262-279 — payload_of, build_id: build.build_id.clone() (faithful echo of the minted id).

## Steps to Reproduce
Cause 1 (unpinned base-image pedigree on the version listing):
1. Boot the emulator from this checkout; mvm = client('lambda-microvms').
2. base = ListManagedMicrovmImages()['items'][0]['imageArn']  (the AWS-managed al2023-1 base image).
3. CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=<an IAM role arn>, codeArtifact={'uri': 's3://<unique>/code.zip'}) with NO baseImageVersion.
4. rows = ListMicrovmImageVersions(imageIdentifier=<the new imageArn>)['items'].
   AWS: rows[0]['baseImageVersion'] is the base-image RELEASE resolved for `base` — the label the managed catalogue's newest generation ships (recorded: '2.0') — and it equals what the create response and GetMicrovmImageVersion report.
   Emulator: '1.0' for every unpinned create on any managed base image, which is also FIRST_IMAGE_VERSION, so the row cannot be distinguished from one where nothing was resolved.
5. Cross-check GetMicrovmImageVersion(imageIdentifier=<imageArn>, imageVersion='1.0')['baseImageVersion'] and the create response: all three must report ONE resolved pedigree after the fix (do not hard-code '2.0' — resolve it from the named base image's published releases).
6. Re-replay fixture lambdamicrovms-customer-image-lifecycle step task#7 to confirm: the row must match the recorded baseImageVersion.

Cause 2 (minted build identifier on the single-build read):
7. With the image of step 3, let the asynchronous build settle, then rows = ListMicrovmImageBuilds(imageIdentifier=<imageArn>, imageVersion='1.0')['items'].
8. GetMicrovmImageBuild(imageIdentifier=<imageArn>, imageVersion='1.0', buildId=rows[0]['buildId'])['buildId'].
   AWS: a build request id in the form AWS mints (the corpus records the value as shaped, so assert the FORM — byte length and character class — not a literal), byte-identical to the addressed label and to the listing row's.
   Emulator: 'build-0000000000000000' — a zero-padded hex global counter that restarts with the store. Because the recorded and served values PRINT the same, assert with repr()/len()/bytes rather than by eye.
9. Restart the emulator and repeat steps 3-8: the same 'build-0000000000000000' is minted again, so two distinct builds share one id across runs.
10. Re-replay fixture lambdamicrovms-customer-image-lifecycle step task#9 to confirm the fix. Do NOT attempt to reproduce the recorded createdAt instants or the recorded chipset generations — the corpus does not assert them as contract.
