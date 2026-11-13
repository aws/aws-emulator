Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-build-batch-environment-variables-preserved
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-build-batch-environment-variables-preserved codebuild-build-and-batch-registry-reads codebuild-project-registry-and-absent-project-errors codebuild-report-group-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].artifacts.location
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].artifacts.type
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].buildBatchConfig.restrictions
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].buildBatchConfig.timeoutInMins
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].buildBatchStatus
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].buildGroups
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].currentPhase
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*]
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*].durationInSeconds
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*].endTime
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*].phaseStatus
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*].phaseType
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].queuedTimeoutInMinutes
Issue-Key: codebuild|BatchDeleteBuilds|2xx->2xx|$.buildsDeleted[*]
Issue-Key: codebuild|DeleteProject|2xx->2xx|$
Issue-Key: codebuild|GetResourcePolicy|2xx->2xx|$.policy
Issue-Related: 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md
Issue-Commit: 6f542d1dda0a64c4c626f1148e416fc4d064c349

# CodeBuild response bodies omit modeled members and lose stored state (4 causes)

> _Note: this issue was identified against the code base at commit `6f542d1dda0a64c4c626f1148e416fc4d064c349`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetBuildBatches

## Description
FOUR net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006): successful (HTTP 200) responses whose BODY diverges — modeled members are absent, a stored document is not returned as stored, and a resource the client created is reported as unknown. ALL FOUR must be closed before this issue is resolved.

