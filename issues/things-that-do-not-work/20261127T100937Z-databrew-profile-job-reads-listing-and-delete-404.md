Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-list-job-runs-max-results-bounds
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-list-job-runs-max-results-bounds databrew-describe-job-run-unknown-run-and-job-not-found
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|DescribeJob|2xx->4xx|$.CreateDate
Issue-Key: databrew|DescribeJob|2xx->4xx|$.CreatedBy
Issue-Key: databrew|DescribeJob|2xx->4xx|$.DatasetName
Issue-Key: databrew|DescribeJob|2xx->4xx|$.JobSample
Issue-Key: databrew|DescribeJob|2xx->4xx|$.LogSubscription
Issue-Key: databrew|DescribeJob|2xx->4xx|$.MaxCapacity
Issue-Key: databrew|DescribeJob|2xx->4xx|$.MaxRetries
Issue-Key: databrew|DescribeJob|2xx->4xx|$.Name
Issue-Key: databrew|DescribeJob|2xx->4xx|$.Outputs
Issue-Key: databrew|DescribeJob|2xx->4xx|$.ResourceArn
Issue-Key: databrew|DescribeJob|2xx->4xx|$.RoleArn
Issue-Key: databrew|DescribeJob|2xx->4xx|$.Tags
Issue-Key: databrew|DescribeJob|2xx->4xx|$.Timeout
Issue-Key: databrew|DescribeJob|2xx->4xx|$.Type
Issue-Key: databrew|DescribeJob|2xx->4xx|status
Issue-Key: databrew|DeleteJob|2xx->4xx|$.Name
Issue-Key: databrew|DeleteJob|2xx->4xx|status
Issue-Key: databrew|ListJobRuns|2xx->4xx|$.JobRuns
Issue-Key: databrew|ListJobRuns|2xx->4xx|status
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# databrew: PROFILE job reads, listing and delete 404 because no profile job is ever stored

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJob

## Description
This bundle carries 4 CAUSES, all of them the same underlying gap: a job of Type PROFILE never enters the service store, so every operation that resolves a job BY NAME answers 404 ResourceNotFoundException for it instead of serving it. CreateProfileJob is still a scaffold (services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_profile_job.rs carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() returns Scaffolded::not_implemented), while CreateRecipeJob does insert a job record — so RECIPE jobs work and PROFILE jobs do not exist at all downstream. The auto-filed stub issue for CreateProfileJob covers only that operation's own 501; it does NOT cover these four downstream read/list/delete divergences, which is why they are filed here. The open issue 'cloudformation__databrew: ... AWS::DataBrew::* resources are stub-provisioned' is a different path (CloudFormation provisioning) and does not cover these either. All four causes are closed by one change: make CreateProfileJob persist a job record of Type PROFILE in crate::state (keyed by Name, carrying DatasetName, RoleArn, Outputs derived from OutputLocation, JobSample, LogSubscription, MaxCapacity, MaxRetries, Timeout, Tags, audit fields and the job ARN) so the existing job handlers resolve it.

Cause 1 of 4 — DescribeJob on a PROFILE job answers 404 instead of the job.
AWS: Smithy: DescribeJob GET /jobs/{Name}, success 200, output members Name (required), Type (JobType, PROFILE here), DatasetName, Outputs, RoleArn, ResourceArn, Tags, Timeout, MaxCapacity, MaxRetries, LogSubscription, JobSample, CreateDate, CreatedBy; errors ResourceNotFoundException (404) only when no job of that name exists. A job created by CreateProfileJob (Smithy: CreateProfileJob POST /profileJobs, output Name) exists from that moment, so DescribeJob on it returns 200 and the recorded job description.
Emulator: 404 ResourceNotFoundException with no body members at all ($.Name, $.Type, $.DatasetName, $.Outputs, $.RoleArn, $.ResourceArn, $.Tags, $.Timeout, $.MaxCapacity, $.MaxRetries, $.LogSubscription, $.JobSample, $.CreateDate, $.CreatedBy all missing), because store.jobs holds no entry for the profile job.
Why: a client that creates a profile job and then describes it to read back its ARN, type, output location or sampling configuration cannot tell a successful creation from a nonexistent job; any provisioning loop that waits for the job to become describable never converges.
Fix: CreateProfileJob's handler (and the job record it must write into crate::state); DescribeJob's handler needs no change once the record exists.
Traces: databrew-list-job-runs-max-results-bounds#task#10.

