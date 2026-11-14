Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.third-party-job-details-reflect-polled-job codepipeline.third-party-job-nonce-single-use-visibility
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.third-party-job-details-reflect-polled-job|run#11|AssertionError|assert details_code == "JobNotFoundException", details_code
Issue-Key: codepipeline.third-party-job-nonce-single-use-visibility|run#14|AssertionError|assert code3 == "JobNotFoundException", code3
Issue-Commit: 3ce102081c5eb2c7e7b5721e0bb6bf7cb79dc6fe

# CodePipeline GetThirdPartyJobDetails answers 501 NotImplemented instead of the modeled JobNotFoundException for an unknown job id

> _Note: this issue was identified against the code base at commit `3ce102081c5eb2c7e7b5721e0bb6bf7cb79dc6fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetThirdPartyJobDetails

## Description
One net-new root cause in the codepipeline behavioral sweep. Neither failing check is marked ALREADY FILED or OWNED by the replay report. Related open issue: 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md — its Cause 8 covers the PutThirdPartyJobFailureResult / PutThirdPartyJobSuccessResult stubs and its Cause 7 the UntagResource stub, but it says nothing about GetThirdPartyJobDetails and its trace list carries neither check id below. The fixed issues 20261022T173520Z-codepipeline-pollforthirdpartyjobs-stub.md and 20261022T173520Z-codepipeline-acknowledgethirdpartyjob-stub.md did land: in both checks PollForThirdPartyJobs now refuses with AccessDeniedException and AcknowledgeThirdPartyJob now refuses with JobNotFoundException as AWS does — the surviving failure is the third-party READ operation only.

=== Cause 1 of 1: GetThirdPartyJobDetails is an unimplemented scaffold stub ===
AWS: Smithy: GetThirdPartyJobDetails takes jobId (ThirdPartyJobId, required, @length 1..512) and clientToken (ClientToken, required, @length 1..256) and models exactly four errors, all errorKind client at HTTP 400: InvalidClientTokenException, InvalidJobException, JobNotFoundException, ValidationException. The model documents JobNotFoundException as "The job was specified in an invalid format or cannot be found." A well-formed jobId (a uuid) that was never issued to this caller by PollForThirdPartyJobs is refused with error code JobNotFoundException, HTTP 400, and the message AWS words for this error class as the fixed text "Job with id '<jobId>' does not exist." — fixed text "Job with id '" ... "' does not exist.", with the single variable part being the jobId taken verbatim from the request (the uuid in each recording is only one filling of that template). This is the same wording the emulator already produces for AcknowledgeThirdPartyJob on the same id, which both checks assert passes immediately before this call.
Emulator: the operation is a scaffold stub: services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/get_third_party_job_details.rs still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and its handle() body ends in Err(crate::scaffolded::Scaffolded::not_implemented("GetThirdPartyJobDetails")), so every call answers HTTP 501 with error code NotImplemented. Both checks report AssertionError: NotImplemented where JobNotFoundException was required.
Why: a partner job worker cannot read a job at all, and a client that distinguishes "this job id is unknown/expired" (JobNotFoundException) from "bad client token" (InvalidClientTokenException) from a transport fault gets a code that is not in the operation's modeled error set and an HTTP status (501) that botocore treats as a retryable server fault rather than a terminal client refusal. It also makes the "effect absent" assertion unreachable: a client cannot confirm, via this independent read, that a refused AcknowledgeThirdPartyJob left no phantom job behind.
Fix: implement the GetThirdPartyJobDetails handler (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/get_third_party_job_details.rs, the Err(..not_implemented..) at line 167) against the third-party job store, returning JobNotFoundException for a jobId the store never issued and InvalidClientTokenException for a jobId that exists but whose clientToken does not match; delete the SCAFFOLDED marker line at the top of the file.
Traces: codepipeline.third-party-job-details-reflect-polled-job, codepipeline.third-party-job-nonce-single-use-visibility.

## Evidence
Cause 1 — expected: HTTP 400, error code JobNotFoundException, message "Job with id '<jobId>' does not exist." (fixed text; <jobId> is the request's jobId). Actual: HTTP 501, error code NotImplemented ("operation ... is scaffolded but not implemented in this emulator").
  codepipeline.third-party-job-details-reflect-polled-job: check_codepipeline_third_party_job_details_reflect_polled_job.py line 70 — `assert details_code == "JobNotFoundException", details_code` -> AssertionError: NotImplemented. The preceding assertions in the same check PASS: poll_for_jobs on a Custom-owned action type returns jobs == [] (positive control) and poll_for_third_party_jobs is refused with AccessDeniedException and the exact "Not whitelisted for accessing ActionType.Category=... ActionType.Owner='ThirdParty' ..." text.
  codepipeline.third-party-job-nonce-single-use-visibility: check_codepipeline_third_party_job_nonce_single_use_visibility.py line 71 — `assert code3 == "JobNotFoundException", code3` -> AssertionError: NotImplemented. Here too the earlier assertions pass: poll_for_third_party_jobs -> AccessDeniedException with the whitelist text, and acknowledge_third_party_job(jobId=<uuid>, nonce="0", clientToken="does-not-exist") -> JobNotFoundException with message "Job with id '<uuid>' does not exist.". Only the sibling read of the SAME id diverges.
Source evidence: services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/get_third_party_job_details.rs:4 ("SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`") and :167 (`Err(crate::scaffolded::Scaffolded::not_implemented("GetThirdPartyJobDetails"))`).

## Steps to Reproduce
1. Boot the emulator and replay the two codepipeline checks listed in trace_ids.
2. Minimal repro: call codepipeline GetThirdPartyJobDetails with a well-formed, never-issued job id and any client token, e.g. get_third_party_job_details(jobId=str(uuid.uuid4()), clientToken="does-not-exist").
3. Observe HTTP 501 with error code NotImplemented. AWS answers HTTP 400 with error code JobNotFoundException and message "Job with id '<the jobId sent>' does not exist.".
4. For contrast, call acknowledge_third_party_job(jobId=<the same uuid>, nonce="0", clientToken="does-not-exist") against the same emulator — it already returns the correct JobNotFoundException and message, so only the read path is missing.