Cause 1 of 4: a build batch never reaches a terminal success, and BatchGetBuildBatches omits most of the BuildBatch structure.
AWS: Smithy BatchGetBuildBatches(ids @required BuildBatchIds) -> {buildBatches, buildBatchesNotFound}. A batch whose builds all succeed settles with buildBatchStatus SUCCEEDED and currentPhase SUCCEEDED, and the returned BuildBatch carries artifacts ({type 'no_artifacts', location ''} for a no-artifacts project), buildBatchConfig (the project's resolved batch config, e.g. timeoutInMins and restrictions.maximumBuildsAllowed), queuedTimeoutInMinutes, a buildGroups list with one entry per batch build graph node (identifier, ignoreFailure, dependsOn, and currentBuildSummary with arn/buildStatus/primaryArtifact/secondaryArtifacts/requestedOn), and a full phase walk ending in COMBINE_ARTIFACTS/SUCCEEDED and a final phase with each phaseStatus SUCCEEDED and non-zero durationInSeconds for the phases that did work.
Emulator: HTTP 200 but buildBatchStatus 'FAULT', currentPhase 'FAILED', the phase list truncated with phaseStatus FAULT on the second phase (durationInSeconds 0) and a 'FAILED' phaseType where AWS has IN_PROGRESS then the SUCCEEDED tail, and artifacts, buildBatchConfig (restrictions, timeoutInMins), queuedTimeoutInMinutes and buildGroups entirely absent.
Why: a client cannot observe a successful batch at all — batch orchestration, per-group build discovery (buildGroups is the only way to reach the batch's child builds) and batch-config round-tripping are all unavailable, while the control plane claims the batch completed.
Fix: the build-batch state machine behind StartBuildBatch and the BuildBatch projection read back by the BatchGetBuildBatches handler — the batch must walk its groups to a terminal SUCCEEDED and the projection must serialize artifacts/buildBatchConfig/queuedTimeoutInMinutes/buildGroups from the project's resolved configuration. Related open issue 20261025T062553Z-codebuild-6-further-divergences-startbuildbatch-takes-no-input.md names batch builds never reaching a terminal outcome but carries no wire replay key for this step, so this fixture must be replayed too.
Traces: codebuild-build-batch-environment-variables-preserved#task#7.

Cause 2 of 4: BatchDeleteBuilds cannot find a build the service itself started, and echoes the id form instead of the ARN.
AWS: Smithy BatchDeleteBuilds(ids @required BuildIds @length 1-100) -> {buildsDeleted (BuildIds), buildsNotDeleted}. Deleting an existing build of this account puts the build's FULL ARN ('arn:aws:codebuild:<region>:<account>:build/<project>:<uuid>') in buildsDeleted and leaves buildsNotDeleted empty.
Emulator: HTTP 200 with buildsDeleted absent and the build reported in buildsNotDeleted as {id: '<project>:<uuid>', statusCode: 'Build <project>:<uuid> does not exist'} — the build registry does not resolve a build that was started earlier in the same fixture, and the identifier is serialized in bare '<project>:<uuid>' form rather than as the ARN AWS returns.
Why: build teardown is impossible and reported as a per-item failure, so cleanup code loops or aborts; and the identifier format difference breaks any client that feeds buildsDeleted back into an ARN-keyed inventory.
Fix: the build lookup and the identifier serialization in the BatchDeleteBuilds handler (resolve against the same build state StartBuild writes; emit ARNs in buildsDeleted/buildsNotDeleted as the model's BuildIds are recorded).
Traces: codebuild-build-and-batch-registry-reads#task#5.

Cause 3 of 4: DeleteProject returns an empty body where AWS reports the webhook disposition.
AWS: recorded behavior only — Smithy declares DeleteProjectOutput with no members, but the recorded 200 response body is {'webhookDeletedStatus': '<status>'}, with the value 'no_webhook' for a project that had no webhook attached (the variable part is the disposition computed from the project's webhook state; the member name is fixed).
Emulator: HTTP 200 with an empty JSON object {}.
Why: a client that deletes a project and inspects the webhook disposition to decide whether it must also clean up a provider-side hook gets no answer, and a response-shape comparison against AWS fails on an otherwise successful delete.
Fix: the DeleteProject handler's response serializer — emit webhookDeletedStatus from the deleted project's webhook state.
Traces: codebuild-project-registry-and-absent-project-errors#task#13.

Cause 4 of 4: GetResourcePolicy returns a re-serialized policy instead of the document that was stored.
AWS: Smithy PutResourcePolicy(policy @required NonEmptyString, resourceArn @required) / GetResourcePolicy(resourceArn @required) -> policy (NonEmptyString @length min 1). The policy is an opaque STRING: GetResourcePolicy returns byte-for-byte what PutResourcePolicy was given — same key order, no inserted whitespace, scalar members left scalar ('Action': '<action>', 'Principal': {'AWS': '<principal ARN>'}).
Emulator: HTTP 200 whose $.policy is the document parsed and re-emitted: a space inserted after every ':' and ',', the scalar Action rewritten as a single-element list, and Principal.AWS rewritten from the submitted principal ARN ('arn:aws:iam::<account>:root') to a single-element list holding the bare account id.
Why: the policy is not a round-trippable document — a client that reads, edits and re-puts it silently rewrites the principal (an IAM root ARN becomes an account id) and any signature, hash or exact-match comparison over the policy text fails.
Fix: the resource-policy state and the GetResourcePolicy handler must store and return the policy string verbatim rather than decoding to a policy model and re-serializing. Open issue 20261025T014438Z-codebuild-8-further-divergences-getresourcepolicy-updatewebhook-deletebuildbatch-stubs.md records GetResourcePolicy as a NotImplemented stub; it now answers 200 with a rewritten document and that issue carries no wire replay key for this step.
Traces: codebuild-report-group-resource-policy-lifecycle#task#4 (same cluster also covers task#6 of that fixture).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (106 steps, 71 passed, 35 mismatched, score 67%). All four causes are status-200 body divergences.

Cause 1 — codebuild-build-batch-environment-variables-preserved#task#7 BatchGetBuildBatches, 200/200: $.buildBatches[0].buildBatchStatus expected 'SUCCEEDED', got 'FAULT'; $.buildBatches[0].currentPhase expected 'SUCCEEDED', got 'FAILED'; $.buildBatches[0].buildGroups missing (expected two entries with identifiers 'DOWNLOAD_SOURCE' and 'build1', each with currentBuildSummary{arn,buildStatus 'SUCCEEDED',primaryArtifact{identifier,type 'no_artifacts'},secondaryArtifacts,requestedOn}); $.buildBatches[0].artifacts.type missing (expected 'no_artifacts'), artifacts.location missing (expected ''); $.buildBatches[0].buildBatchConfig.restrictions missing (expected {maximumBuildsAllowed: 100}) and .timeoutInMins missing (expected 2160); $.buildBatches[0].queuedTimeoutInMinutes missing; $.buildBatches[0].phases[1].phaseStatus expected 'SUCCEEDED', got 'FAULT' with durationInSeconds 0 instead of a non-zero duration; phases[2].phaseType expected 'IN_PROGRESS', got 'FAILED', and phases[2..4] (phaseStatus SUCCEEDED, COMBINE_ARTIFACTS, SUCCEEDED tail) missing.
Cause 2 — codebuild-build-and-batch-registry-reads#task#5 BatchDeleteBuilds, 200/200: $.buildsDeleted[0] missing (expected 'arn:aws:codebuild:<region>:<account>:build/<project>:<uuid>'); $.buildsNotDeleted[0] extra, got {'id': '<project>:<uuid>', 'statusCode': 'Build <project>:<uuid> does not exist'}.
Cause 3 — codebuild-project-registry-and-absent-project-errors#task#13 DeleteProject, 200/200: $ empty_body — expected {'webhookDeletedStatus': 'no_webhook'}, got {}.
Cause 4 — codebuild-report-group-resource-policy-lifecycle#task#4 (and #task#6) GetResourcePolicy, 200/200: $.policy expected '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"codebuild:BatchGetReportGroups","Resource":"<report-group ARN>"}]}', got '{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"AWS": ["<account>"]}, "Action": ["codebuild:BatchGetReportGroups"], "Resource": "<report-group ARN>"}]}' — i.e. inserted whitespace, Action and Principal.AWS promoted to lists, and the root ARN reduced to the bare account id.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — BatchGetBuildBatches (ids @required -> buildBatches/buildBatchesNotFound), BatchDeleteBuilds (ids @required BuildIds @length 1-100 -> buildsDeleted/buildsNotDeleted), DeleteProject (name @required; DeleteProjectOutput declares no members, hence 'recorded behavior only' for webhookDeletedStatus), GetResourcePolicy/PutResourcePolicy (policy and resourceArn as NonEmptyString, policy returned as an opaque string).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus) and read the codebuild divergences, or reproduce each cause directly:
3. Cause 1: CreateProject with NO_SOURCE / no artifacts and a buildspec declaring a batch build-graph (DOWNLOAD_SOURCE plus one build), StartBuildBatch, poll BatchGetBuildBatches(ids=[<batch id>]) until complete — expect buildBatchStatus SUCCEEDED with buildGroups, artifacts, buildBatchConfig and queuedTimeoutInMinutes populated; observe FAULT/FAILED with those members absent.
4. Cause 2: StartBuild on an existing project, then BatchDeleteBuilds(ids=['<project>:<uuid of that build>']) — expect the build's ARN in buildsDeleted; observe it in buildsNotDeleted with statusCode 'Build ... does not exist'.
5. Cause 3: CreateProject then DeleteProject(name=<project with no webhook>) — expect body {'webhookDeletedStatus': 'no_webhook'}; observe {}.
6. Cause 4: CreateReportGroup, PutResourcePolicy(resourceArn=<its ARN>, policy='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"codebuild:BatchGetReportGroups","Resource":"<arn>"}]}'), then GetResourcePolicy(resourceArn=<its ARN>) — expect the identical string back; observe the re-serialized document.
