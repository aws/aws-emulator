Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-model-package-unicode-description-round-trip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-model-package-unicode-description-round-trip
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|BatchDescribeModelPackage|2xx->2xx|$.ModelPackageSummaries.arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mpg-b23957ee-bdd9-454d-ab9c-ec83a3b30441/1.InferenceSpecification.Containers
Issue-Commit: cf865d8505997a7b4c21ad4175685344e53c6578

# SageMaker BatchDescribeModelPackage emits an empty InferenceSpecification.Containers list for a model package registered without one

> _Note: this issue was identified against the code base at commit `cf865d8505997a7b4c21ad4175685344e53c6578`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDescribeModelPackage

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: BatchDescribeModelPackage serializes an empty Containers list inside the per-ARN summary's InferenceSpecification when the model package version was registered with no InferenceSpecification at all.

AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model) — BatchDescribeModelPackage output ModelPackageSummaries is a map of ModelPackageArn -> BatchDescribeModelPackageSummary; BatchDescribeModelPackageSummary.InferenceSpecification targets InferenceSpecification, whose member Containers is @required with @length(min: 1, max: 15) on ModelPackageContainerDefinitionList. An empty list is therefore not a representable value for Containers: AWS never serializes Containers: []. In the recording, the model package was created (CreateModelPackage, whose InferenceSpecification member is optional) without an InferenceSpecification, and the 200 BatchDescribeModelPackage response for that ARN carries no InferenceSpecification.Containers at all — the member is simply absent from the summary. Recorded behavior only for the choice to omit the enclosing InferenceSpecification rather than emit a degenerate one; the model supplies the min-1 constraint that makes [] invalid.

Emulator: for the same ARN the emulator returns HTTP 200 with the summary carrying InferenceSpecification.Containers = [] (replay diff: `$.ModelPackageSummaries.<model-package-arn>.InferenceSpecification.Containers: unexpected_present (expected None, got [])`). It manufactures an empty-shell InferenceSpecification with a zero-length Containers list for a package that has no inference specification, instead of leaving the member unset.

Why: a client that reads the summary cannot distinguish "this version has no inference specification" from "this version has one". SDK/code paths that branch on presence of InferenceSpecification (or that index Containers[0] once the member is present, because the model guarantees at least one container) see a present-but-empty structure and either take the wrong branch or fault on an empty list — behavior that can never occur against AWS, where the member is absent whenever there is no container to report. Strict client-side model validation also rejects the response because Containers violates its @length min 1.

Fix: the BatchDescribeModelPackage handler's response projection / serializer for the ModelPackageSummaries map entry — build InferenceSpecification only when the stored model-package record actually has one, and never emit Containers as an empty list; the same omit-when-empty rule belongs with the stored model-package state's accessor so the other read-backs of the same record agree.

Traces: sagemaker-model-package-unicode-description-round-trip#task#4 (BatchDescribeModelPackage, expected 200, actual 200, body shape diverges).

## Evidence
Fixture sagemaker-model-package-unicode-description-round-trip, step task#4, operation BatchDescribeModelPackage, service sagemaker.

Expected (recorded AWS) status 200; actual status 200. Body divergence reported by replay:
  $.ModelPackageSummaries.arn:aws:sagemaker:<region>:<account>:model-package/<group>/<version>.InferenceSpecification.Containers: unexpected_present (expected None, got [])

Recorded run's concrete key (environment-specific values, not a requirement): $.ModelPackageSummaries.arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mpg-b23957ee-bdd9-454d-ab9c-ec83a3b30441/1.InferenceSpecification.Containers

Expected: the InferenceSpecification/Containers member is absent from the summary for a model package registered without an InferenceSpecification.
Actual: InferenceSpecification.Containers is present and equal to [].

Model evidence: InferenceSpecification$Containers is @required with @length(min: 1, max: 15) — an empty list is outside the modeled range, so no AWS response can contain it.

## Steps to Reproduce
1. Boot the emulator and point a SageMaker client at it.
2. CreateModelPackage for a (versioned) model package group, passing ModelPackageDescription but NO InferenceSpecification — exactly as fixture sagemaker-model-package-unicode-description-round-trip does before task#4.
3. Call BatchDescribeModelPackage with ModelPackageArnList = [the ARN returned in step 2].
4. Inspect the ModelPackageSummaries entry for that ARN: the emulator returns InferenceSpecification.Containers = []. AWS returns the summary with no InferenceSpecification.Containers member at all.
5. Equivalently: replay fixture sagemaker-model-package-unicode-description-round-trip and observe step task#4 mismatch on $.ModelPackageSummaries.<arn>.InferenceSpecification.Containers.
