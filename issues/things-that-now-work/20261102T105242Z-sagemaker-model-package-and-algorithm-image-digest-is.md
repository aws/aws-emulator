Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-algorithm-duplicate-and-missing-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-algorithm-duplicate-and-missing-errors sagemaker-model-package-create-and-verify sagemaker-model-package-metric-numeric-precision-preserved sagemaker-model-package-unicode-description-round-trip
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeAlgorithm|2xx->2xx|$.TrainingSpecification.TrainingImageDigest
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.CreatedBy.IamIdentity.Arn
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.InferenceSpecification.Containers[*].ImageDigest
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.ModelPackageStatus
Issue-Key: sagemaker|BatchDescribeModelPackage|2xx->2xx|$.ModelPackageSummaries.arn
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# SageMaker model-package and algorithm image digest is regenerated, and ModelPackageStatus never settles

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlgorithm

## Description
Four net-new wire divergences on the algorithm / model-package read path. All answer 200, and all are the same two faults: the container image digest submitted at create is replaced by an emulator-derived hash instead of being echoed, and a created model package never leaves 'Pending'. Related open issues, neither of which owns these clusters (the replay reports them unowned): 'SageMaker lifecycle, identity and round-trip fidelity defects across ten operations' (20261029T192819Z-sagemaker-lifecycle-identity-and-round-trip-fidelity-defects.md), whose cause 4 is the same shape of stuck-status defect for DescribeHub, and 'SageMaker control-plane gaps: ... BatchDescribeModelPackage mis-codes a missing ARN' (20261024T160943Z-sagemaker-control-plane-gaps-createuserprofile-and-deleteaibenchmarkjob-are.md). Note: two of these clusters also report $.CreatedBy.IamIdentity.Arn differing only in the emulator's per-run generated principal name; that is a generated value, not a requirement, and is deliberately NOT a cause here.

Cause 1 of 4: DescribeAlgorithm does not echo the submitted TrainingSpecification.TrainingImageDigest.
AWS: TrainingImageDigest is an opaque identifier of the training image that the caller supplies to CreateAlgorithm and that AWS stores and returns unchanged. Smithy: TrainingSpecification.TrainingImageDigest -> ImageDigest, @length 0..72, @pattern ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$ - the model constrains its form but AWS never recomputes it, so round-tripping the submitted value is recorded behavior.
Emulator: returns a different sha256 digest of its own - correctly shaped, but not the value the create request carried, and stable across fixtures, i.e. derived locally from the image URI rather than read from stored state.
Why: the digest is how a client pins the exact image an algorithm was registered against; a caller that compares the described digest to the one it submitted (or to an ECR manifest digest) concludes the registration is wrong and refuses to deploy.
Fix: CreateAlgorithm must persist TrainingSpecification.TrainingImageDigest as given and DescribeAlgorithm's output projection must return the stored value, never a computed one.
Traces: sagemaker-algorithm-duplicate-and-missing-errors#task#4.

Cause 2 of 4: DescribeModelPackage does not echo the submitted InferenceSpecification.Containers[].ImageDigest - the same fault in the model-package state.
AWS: ModelPackageContainerDefinition.ImageDigest is stored as supplied and returned unchanged. Smithy: ModelPackageContainerDefinition.ImageDigest -> ImageDigest, @length 0..72, @pattern ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$.
Emulator: each described container carries an emulator-derived digest instead of the one CreateModelPackage was given.
Why: same as cause 1 - the pinned inference image cannot be verified.
Fix: CreateModelPackage container persistence plus the DescribeModelPackage output projection.
Traces: sagemaker-model-package-create-and-verify#task#3.

Cause 3 of 4: DescribeModelPackage reports ModelPackageStatus 'Pending' indefinitely.
AWS: a model package created without validation work settles to 'Completed'; Smithy ModelPackageStatus enum values are Pending, InProgress, Completed, Failed, Deleting, and a successfully created package reaches Completed.
Emulator: the status stays 'Pending' for the life of the resource (this cluster also shows the cause-2 digest fault).
Why: a client that waits for Completed before approving or deploying the package polls forever; one that treats non-Completed as failure rejects a package AWS considers good.
Fix: the model-package status machine behind CreateModelPackage / DescribeModelPackage must advance the stored status to Completed once creation finishes, instead of leaving the initial Pending value in state.
Traces: sagemaker-model-package-metric-numeric-precision-preserved#task#4.

Cause 4 of 4: BatchDescribeModelPackage reports the same stuck 'Pending' in its summaries.
AWS: each ModelPackageSummaries entry carries the package's settled ModelPackageStatus, i.e. 'Completed' for a created package.
Emulator: the summary for the package reports 'Pending', confirming the defect is in the shared stored status and not in one projection.
Why: a batch poller never observes the package become ready.
Fix: the same model-package status machine; BatchDescribeModelPackage reads the corrected stored status.
Traces: sagemaker-model-package-unicode-description-round-trip#task#3.

## Evidence
Cause 1 - fixture sagemaker-algorithm-duplicate-and-missing-errors step task#4, DescribeAlgorithm, 200 expected and 200 observed: $.TrainingSpecification.TrainingImageDigest value_mismatch - expected the digest supplied at create ('sha256:8ebddbc1...aa9e' in this recording), got an emulator-derived digest ('sha256:a9a77d47...166a'). The observed value is the same in every fixture, so it is derived from the image URI rather than per-run.

Cause 2 - fixture sagemaker-model-package-create-and-verify step task#3, DescribeModelPackage, 200/200: $.InferenceSpecification.Containers[0].ImageDigest value_mismatch, expected the submitted digest, got the same emulator-derived digest as cause 1. (The cluster also reports $.CreatedBy.IamIdentity.Arn differing only by the emulator's generated principal name - excluded as a generated value.)

Cause 3 - fixture sagemaker-model-package-metric-numeric-precision-preserved step task#4, DescribeModelPackage, 200/200: $.ModelPackageStatus value_mismatch (expected 'Completed', got 'Pending'); plus the cause-2 ImageDigest mismatch, plus the excluded generated CreatedBy ARN.

Cause 4 - fixture sagemaker-model-package-unicode-description-round-trip step task#3, BatchDescribeModelPackage, 200/200: $.ModelPackageSummaries.<model-package-arn>.ModelPackageStatus value_mismatch (expected 'Completed', got 'Pending').

Smithy consulted: TrainingSpecification.TrainingImageDigest and ModelPackageContainerDefinition.ImageDigest both target ImageDigest with @length 0..72 and @pattern ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$; ModelPackageStatus enum = [Pending, InProgress, Completed, Failed, Deleting].

## Steps to Reproduce
Replay the wire corpus against the emulator. Representative: fixture sagemaker-algorithm-duplicate-and-missing-errors, step task#4 - CreateAlgorithm with TrainingSpecification.TrainingImageDigest set, then DescribeAlgorithm and compare the returned digest to the submitted one. Cause 2: fixture sagemaker-model-package-create-and-verify, step task#3 - CreateModelPackage with InferenceSpecification.Containers[0].ImageDigest, then DescribeModelPackage. Cause 3: fixture sagemaker-model-package-metric-numeric-precision-preserved, step task#4 - create a model package, then DescribeModelPackage and read ModelPackageStatus. Cause 4: fixture sagemaker-model-package-unicode-description-round-trip, step task#3 - BatchDescribeModelPackage on that package ARN and read ModelPackageSummaries[...].ModelPackageStatus.
