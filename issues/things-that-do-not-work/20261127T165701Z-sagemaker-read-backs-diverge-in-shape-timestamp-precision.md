Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-pipeline-run-condition-dependency-errors
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-pipeline-run-condition-dependency-errors sagemaker-train-host-and-invoke-xgboost-model sagemaker-model-package-unicode-description-round-trip
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribePipeline|2xx->2xx|$.CreationTime
Issue-Key: sagemaker|DescribePipeline|2xx->2xx|$.LastModifiedTime
Issue-Key: sagemaker|DescribeEndpoint|2xx->2xx|$.CreationTime
Issue-Key: sagemaker|DescribeEndpoint|2xx->2xx|$.LastModifiedTime
Issue-Key: sagemaker|DescribeEndpoint|2xx->2xx|$.ProductionVariants[*].DeployedImages[*].ResolutionTime
Issue-Key: sagemaker|DescribeEndpoint|2xx->2xx|$.ProductionVariants[*].DeployedImages[*].ResolvedImage
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.CreationTime
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.FinalMetricDataList[*].Timestamp
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.LastModifiedTime
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.TrainingEndTime
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.TrainingStartTime
Issue-Key: sagemaker|BatchDescribeModelPackage|2xx->2xx|$.ModelPackageSummaries.arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mpg-72870603-1bcb-4a39-aa1f-efbd59d850c0/1.InferenceSpecification.Containers
Issue-Related: 20261122T164956Z-sagemaker-describetrainingjob-read-back-omits-algorithm-metric-definitions.md
Issue-Commit: 0ee9b03ca15a3996ac8c0c71f0e70a3fe13706ad

# sagemaker read-backs diverge in shape: timestamp precision, training-job ImageMetadata, and an empty Containers list

