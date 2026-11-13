Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.iam-role-trusts-s3-service-positive
Issue-Key: athena__glue__s3.iam-role-trusts-s3-service-positive|_wait_for_job_terminal#24|Timeout|time.sleep(2)
Issue-Commit: 1bdb62554babe5a57dd4c3017ee8c08adb16fcb5

# S3 Control batch job (S3PutObjectTagging) never reaches a terminal status, so the batchoperations.s3.amazonaws.com trust path is never exercised and the manifest's object is never tagged

> _Note: this issue was identified against the code base at commit `1bdb62554babe5a57dd4c3017ee8c08adb16fcb5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
One net-new root cause, grounded in one failing behavioral check of suite simparity-behavioral-aws-v1.

Cause 1 of 1: a submitted S3 Batch Operations job is accepted by CreateJob but never progresses to a terminal status and its tasks are never executed, so the job's RoleArn is never assumed asynchronously and the objects named in the manifest are never tagged.

AWS: Smithy com.amazonaws.s3control#CreateJob — POST /v20180820/jobs, HTTP 200, output member JobId (@length 5-36, pattern ^[a-zA-Z0-9\-\_]+$); required input members Operation, Report, ClientRequestToken (@idempotencyToken), Priority, RoleArn (pattern ^arn:[^:]+:iam::\d{12}:role/), with AccountId bound to the x-amz-account-id header and the host prefix {AccountId}. Smithy com.amazonaws.s3control#DescribeJob — GET /v20180820/jobs/{JobId}, HTTP 200, output member Job of shape JobDescriptor, whose Status member targets enum com.amazonaws.s3control#JobStatus = Active|Cancelled|Cancelling|Complete|Completing|Failed|Failing|New|Paused|Pausing|Preparing|Ready|Suspended; JobDescriptor also models ProgressSummary, FailureReasons ("If the specified job failed, this field contains information describing the failure") and TerminationDate ("a timestamp indicating when this job terminated ... when it succeeded, failed, or was canceled"). Recorded behavior: with ConfirmationRequired=false the job advances on its own out of New/Preparing/Active into a terminal Complete; S3 Batch Operations does not validate role assumability at CreateJob time but assumes RoleArn asynchronously when the job runs, so an un-assumable role is observed as a terminal Failed with a FailureReasons entry about not being able to assume the IAM role. For Operation S3PutObjectTagging the tasks really run: every object listed in the manifest ends up carrying exactly the job's TagSet, observable through s3:GetObjectTagging, and an enabled Report is written under <Report.Prefix>/job-<JobId>/.

Emulator: DescribeJob keeps answering 200 with a non-terminal Job.Status for the whole life of the check — the status never enters {Complete, Failed, Cancelled}. The check creates the job twice (a 280 s terminal-status poll at 2 s intervals per attempt) and is killed by the suite's wall clock: the replay output is "Error: Check timed out after 60.0s". Everything ahead of the poll answers immediately (s3.create_bucket, put_bucket_tagging, two put_objects, sts.get_caller_identity, iam.create_role, iam.put_role_policy, s3control.create_job), so the stall is the job state machine, not a hanging call. Consequently the assertion final_status == 'Complete' and the follow-up s3.get_object_tagging assertion for {'batch': 'true'} are never reached, and no completion report is written.

Why: a client cannot wait for a batch job, cannot read a ProgressSummary or FailureReasons, cannot consume a completion report, and — the specific point of this check — cannot distinguish a role that correctly trusts batchoperations.s3.amazonaws.com from one whose trust policy is wrong: both produce 'nothing ever happened' instead of Complete-plus-tagged-object vs Failed-with-FailureReasons. Any workflow that polls DescribeJob for a terminal status hangs forever against this emulator.

Fix: the s3control batch-job state machine (advance an accepted job through New/Preparing/Active to a terminal status, set TerminationDate and ProgressSummary) and its task execution for Operation S3PutObjectTagging, including the asynchronous assumption of RoleArn that must populate FailureReasons and a terminal Failed when the role cannot be assumed, plus writing the enabled Report. Related open issue naming the same root cause from a different check id: 20261025T000830Z-s3control-an-s3-batch-operations-s3putobjecttagging-job-never-reaches.md (grounded in emr__glue__s3.iam-role-trust-s3-service-assumed-for-batch-op-positive) — it does not carry this check's replay key, which is why this check is recorded here and should be re-replayed together with it.

Traces: athena__glue__s3.iam-role-trusts-s3-service-positive.

Service-attribution note: the suite lists this check under the composite heading 'athena__glue__s3' (the check declares SERVICES = ['athena','glue','s3']), which is not a single service token. The diverging operations are S3 Control's CreateJob/DescribeJob, so the issue is filed under service 's3control', which is bootstrapped.

## Evidence
Expected (AWS): s3control.create_job(AccountId=<account>, ConfirmationRequired=False, Operation={'S3PutObjectTagging': {'TagSet': [{'Key': 'batch', 'Value': 'true'}]}}, Manifest={'Spec': {'Format': 'S3BatchOperations_CSV_20180820', 'Fields': ['Bucket','Key']}, 'Location': {'ObjectArn': 'arn:aws:s3:::<bucket>/manifest/manifest.csv', 'ETag': <etag of that object>}}, Report={'Bucket': 'arn:aws:s3:::<bucket>', 'Prefix': 'reports', 'Format': 'Report_CSV_20180820', 'Enabled': True, 'ReportScope': 'AllTasks'}, Priority=1, RoleArn=<role trusting batchoperations.s3.amazonaws.com>) -> HTTP 200 with JobId; then s3control.describe_job(AccountId, JobId)['Job']['Status'] reaches the terminal value 'Complete' (or 'Failed' with a FailureReasons entry describing the failed role assumption while the role is still propagating); then s3.get_object_tagging(Bucket=<bucket>, Key='objects/target.txt')['TagSet'] == [{'Key': 'batch', 'Value': 'true'}].

Actual (emulator): CreateJob is accepted and returns a JobId, but DescribeJob('Job']['Status']) never becomes one of Complete/Failed/Cancelled. The check's _wait_for_job_terminal poll (describe_job every 2 s, 280 s budget, two create-and-poll attempts) never observes a terminal status and the suite reports the failure as 'Error: Check timed out after 60.0s' for athena__glue__s3.iam-role-trusts-s3-service-positive. The assertions final_status == 'Complete' and tags.get('batch') == 'true' are never reached; no completion report object appears under reports/job-<JobId>/.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=b) (add CreateBucketConfiguration={'LocationConstraint': <region>} outside us-east-1); s3.put_object(Bucket=b, Key='objects/target.txt', Body=b'hello-batch'); etag = s3.put_object(Bucket=b, Key='manifest/manifest.csv', Body=b'<b>,objects/target.txt\n')['ETag'].
3. iam.create_role(RoleName=r, AssumeRolePolicyDocument=<trust policy for Principal {'Service': 'batchoperations.s3.amazonaws.com'}, Action sts:AssumeRole>); iam.put_role_policy(RoleName=r, PolicyName='batch-ops', PolicyDocument=<Allow s3:GetObject on arn:aws:s3:::<b>/manifest/manifest.csv; Allow s3:GetObject, s3:PutObject, s3:PutObjectTagging on arn:aws:s3:::<b>/*; Allow s3:GetBucketLocation on arn:aws:s3:::<b>>).
4. s3control.create_job(AccountId=<account from sts.get_caller_identity>, ConfirmationRequired=False, Operation={'S3PutObjectTagging': {'TagSet': [{'Key':'batch','Value':'true'}]}}, Manifest={'Spec': {'Format':'S3BatchOperations_CSV_20180820','Fields':['Bucket','Key']}, 'Location': {'ObjectArn':'arn:aws:s3:::<b>/manifest/manifest.csv','ETag':etag}}, Report={'Bucket':'arn:aws:s3:::<b>','Prefix':'reports','Format':'Report_CSV_20180820','Enabled':True,'ReportScope':'AllTasks'}, Priority=1, RoleArn=<role arn>) -> 200 + JobId.
5. Poll s3control.describe_job(AccountId, JobId)['Job']['Status'] every 2 s: expect it to reach 'Complete' (or 'Failed' with FailureReasons); observe it never reaching any terminal status.
6. s3.get_object_tagging(Bucket=b, Key='objects/target.txt') -> expect [{'Key':'batch','Value':'true'}]; observe an empty TagSet.
7. Re-replay athena__glue__s3.iam-role-trusts-s3-service-positive.
