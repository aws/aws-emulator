Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-algorithm-duplicate-and-missing-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-algorithm-duplicate-and-missing-errors sagemaker-algorithm-lifecycle sagemaker-model-package-create-and-verify sagemaker-model-package-metric-numeric-precision-preserved
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeAlgorithm|2xx->2xx|$.TrainingSpecification.TrainingImageDigest
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.InferenceSpecification.Containers[*].ImageDigest
Issue-Commit: 4c602bb694b43bcf0075caf653c8c8008efcdb0d

# sagemaker: container image digests are replaced by a synthetic digest on read-back instead of echoing the registered value

> _Note: this issue was identified against the code base at commit `4c602bb694b43bcf0075caf653c8c8008efcdb0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlgorithm

## Description
This issue bundles 2 causes with one shared root: SageMaker container-image digest members (`sha256:<64 hex>` values supplied by the client at create time) are NOT persisted and echoed back. Every read-back returns the SAME synthetic digest value regardless of what was registered, so the emulator appears to mint/derive a digest of its own rather than storing the caller's. Both causes are 200-status payload mismatches on a single member.

Cause 1 of 2: DescribeAlgorithm loses TrainingSpecification.TrainingImageDigest (fixture sagemaker-algorithm-duplicate-and-missing-errors, step task#4).
AWS: Smithy sagemaker#TrainingSpecification member TrainingImageDigest -> ImageDigest, optional, @length max 72, @pattern ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$, documented as the hash identifying the Docker image used for training. It is a caller-supplied CreateAlgorithm input with no service-side derivation, so DescribeAlgorithm must return exactly the digest string that CreateAlgorithm recorded for that algorithm; the recording confirms the registered digest is returned verbatim (HTTP 200).
Emulator: returns HTTP 200 with TrainingSpecification.TrainingImageDigest set to a fixed value unrelated to the request ('sha256:a9a77d475defd1d437e0fe8ffb39a703f0c3c7f5a27a504eba82f2a5926b166a') where the registered digest was expected ('sha256:8ebddbc1bfbd56c897023d25d19aefde6606d876ddc5abe295d0628f3144aa9e'). The same constant appears in Cause 2, so it is not request-derived. Hypothesis: the create handler drops the member and the describe serializer fills it from a placeholder/default.
Why: a client that registers an algorithm pinned to a specific training image digest and then reads it back to verify or to pin a downstream training job gets a digest that was never registered. Digest pinning exists precisely to make image identity verifiable, so verification code compares the two strings and fails, or worse pins a training job to an image that does not exist.
Fix: CreateAlgorithm's handler/state must persist TrainingSpecification.TrainingImageDigest as given, and DescribeAlgorithm's serializer must emit the stored value (omitting the member when none was supplied) rather than a generated or constant digest.
Traces: sagemaker-algorithm-duplicate-and-missing-errors#task#4, sagemaker-algorithm-lifecycle#task#3.

Cause 2 of 2: DescribeModelPackage loses InferenceSpecification.Containers[].ImageDigest (fixture sagemaker-model-package-create-and-verify, step task#3).
AWS: Smithy sagemaker#ModelPackageContainerDefinition member ImageDigest -> ImageDigest, optional, @length max 72, @pattern ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$. It is caller-supplied on CreateModelPackage (InferenceSpecification.Containers), so DescribeModelPackage must echo, per container, the digest that was registered for it; the recording shows the registered digest returned verbatim (HTTP 200).
Emulator: returns HTTP 200 with InferenceSpecification.Containers[0].ImageDigest set to the same fixed 'sha256:a9a77d475defd1d437e0fe8ffb39a703f0c3c7f5a27a504eba82f2a5926b166a' instead of the registered 'sha256:8ebddbc1bfbd56c897023d25d19aefde6606d876ddc5abe295d0628f3144aa9e'.
Why: model-package consumers use the container digest to decide whether a package points at an already-approved, immutable image before deploying it to an endpoint. A substituted digest makes approval/promotion gates compare against a value the registry never contained, so a correct package looks tampered with and an image-identity check cannot be implemented against the emulator.
Fix: CreateModelPackage's handler/state must persist each container's ImageDigest, and DescribeModelPackage's serializer must emit the per-container stored value (member absent when not supplied) rather than a shared constant. Both causes point at one place: the shared container/image-spec round-trip in the sagemaker model's state and serializers.
Traces: sagemaker-model-package-create-and-verify#task#3, sagemaker-model-package-metric-numeric-precision-preserved#task#4.

No open issue covers these two members; the nearest open sagemaker read-back issues (20261120T143955Z-sagemaker-training-job-and-endpoint-read-backs-lose.md and 20261122T164956Z-sagemaker-describetrainingjob-read-back-omits-algorithm-metric-definitions.md) own DescribeTrainingJob/DescribeEndpoint clusters only and do not carry these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 85.9%, 10 diverged steps):

Cause 1 - service=sagemaker|DescribeAlgorithm|status=200->200: expected 200, got 200; $.TrainingSpecification.TrainingImageDigest: value_mismatch (expected 'sha256:8ebddbc1bfbd56c897023d25d19aefde6606d876ddc5abe295d0628f3144aa9e', got 'sha256:a9a77d475defd1d437e0fe8ffb39a703f0c3c7f5a27a504eba82f2a5926b166a'). Cluster members: sagemaker-algorithm-duplicate-and-missing-errors#task#4, sagemaker-algorithm-lifecycle#task#3 (affected_count 2).

Cause 2 - service=sagemaker|DescribeModelPackage|status=200->200: expected 200, got 200; $.InferenceSpecification.Containers[0].ImageDigest: value_mismatch (expected 'sha256:8ebddbc1bfbd56c897023d25d19aefde6606d876ddc5abe295d0628f3144aa9e', got 'sha256:a9a77d475defd1d437e0fe8ffb39a703f0c3c7f5a27a504eba82f2a5926b166a'). Cluster members: sagemaker-model-package-create-and-verify#task#3, sagemaker-model-package-metric-numeric-precision-preserved#task#4 (affected_count 2).

The identical 'got' value across two different operations and four fixtures is the evidence that the returned digest is synthetic rather than the registered one. The specific hex strings above are one recording's values and are not themselves requirements: the requirement is that the digest read back equals the digest supplied at create time, matching ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture sagemaker-algorithm-duplicate-and-missing-errors, step task#4 - CreateAlgorithm with TrainingSpecification.TrainingImageDigest set to a sha256:<64 hex> value, then DescribeAlgorithm the same algorithm name and compare $.TrainingSpecification.TrainingImageDigest with the value sent. (Also reproduced by sagemaker-algorithm-lifecycle#task#3.)
3. Cause 2: replay fixture sagemaker-model-package-create-and-verify, step task#3 - CreateModelPackage with InferenceSpecification.Containers[0].ImageDigest set to a sha256:<64 hex> value, then DescribeModelPackage and compare $.InferenceSpecification.Containers[0].ImageDigest with the value sent. (Also reproduced by sagemaker-model-package-metric-numeric-precision-preserved#task#4.)
4. Both read-backs return 200 with the same constant digest instead of the registered one.
