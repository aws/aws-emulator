Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.ingest-endpoint-unsigned-post-rejected osis.stop-pipeline-already-stopped-rejected
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis.ingest-endpoint-unsigned-post-rejected|run#11|AssertionError|assert re.fullmatch(host_shape, urls[0]), (
Issue-Key: osis.stop-pipeline-already-stopped-rejected|run#28|AssertionError|assert sorted(named) == ["ACTIVE", "UPDATE_FAILED"], f"second StopPipeline answered {got!r}"
Issue-Commit: 429bdfecbf56545571d755c8b6ad852b4e3ed1bc

# osis: GetPipeline publishes IngestEndpointUrls as a loopback host plus an /_aws path instead of a bare pipeline-named ingest host, and StopPipeline's already-stopped ConflictException uses the emulator's own wording instead of AWS's eligible-status message

> _Note: this issue was identified against the code base at commit `429bdfecbf56545571d755c8b6ad852b4e3ed1bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipeline

## Description
Two net-new root causes in the osis (Amazon OpenSearch Ingestion) emulator, both found by behavioral replay. Each is independently fixable and each must be closed before submitting.

--- Cause 1 of 2: GetPipeline publishes Pipeline.IngestEndpointUrls as "<host>:<port>/<path>" instead of a bare, pipeline-named ingest host ---
AWS: Smithy com.amazonaws.osis#Pipeline member IngestEndpointUrls (IngestEndpointUrlsList), documented "The ingestion endpoints for the pipeline, which you can send data to"; returned by GetPipeline (GET /2022-01-01/osis/getPipeline/{PipelineName}, 200) and CreatePipeline. The model places no constraint on the value's shape, so this is recorded behavior only for the shape: for an ACTIVE pipeline with a public (non-VPC) http source, AWS publishes EXACTLY ONE entry and it is a BARE HOST with no scheme and no path, named after the pipeline — <pipeline-name>-<generated-id>.<region>.osis.<domain> (an optional :<port> suffix is permitted). The client itself prepends the scheme and appends the Data Prepper source path (here /events) and signs the POST SigV4 for service osis. The identifying property is the FORM: a host whose first label is the request's PipelineName followed by "-" and a generated id, then the pipeline's region, then the label "osis", then the deployment's domain — the generated id, region label, domain and port are per-run/per-deployment values, not part of the requirement.
Emulator: GetPipeline returns a single entry of the form '127.0.0.1:39405/_aws/osis/<pipeline-name>' — a loopback host:port with an emulator-internal path component appended, and no pipeline-named host label at all. The replayed check reads IngestEndpointUrls for an ACTIVE pipeline it just created and fails at the shape assertion before any ingest request is made: AssertionError: published ingest URL is not a bare <pipeline-name>-<id>.us-west-2.osis host: '127.0.0.1:39405/_aws/osis/sim2real-ing-71f25852a6f0' (the 127.0.0.1, the port 39405 and the pipeline name are this run's values; the divergence is that the published value carries a path and is not a pipeline-named host).
Why: IngestEndpointUrls is the only way a client learns where to send documents. A client — or any library/IaC that stores the value as a hostname and builds "https://<value><source-path>" or uses it as a SigV4 Host header — composes a URL the emulator does not serve, because the emulator's value already contains a path segment and must instead be treated as an opaque base. Code written against AWS's bare-host contract therefore cannot reach the pipeline's ingest data plane at all, so no ingest-then-read-the-sink flow, and no ingest authentication behavior (the unsigned-POST refusal this check goes on to grade), is reachable.
Fix: the osis crate's pipeline state that holds the published endpoint, plus the GetPipeline/CreatePipeline response construction and the ingest listener routing that serves it — publish a bare host (optionally with a port) whose first label is "<PipelineName>-<generated-id>" followed by "<region>.osis.<domain>", served by the emulator on a per-resource listener, and serve the pipeline's configured http-source path directly under that host rather than under an /_aws/osis/<name> path prefix.
Note on prior issues: the resolved issue 20261109T133548Z-osis-getpipeline-advertises-ingestendpointurls-on-a-real-aws.md moved this value OFF an *.osis.amazonaws.com name the emulator did not serve; this is the opposite end of the same contract — the value is now reachable but no longer has AWS's bare pipeline-named host form, so it does not cover this cause. The open issue 20261127T234518Z-ec2-elbv2-opensearch-an-internal-scheme-alb-s.md reports the same class of defect for elbv2 DNSName (a different service) and covers nothing here.
Traces: osis.ingest-endpoint-unsigned-post-rejected.

--- Cause 2 of 2: StopPipeline on an already-STOPPED pipeline returns a ConflictException whose message is the emulator's own template, with no eligible-status list ---
AWS: Smithy StopPipeline (PUT /2022-01-01/osis/stopPipeline/{PipelineName}, 200 with Pipeline) models ConflictException with httpError 409 and member message; PipelineName is @required, @length 3..28, pattern ^[a-z][a-z0-9\-]+$. Recorded behavior: stopping a pipeline whose status is not eligible for stopping is refused 409 ConflictException with message 'Only pipelines with one of the following statuses are eligible for stopping: <statuses>. The current status is <status>.' Fixed text: the head 'Only pipelines with one of the following statuses are eligible for stopping: ' and the tail '. The current status is ' + <status> + '.'. Variable parts: <statuses>, the eligible-status set ACTIVE and UPDATE_FAILED rendered comma-separated in an order AWS does not document (so it is compared as a set), and <status>, the pipeline's CURRENT status read from service state (STOPPED in this replay). The pipeline name does NOT appear in AWS's message, and the refusal leaves the pipeline's status unchanged.
Emulator: the correct code and status (ConflictException/409), but the message is 'Pipeline <pipeline-name> cannot be stopped while its status is STOPPED' — the emulator's own template, which echoes the PipelineName AWS omits and drops the eligible-status list entirely. Replay: AssertionError: second StopPipeline answered ('ConflictException', 'Pipeline sim2real-stop-14309562f163 cannot be stopped while its status is STOPPED') (the pipeline name is this run's generated name).
Why: the eligible-status list is the actionable content of this error — a client decides from it whether to wait, start the pipeline first, or give up. Against the emulator that list is absent, and every matcher, log scraper or test keyed to AWS's wording fails to classify an otherwise correctly-coded 409.
Fix: StopPipeline's status-eligibility check in the osis crate's stop_pipeline operation handler — build the ConflictException message from the eligible-status set {ACTIVE, UPDATE_FAILED} and the pipeline's current status using AWS's fixed text, with no pipeline name, mirroring what the resolved issue 20261029T191455Z-osis-pipeline-error-paths-four-wrong-rejection-messages.md (its Cause 3) already did for StartPipeline's symmetric 'eligible for starting' message; that issue named only StartPipeline, so the StopPipeline path still carries the old template.
Traces: osis.stop-pipeline-already-stopped-rejected.

## Evidence
run_behavioral_suite (suite simparity-behavioral-aws-v1): total 21/26 passed (80.8%), 4 mismatched, 1 errored, 0 skipped, bootstrap_signal false — the osis emulator is up and answering (osis 21/25, 84%). The two checks below are verdict=mismatch and are marked neither "ALREADY FILED" nor "OWNED by".

Cause 1 — [mismatch] osis.ingest-endpoint-unsigned-post-rejected:
  File "check_osis_ingest_endpoint_unsigned_post_rejected.py", line 265, in run
    assert re.fullmatch(host_shape, urls[0]), ...
  AssertionError: published ingest URL is not a bare <pipeline-name>-<id>.us-west-2.osis host: '127.0.0.1:39405/_aws/osis/sim2real-ing-71f25852a6f0'
The check creates an S3 sink bucket and a writer role, CreatePipeline(MinUnits=1, MaxUnits=1) with an http source on path /events, waits for Status == ACTIVE, then reads GetPipeline(...)['Pipeline']['IngestEndpointUrls']. The preceding assertion (exactly one URL for an ACTIVE public pipeline) passed; the shape assertion is host_shape = rf"{re.escape(name)}-[a-z0-9]+\.{re.escape(ctx.region)}\.osis\.[a-z0-9.-]+(:\d+)?" — i.e. a bare host, optional port allowed, no path. Expected a value such as '<pipeline-name>-<id>.<region>.osis.<domain>[:<port>]'; got a loopback host:port followed by '/_aws/osis/<pipeline-name>'. Because the check stops here, its subsequent AWS-grounded data-plane assertions (signed POST -> 200; UNSIGNED POST -> 401 with body exactly '401 Unauthorized' and the event never reaching the sink; signed POST after the refusal -> 200) were never exercised against the emulator.

Cause 2 — [mismatch] osis.stop-pipeline-already-stopped-rejected:
  File "check_osis_stop_pipeline_already_stopped_rejected.py", line 187, in run
    assert sorted(named) == ["ACTIVE", "UPDATE_FAILED"], f"second StopPipeline answered {got!r}"
  AssertionError: second StopPipeline answered ('ConflictException', 'Pipeline sim2real-stop-14309562f163 cannot be stopped while its status is STOPPED')
The check creates a pipeline, waits for ACTIVE, StopPipeline, waits for STOPPED (both legs succeeded), then calls StopPipeline again. It parses the message as lead = 'Only pipelines with one of the following statuses are eligible for stopping: ', end = '. The current status is STOPPED.' and compares the named statuses as a SET ({ACTIVE, UPDATE_FAILED}) and the rest exactly; the emulator's message matches neither head nor tail, so the parsed list is empty. Error code and HTTP status are already correct; only the message diverges.

Model grounding (services/service-com-amazonaws-osis-amazonopensearchingestionservice/smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json): com.amazonaws.osis#Pipeline.IngestEndpointUrls -> IngestEndpointUrlsList, doc "The ingestion endpoints for the pipeline, which you can send data to" (no host constraint modelled — shape is recorded behavior); StopPipeline PUT /2022-01-01/osis/stopPipeline/{PipelineName} -> 200 Pipeline, errors ConflictException(409), ResourceNotFoundException(404), ValidationException(400), DisabledOperationException(409), AccessDeniedException(403), InternalException(500); PipelineName @required, @length 3..28, pattern ^[a-z][a-z0-9\-]+$.

## Steps to Reproduce
Boot the emulator from this checkout and replay the osis behavioral checks of suite simparity-behavioral-aws-v1 (run_behavioral_suite).

Cause 1 — osis.ingest-endpoint-unsigned-post-rejected:
1. Create an S3 bucket and an IAM role osis-pipelines.amazonaws.com may assume with s3:PutObject on that bucket.
2. CreatePipeline with MinUnits=1, MaxUnits=1 and a Data Prepper v2 configuration whose source is `http` with path "/events" and whose sink is that s3 bucket (json codec, sts_role_arn = the role).
3. Poll GetPipeline until Status == ACTIVE, then read Pipeline.IngestEndpointUrls.
4. Observe exactly one entry, but of the form '127.0.0.1:<port>/_aws/osis/<pipeline-name>'. Expected a BARE host matching ^<PipelineName>-[a-z0-9]+\.<region>\.osis\.[a-z0-9.-]+(:\d+)?$ with no path component.
5. After the fix, the published value must have that bare pipeline-named host form AND the emulator must serve the pipeline's configured source path directly under it: POST https://<published host>/events SigV4-signed for service osis -> 200 and the event reaches the sink; the same POST UNSIGNED -> 401 with body exactly '401 Unauthorized' and that event never reaching the sink.

Cause 2 — osis.stop-pipeline-already-stopped-rejected:
1. CreatePipeline (MinUnits=1, MaxUnits=1, http source + s3 sink) and poll GetPipeline until Status == ACTIVE.
2. StopPipeline, then poll until Status == STOPPED.
3. Call StopPipeline again for the same PipelineName.
4. Expect 409 ConflictException with message 'Only pipelines with one of the following statuses are eligible for stopping: <ACTIVE, UPDATE_FAILED in any order>. The current status is STOPPED.' — no pipeline name in the message — and GetPipeline still reporting STOPPED afterwards. Observe instead 'Pipeline <name> cannot be stopped while its status is STOPPED'.

Re-replay osis.ingest-endpoint-unsigned-post-rejected and osis.stop-pipeline-already-stopped-rejected to confirm both causes are closed.