> _Note: this issue was identified against the code base at commit `0ee9b03ca15a3996ac8c0c71f0e70a3fe13706ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePipeline, DescribeEndpoint, DescribeTrainingJob, BatchDescribeModelPackage

## Description
Three independent shape causes in the emulated SageMaker read-back serializers. Each is numbered and each must be closed before this issue is submitted.

=== Cause 1 of 3: timestamp members are serialized with sub-millisecond fractional precision, so every timestamp in a response fails its shape check ===
AWS: Smithy — SageMaker uses awsJson1.1, where a timestamp member (DescribePipeline.CreationTime/LastModifiedTime, DescribeEndpoint.CreationTime/LastModifiedTime and ProductionVariants[].DeployedImages[].ResolutionTime, DescribeTrainingJob.CreationTime/TrainingStartTime/TrainingEndTime/LastModifiedTime and FinalMetricDataList[].Timestamp — all target Timestamp) is sent as epoch-seconds with AWS's default epoch-seconds rendering, i.e. at most millisecond resolution. The recordings do not assert the instants themselves (they are per-run clock values, recorded as '<shaped-not-asserted>'); they assert the serialized SHAPE, and that shape check fails against the emulator on every timestamp member of five different responses.
Emulator: emits values such as 1795798257.6469588 and 1795798132.0962138 — seven fractional digits, i.e. sub-microsecond precision no AWS JSON timestamp carries. Hypothesis: the serializer writes the full f64 of a system clock instead of rendering the epoch-seconds form AWS uses, which is what the shape check rejects.
Why: clients and recorders that round-trip or compare SageMaker timestamps (and any fixture/test asserting the wire shape of a timestamp) see a number form AWS never produces; re-serialization and digest/signature comparisons over response bodies diverge even when the instant is right.
Fix: the shared JSON timestamp serializer used by the SageMaker read-back handlers (epoch-seconds rendering, millisecond resolution), not the individual handlers.
Traces: sagemaker-pipeline-run-condition-dependency-errors#task#13 (DescribePipeline — the ONLY divergence in that cluster is the two timestamps), sagemaker-train-host-and-invoke-xgboost-model#task#11 (DescribeEndpoint CreationTime/LastModifiedTime/ResolutionTime). The same timestamp mismatches also appear inside the DescribeTrainingJob and DescribePipelineExecution clusters named under the other causes/issues of this sweep.

=== Cause 2 of 3: DescribeTrainingJob's ImageMetadata omits ImageType and instead reports a ResolvedImage the emulator does not own ===
AWS: Smithy — DescribeTrainingJob's AlgorithmSpecification carries the training image metadata (ImageType / ResolvedImage / ResolutionTime). Recorded behavior for a job that ran a SageMaker first-party algorithm image: the response reports ImageType 'SageMaker1PAlgorithm' and NO ResolvedImage member at that point in the job's life; ImageType is the fixed enumerated classification of the image the job requested, not a per-run value.
Emulator: ImageType is missing altogether, and a ResolvedImage is emitted naming an ECR registry host in amazonaws.com — an address outside the emulator that nothing in this deployment can serve (replay flags it host_not_own).
Why: a client that keys on ImageType to tell a 1P-algorithm job from a BYO-container job sees nothing, and one that follows ResolvedImage is pointed at a real-AWS registry host the emulator does not own, which is an egress attempt rather than an emulated read. Related open issue 20261122T164956Z-sagemaker-describetrainingjob-read-back-omits-algorithm-metric-definitions.md owns a different fixture's cluster for this response; this fixture's cluster is unowned and names the ImageType/ResolvedImage pair, so it must be closed here.
Fix: the DescribeTrainingJob read-back serializer/state for image metadata — classify and store ImageType when the job is accepted, and do not synthesize a foreign-registry ResolvedImage where AWS reports none.
Traces: sagemaker-train-host-and-invoke-xgboost-model#task#6.

=== Cause 3 of 3: BatchDescribeModelPackage emits InferenceSpecification.Containers as an empty list where AWS omits the member ===
AWS: Smithy — BatchDescribeModelPackageSummary.InferenceSpecification targets InferenceSpecification, whose Containers member is @required with @length min 1 max 15; an empty list is therefore not a legal value of that member. Recorded behavior: for a model-package version whose summary carries no container list, the response's InferenceSpecification has no Containers key at all.
Emulator: serializes "Containers": [] inside the summary's InferenceSpecification — a value its own model forbids.
Why: a client that treats the presence of Containers as "this version declares inference containers" (the normal reading of a @required, min-1 list) sees an empty list and must special-case it; strict SDK/shape validators reject the body outright because the length constraint is violated.
Fix: the BatchDescribeModelPackage serializer — omit Containers (and the enclosing InferenceSpecification where AWS omits it) rather than emitting an empty list.
Traces: sagemaker-model-package-unicode-description-round-trip#task#4.

## Evidence
Replay of the wire corpus (trace set simparity-wire-aws-v1) against the booted emulator, verbatim from the divergence clusters:

Cause 1 — fixture sagemaker-pipeline-run-condition-dependency-errors, step task#13, DescribePipeline, expected 200 / actual 200 (the whole cluster):
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1795798257.6469588)
  $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1795798257.6469588)
fixture sagemaker-train-host-and-invoke-xgboost-model, step task#11, DescribeEndpoint, expected 200 / actual 200:
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1795798147.4955437)
  $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1795798147.7955437)
  $.ProductionVariants[0].DeployedImages[0].ResolutionTime: value_mismatch (expected '<shaped-not-asserted>', got 1795798147.4680526)
  (expected '<shaped-not-asserted>' means the instant itself is NOT asserted — only the serialized shape is, and it mismatches. The DeployedImages[0].ResolvedImage in this same cluster matched the recorded AWS value exactly and is not reported here as a defect.)

Cause 2 — fixture sagemaker-train-host-and-invoke-xgboost-model, step task#6, DescribeTrainingJob, expected 200 / actual 200:
  $.ImageMetadata.ImageType: missing (expected 'SageMaker1PAlgorithm', got None)
  $.ImageMetadata.ResolvedImage: host_not_own (expected None, got '<account>.dkr.ecr.us-west-2.amazonaws.com/xgboost@sha256:<digest>')
  (plus the cause-1 timestamp mismatches on CreationTime/TrainingStartTime/TrainingEndTime/LastModifiedTime/FinalMetricDataList[0].Timestamp.)

Cause 3 — fixture sagemaker-model-package-unicode-description-round-trip, step task#4, BatchDescribeModelPackage, expected 200 / actual 200:
  $.ModelPackageSummaries.<model-package-arn>.InferenceSpecification.Containers: unexpected_present (expected None, got [])

## Steps to Reproduce
Against the booted emulator with boto3/botocore pointed at it (region us-west-2), inspecting the raw JSON response bodies:

Cause 1: sm.create_pipeline(...) then sm.describe_pipeline(PipelineName=...) and, for a hosted endpoint, sm.describe_endpoint(EndpointName=...). Observe CreationTime/LastModifiedTime/ResolutionTime serialized with seven fractional digits (e.g. 1795798257.6469588) instead of AWS's epoch-seconds rendering (fixtures sagemaker-pipeline-run-condition-dependency-errors step task#13 and sagemaker-train-host-and-invoke-xgboost-model step task#11).

Cause 2: create a training job that uses a SageMaker first-party algorithm image (e.g. the managed xgboost image) and call sm.describe_training_job(TrainingJobName=...). Observe no ImageMetadata.ImageType and an ImageMetadata.ResolvedImage naming an *.dkr.ecr.*.amazonaws.com host instead of ImageType 'SageMaker1PAlgorithm' with no ResolvedImage (fixture sagemaker-train-host-and-invoke-xgboost-model step task#6).

Cause 3: create a model-package group and a model-package version, then sm.batch_describe_model_package(ModelPackageArnList=[<that version's ARN>]). Observe InferenceSpecification: {"Containers": []} in the summary instead of the member being omitted (fixture sagemaker-model-package-unicode-description-round-trip step task#4).