Cause 2 of 4 — DeleteJob on a PROFILE job answers 404 instead of 200 with the deleted name.
AWS: Smithy: DeleteJob DELETE /jobs/{Name}, success 200, output Name (required, the name of the job deleted); errors ConflictException (409), ResourceNotFoundException (404), ValidationException (400). Deleting an existing profile job returns 200 and echoes Name.
Emulator: 404 ResourceNotFoundException, $.Name missing — the job to delete was never stored.
Why: teardown of a profile job fails, so a client cannot clean up and cannot distinguish 'already deleted' from 'never created'; test and IaC teardown paths abort on the unexpected 404.
Fix: same CreateProfileJob persistence; DeleteJob's handler then finds and removes the record.
Traces: databrew-describe-job-run-unknown-run-and-job-not-found#task#10 and databrew-list-job-runs-max-results-bounds#teardown#11 (the teardown delete of the same profile job).

Cause 3 of 4 — ListJobRuns for a PROFILE job answers 404 instead of 200 with an empty JobRuns list.
AWS: Smithy: ListJobRuns GET /jobs/{Name}/jobRuns, success 200, output JobRuns (required list) + NextToken; ResourceNotFoundException (404) only when the job itself does not exist. A job that exists but has never been run returns 200 with JobRuns = [].
Emulator: 404 ResourceNotFoundException, $.JobRuns missing (expected the empty list).
Why: a client polling a freshly created profile job for its runs sees a not-found error rather than 'no runs yet', so it treats a healthy job as missing; the required JobRuns member is absent, which breaks SDK deserialization expectations for the success path.
Fix: same CreateProfileJob persistence; ListJobRuns' handler then resolves the job and pages its (empty) run set.
Traces: databrew-list-job-runs-max-results-bounds#task#7.

Cause 4 of 4 — DeleteJob in teardown answers 404, leaving the fixture's profile job unresolvable end to end.
AWS: as Cause 2 — DELETE /jobs/{Name} on an existing job is 200 with Name echoed.
Emulator: 404 with only the status diverging; the job was never created so the whole create/describe/list/delete lifecycle of a PROFILE job is unserved.
Why: the lifecycle an SDK caller performs on profile jobs (create, describe, list runs, delete) never completes against the emulator, so no profile-job workflow can be exercised at all.
Fix: CreateProfileJob's handler plus the shared job record in crate::state (the same file the other three causes point at).
Traces: databrew-list-job-runs-max-results-bounds#teardown#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):
- databrew-list-job-runs-max-results-bounds#task#10 DescribeJob: expected 200, got 404; expected $.Name='<profile job name>', $.Type='PROFILE', $.DatasetName='<dataset name>', $.JobSample={'Mode':'CUSTOM_ROWS','Size':20000}, $.LogSubscription='DISABLE', $.MaxCapacity=2, $.MaxRetries=0, $.Timeout=10, $.Outputs=[{Format:'JSON', Location:{Bucket:'<bucket>', BucketOwner:'<account>', Key:'profile/'}, Overwrite:false}], $.ResourceArn='arn:aws:databrew:<region>:<account>:job/<job name>', $.RoleArn='arn:aws:iam::<account>:role/<role name>', $.Tags={'sim2real':'wire'}, $.CreateDate and $.CreatedBy present — all missing in the emulator's response.
- databrew-list-job-runs-max-results-bounds#task#7 ListJobRuns: expected 200 with $.JobRuns=[], got 404 with $.JobRuns missing.
- databrew-describe-job-run-unknown-run-and-job-not-found#task#10 DeleteJob: expected 200 with $.Name='<profile job name>', got 404 with $.Name missing.
- databrew-list-job-runs-max-results-bounds#teardown#11 DeleteJob: expected 200, got 404.
Source evidence: services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_profile_job.rs lines 4 and 191 — SCAFFOLDED marker and Err(Scaffolded::not_implemented("CreateProfileJob")); contrast services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_recipe_job.rs line 5, which documents inserting a job record of Type RECIPE into crate::state.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS DataBrew client at it.
2. Create an S3-backed dataset and an IAM role, then call CreateProfileJob with a job Name, that DatasetName, a RoleArn, an OutputLocation, JobSample {Mode: CUSTOM_ROWS, Size: 20000}, MaxCapacity 2, MaxRetries 0, Timeout 10, LogSubscription DISABLE and a tag.
3. Call DescribeJob(Name=<job>) — emulator answers 404 ResourceNotFoundException instead of 200 with the PROFILE job description.
4. Call ListJobRuns(Name=<job>) — emulator answers 404 instead of 200 with JobRuns=[].
5. Call DeleteJob(Name=<job>) — emulator answers 404 instead of 200 echoing Name.
Equivalently: replay fixtures databrew-list-job-runs-max-results-bounds (steps task#7, task#10, teardown#11) and databrew-describe-job-run-unknown-run-and-job-not-found (step task#10).
