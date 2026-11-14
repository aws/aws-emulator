Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-list-pipelines-membership-and-pagination
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-list-pipelines-membership-and-pagination
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-c774cede"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-c774cede"].updated
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-4e689c79"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-4e689c79"].updated
Issue-Commit: 36cbc41e18f8f7f03eaee998bf950ce282c69b6c

# CodePipeline ListPipelines serializes PipelineSummary created/updated with sub-millisecond precision

> _Note: this issue was identified against the code base at commit `36cbc41e18f8f7f03eaee998bf950ce282c69b6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelines

## Description
One cause. ListPipelines emits the PipelineSummary timestamps with the native precision of the emulator's clock instead of the wire form CodePipeline serializes.

Cause 1 of 1: ListPipelines PipelineSummary.created / PipelineSummary.updated carry more fractional precision than a CodePipeline timestamp (fixture codepipeline-list-pipelines-membership-and-pagination, step task#8).
AWS: Smithy ListPipelines -> ListPipelinesOutput.pipelines : PipelineList of PipelineSummary {name, version, pipelineType (PipelineType enum), executionMode (ExecutionMode enum), created : Timestamp, updated : Timestamp}; ListPipelines is paginated (inputToken/outputToken nextToken, items pipelines, pageSize maxResults) and models InvalidNextTokenException and ValidationException (both 400). CodePipeline speaks awsJson1_1, whose default timestamp form is epoch-seconds as a JSON number. Recorded behavior only for the precision: every timestamp in this corpus's recorded CodePipeline responses carries at most three fractional digits (millisecond resolution, e.g. a recorded ListActionExecutions lastUpdateTime of 1793700369.272). Millisecond resolution is therefore the wire form; the instant itself is a per-run value and is not a requirement, which is why the recording asserts the shape of created/updated and not their value.
Emulator: the step replays with status 200 (expected 200) but both summaries fail the shape assertion on their timestamps: $.pipelines[name="...-lp-a-..."].created and .updated and $.pipelines[name="...-lp-b-..."].created and .updated come back as epoch-seconds numbers with seven fractional digits (observed 1794638927.6545506 and 1794638927.6775227 in this run; those instants and the generated pipeline names are per-run values, not requirements). Hypothesis: the float seconds-since-epoch of the internal clock is written straight into the JSON body instead of being truncated/rounded to milliseconds first.
Why: a client that compares, digests or round-trips these numbers against AWS-produced values, or that parses them with a millisecond-resolution timestamp parser, sees a numeric form AWS never emits. Equality and fingerprint comparisons over ListPipelines responses fail even when the instant is irrelevant, and a consumer that re-sends the value as a millisecond epoch loses or corrupts the sub-millisecond tail.
Fix: the codepipeline response serializer's epoch-seconds timestamp encoding (shared by the PipelineSummary projection used by the ListPipelines handler) must emit at most millisecond resolution rather than the clock's native precision.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8 (ListPipelines).

Related open issue: 20261111T061123Z-codepipeline-listpipelines-reports-pipelinetype-v1-and-timestamps-carry.md describes the same serializer fault, but its ListPipelines replay keys are bound to the generated pipeline names of an earlier run, so the replay reports this cluster as unowned and its pointer does not re-run this step. Its PutApprovalResult approvedAt cluster IS still owned by it and is not re-filed here. The pipelineType V1-vs-V2 half of that issue no longer diverges on this step; only created/updated do.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, score_pct 93.3, 14 divergence clusters).

Cluster: codepipeline-list-pipelines-membership-and-pagination#task#8, operation ListPipelines, verdict mismatch, expected status 200, actual status 200, owned_by [] (unowned), stub_path empty, affected_count 1.
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-c774cede"].created: value_mismatch (expected '<shaped-not-asserted>', got 1794638927.6545506)
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-c774cede"].updated: value_mismatch (expected '<shaped-not-asserted>', got 1794638927.6545506)
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-4e689c79"].created: value_mismatch (expected '<shaped-not-asserted>', got 1794638927.6775227)
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-4e689c79"].updated: value_mismatch (expected '<shaped-not-asserted>', got 1794638927.6775227)

Expected (wire form): an epoch-seconds JSON number with at most three fractional digits, as in every recorded CodePipeline timestamp in this corpus (e.g. 1793700369.272). Actual: seven fractional digits. The pipeline names (sim2real-probe-codepipeline-lp-a-c774cede, ...-lp-b-4e689c79) and the instants themselves are per-run values; the requirement under test is the serialized precision of the number.

Corroborating steps for the same serializer, already owned by open issues for their own faults: codepipeline-s3-source-manual-approval-run-succeeds#task#9 ($.approvedAt got 1794638809.2453322) and #task#12 ($.created / $.updated got 1794638804.2086265, latestExecution.lastStatusChange got 1794638804.5272076).

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json — ListPipelines (output.pipelines: PipelineList; paginated nextToken/maxResults; errors InvalidNextTokenException, ValidationException) and shape PipelineSummary (created: Timestamp 'The date and time the pipeline was created, in timestamp format', updated: Timestamp).

## Steps to Reproduce
1. Build and boot the emulator from this checkout (health: /__admin/health returns 200).
2. Replay fixture codepipeline-list-pipelines-membership-and-pagination against it.
3. At step task#8, call ListPipelines after the fixture has created its two pipelines with CreatePipeline.
4. Read every PipelineSummary's created and updated: each is an epoch-seconds JSON number with six or seven fractional digits (e.g. 1794638927.6545506).
5. Compare with the recorded CodePipeline responses in this corpus, where every timestamp has at most three fractional digits (e.g. 1793700369.272). The step replays 200/200 but fails the timestamp shape assertion on all four keys.
