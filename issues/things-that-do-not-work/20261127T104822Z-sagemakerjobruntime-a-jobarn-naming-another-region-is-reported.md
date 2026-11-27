Issue-Source: trace-sweep-divergence
Issue-Trace: sagemakerjobruntime-job-arn-region-and-category-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemakerjobruntime-job-arn-region-and-category-rejected sagemakerjobruntime-trajectory-id-limit-and-empty-body sagemakerjobruntime-sample-with-response-stream-job-arn-scope-rejected
Issue-Shape-Id: com.amazonaws.sagemakerjobruntime#AgenticRFTRuntimeService
Issue-Key: sagemakerjobruntime|Sample|4xx->4xx|$.Message
Issue-Key: sagemakerjobruntime|SampleWithResponseStream|4xx->4xx|$.Message
Issue-Commit: 96893890513d5e7e8a4c2241420ddac2d94594cf

# sagemakerjobruntime: a JobArn naming another Region is reported as an unknown job instead of AWS's region-mismatch refusal

> _Note: this issue was identified against the code base at commit `96893890513d5e7e8a4c2241420ddac2d94594cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Sample

## Description
Two causes, one per sampling operation: neither checks the Region segment of the request's JobArn against the Region the endpoint serves, so a cross-Region ARN falls through to the job-directory lookup and is reported as a job that does not exist.

Cause 1 of 2: Sample does not refuse a JobArn addressed to another Region.
AWS: Smithy: Sample, POST /sample; JobArn is bound to httpHeader X-Amzn-SageMaker-Job-Arn, @required, @length 1..256, @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$ — the pattern's region segment ([a-z0-9\-]*) admits ANY Region, so the Region is checked by the service, not by the model; modelled errors include ValidationException(400) and ResourceNotFoundException(404). Recorded behavior: a syntactically valid JobArn whose Region segment differs from the Region the endpoint is serving is refused HTTP 400 with Message "Input arn <jobArn> does not match the current region <region>". Fixed text: "Input arn " / " does not match the current region "; variable parts: the JobArn exactly as the request spelled it, and the Region of the endpoint the call was made to. The refusal is about the ADDRESSING of the ARN and is therefore decided before any lookup of the job itself.
Emulator: HTTP 400 (status matches) but Message "Could not find job with jobArn <jobArn>" — src/state.rs job_not_found_message, reached from serve_sampling_turn's `if !job_exists(engine, request.job_arn)` branch. The crate deliberately strips partition/Region/account from a JobArn when it identifies a job (src/state.rs job_identity / job_identity_of_arn), so a cross-Region ARN is only ever seen as an identity the directory happens not to hold; no code path in the crate words a region rule at all (src/service.rs words only "Input arn <value> is malformed" for a @pattern violation).
Why: a client cannot distinguish a mis-addressed endpoint (wrong Region in the ARN, the job exists where it was created) from a job that was never created. The first is fixed by re-addressing the call, the second by creating the job; AWS's message names the Region the caller must use, the emulator's names nothing actionable, so cross-Region misconfiguration is diagnosed as a missing resource.
Fix: the sampling pre-flight / this service's ARN validation — src/state.rs serve_sampling_turn (the job_exists branch) together with the ARN wording seam in src/service.rs (refusal_message / malformed_arn_message): the Region comparison must run before the job-directory lookup and produce the sentence above for both sampling operations.
Traces: sagemakerjobruntime-job-arn-region-and-category-rejected#task#1 (cluster also covers #task#4 and sagemakerjobruntime-trajectory-id-limit-and-empty-body#task#6).

Cause 2 of 2: SampleWithResponseStream does not refuse a JobArn addressed to another Region.
AWS: Smithy: SampleWithResponseStream, POST /sample-with-response-stream, same JobArn member, constraints and @pattern, same error set. Recorded behavior: identical to cause 1 — HTTP 400, Message "Input arn <jobArn> does not match the current region <region>", refused before the stream is opened.
Emulator: HTTP 400 with Message "Could not find job with jobArn <jobArn>", from the same shared sampling pre-flight.
Why: same as cause 1, and for a streaming client the wrong diagnosis is taken before any frame is produced, so the caller retries a stream it should have re-addressed.
Fix: same seam — one Region check in the shared sampling pre-flight (src/state.rs serve_sampling_turn) so the two sampling operations cannot disagree.
Traces: sagemakerjobruntime-job-arn-region-and-category-rejected#task#2 (cluster also covers sagemakerjobruntime-sample-with-response-stream-job-arn-scope-rejected#task#2, #task#3, #task#4).

No open issue covers this: the fixed issues about JobArn resolution (e.g. 20261024T155452Z, 20261029T010104Z) are about job EXISTENCE and job identity keying, not about refusing an ARN addressed to a different Region.

## Evidence
Replay of trace set simparity-wire-aws-v1 (2 divergence clusters, 7 steps). Both clusters match on status (400) and diverge only in $.Message:

Cause 1 — fixture sagemakerjobruntime-job-arn-region-and-category-rejected, step task#1, Sample (cluster also: #task#4, sagemakerjobruntime-trajectory-id-limit-and-empty-body#task#6):
  expected: $.Message = "Input arn arn:aws:sagemaker:us-east-1:<account>:job/AgentRFT/<jobName> does not match the current region us-west-2"
  actual:   $.Message = "Could not find job with jobArn arn:aws:sagemaker:us-east-1:<account>:job/AgentRFT/<jobName>"

Cause 2 — same fixture, step task#2, SampleWithResponseStream (cluster also: sagemakerjobruntime-sample-with-response-stream-job-arn-scope-rejected#task#2, #task#3, #task#4):
  expected: $.Message = "Input arn arn:aws:sagemaker:us-east-1:<account>:job/AgentRFT/<jobName> does not match the current region us-west-2"
  actual:   $.Message = "Could not find job with jobArn arn:aws:sagemaker:us-east-1:<account>:job/AgentRFT/<jobName>"

(The ARN, account and Region above are the recording's values filling the template; the requirement is the sentence's fixed text plus the request's own JobArn and the endpoint's Region.)

Code that produces the actual answer: src/state.rs serve_sampling_turn, `if !job_exists(engine, request.job_arn) { return Err(SamplingRefusal::invalid(job_not_found_message(request.job_arn))) }` (~line 841) with job_not_found_message (~line 648) and job_identity (~line 283), which discards partition/Region/account; src/service.rs holds the service's only ARN-specific wording ("Input arn <value> is malformed") and no Region rule.

## Steps to Reproduce
1. Replay fixture sagemakerjobruntime-job-arn-region-and-category-rejected against the emulator.
2. Step task#1 — POST /sample to the job-runtime.sagemaker endpoint serving Region us-west-2, with header X-Amzn-SageMaker-Job-Arn set to a well-formed job ARN whose Region segment is us-east-1, a trajectory-id header and a non-empty body. AWS answers 400 "Input arn <jobArn> does not match the current region us-west-2"; the emulator answers 400 "Could not find job with jobArn <jobArn>".
3. Step task#2 — the same request against POST /sample-with-response-stream. Same divergence.
4. The same clusters reproduce at sagemakerjobruntime-job-arn-region-and-category-rejected#task#4, sagemakerjobruntime-trajectory-id-limit-and-empty-body#task#6 and sagemakerjobruntime-sample-with-response-stream-job-arn-scope-rejected#task#2..#task#4.
