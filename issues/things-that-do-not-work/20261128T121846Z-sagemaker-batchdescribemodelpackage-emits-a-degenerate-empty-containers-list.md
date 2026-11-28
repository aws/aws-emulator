Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-model-package-unicode-description-round-trip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-model-package-unicode-description-round-trip
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|BatchDescribeModelPackage|2xx->2xx|$.ModelPackageSummaries.arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mpg-c7c30814-b7e9-4f42-b3fc-9164032378eb/1.InferenceSpecification.Containers
Issue-Commit: 96996ff548d771d992d512709b10ef647dc44007

# sagemaker BatchDescribeModelPackage emits a degenerate empty Containers list in the per-ARN summary

> _Note: this issue was identified against the code base at commit `96996ff548d771d992d512709b10ef647dc44007`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDescribeModelPackage

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: BatchDescribeModelPackage manufactures an empty-shell InferenceSpecification whose Containers list is zero-length, for a model package version that was registered without an InferenceSpecification. Fixture sagemaker-model-package-unicode-description-round-trip, step task#4.

AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json) — BatchDescribeModelPackage output member ModelPackageSummaries targets ModelPackageSummaries, a map of ModelPackageArn -> BatchDescribeModelPackageSummary; BatchDescribeModelPackageSummary.InferenceSpecification targets InferenceSpecification; InferenceSpecification.Containers is @required and targets ModelPackageContainerDefinitionList with @length(min: 1, max: 15). An empty list is therefore outside the modeled range for Containers, so no AWS response can carry Containers: []. In the recording the package was registered with no InferenceSpecification (CreateModelPackage.InferenceSpecification is optional) and the 200 BatchDescribeModelPackage response for that ARN carries no InferenceSpecification.Containers member at all. Recorded behavior only for the choice to omit the enclosing InferenceSpecification entirely rather than emit a degenerate one; the model supplies the min-1 constraint that makes [] unrepresentable. Status and body are a success (200); no error code or message is involved in this cause.

Emulator: for the same ARN the emulator answers 200 but the summary carries InferenceSpecification.Containers = []. Replay diff: `$.ModelPackageSummaries.<model-package-arn>.InferenceSpecification.Containers: unexpected_present (expected None, got [])`. The ARN, group name and version in the recorded key are per-run values and are not part of the requirement; the requirement is that the member is absent whenever the stored record has no inference specification.

Why: a client cannot distinguish "this version has no inference specification" from "this version has one". Code that branches on the presence of InferenceSpecification takes the has-one branch, and code that then reads Containers[0] — safe against AWS, because the model guarantees at least one container whenever the member is present — faults or silently deploys nothing. Strict client-side model validation rejects the response outright, since Containers violates its @length min 1.

Fix: the BatchDescribeModelPackage handler's response projection/serializer for each ModelPackageSummaries map entry — build InferenceSpecification only when the stored model-package record actually holds one, and never serialize Containers as a zero-length list. The omit-when-empty rule belongs on the stored model-package state's accessor so the other read-backs of the same record (DescribeModelPackage, ListModelPackages) agree.

Related open issues that do NOT cover this replay step: "SageMaker BatchDescribeModelPackage emits an empty InferenceSpecification.Containers list for a model package registered without one" (20261127T200716Z-sagemaker-batchdescribemodelpackage-emits-an-empty-inferencespecification-containers-list.md) describes the same fault but its Issue-Key pins a previous run's concrete model-package ARN, so this replay reports the cluster as unowned; and "sagemaker read-backs diverge in shape: timestamp precision, training-job ImageMetadata, and an empty Containers list" (20261127T165701Z-sagemaker-read-backs-diverge-in-shape-timestamp-precision.md) bundles other read-back shape causes and does not carry this step's replay key. A fixer should close this step together with those.

Traces: sagemaker-model-package-unicode-description-round-trip#task#4 (BatchDescribeModelPackage, expected 200, actual 200, body shape diverges).

## Evidence
Fixture sagemaker-model-package-unicode-description-round-trip, step task#4, operation BatchDescribeModelPackage, service sagemaker, verdict mismatch.

Expected (recorded AWS) status 200; actual status 200.

Body divergence reported by replay (generalized):
  $.ModelPackageSummaries.arn:aws:sagemaker:<region>:<account>:model-package/<group>/<version>.InferenceSpecification.Containers: unexpected_present (expected None, got [])

This run's concrete key (per-run values, not a requirement):
  $.ModelPackageSummaries.arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mpg-c7c30814-b7e9-4f42-b3fc-9164032378eb/1.InferenceSpecification.Containers

Expected: no InferenceSpecification.Containers member in the summary for a model package registered without an InferenceSpecification.
Actual: InferenceSpecification.Containers present and equal to [].

Model evidence: InferenceSpecification$Containers is @required with @length(min: 1, max: 15) on ModelPackageContainerDefinitionList, so [] is not a value AWS can serialize.

## Steps to Reproduce
1. Boot the emulator and point a SageMaker client at its endpoint.
2. CreateModelPackage for a versioned model package group, passing ModelPackageDescription but NO InferenceSpecification — as fixture sagemaker-model-package-unicode-description-round-trip does before task#4.
3. Call BatchDescribeModelPackage with ModelPackageArnList = [the ModelPackageArn returned in step 2].
4. Inspect the ModelPackageSummaries entry for that ARN: the emulator returns InferenceSpecification.Containers = []; AWS returns the summary with no InferenceSpecification.Containers member.
5. Equivalently: replay fixture sagemaker-model-package-unicode-description-round-trip and observe step task#4 mismatch on $.ModelPackageSummaries.<arn>.InferenceSpecification.Containers.
