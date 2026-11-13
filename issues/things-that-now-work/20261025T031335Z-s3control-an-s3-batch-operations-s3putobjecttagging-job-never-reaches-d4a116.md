Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.iam-role-trusts-s3-service-positive
Issue-Key: athena__glue__s3.iam-role-trusts-s3-service-positive|_wait_for_job_terminal#24|Timeout|time.sleep(10)
Issue-Commit: 176bb95e3c08e52684921328b2da3d677bd84f34

# An S3 Batch Operations S3PutObjectTagging job never reaches a terminal status, so a role trusting batchoperations.s3.amazonaws.com is never proven assumable (1 cause)

> _Note: this issue was identified against the code base at commit `176bb95e3c08e52684921328b2da3d677bd84f34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
One root cause, grounded in one failing behavioral check of suite simparity-behavioral-ga-prod-v1.

Cause 1 of 1 — a submitted S3 Control batch job is recorded but never progressed or executed.

AWS: Smithy com.amazonaws.s3control#CreateJob returns a JobId, and DescribeJob returns a JobDescriptor whose Status is the enum com.amazonaws.s3control#JobStatus = Active|Cancelled|Cancelling|Complete|Completing|Failed|Failing|New|Paused|Pausing|Preparing|Ready|Suspended. Recorded behavior: with ConfirmationRequired=false the job walks New -> Preparing -> Active -> Complete with no further input; the tasks really run, so for Operation S3PutObjectTagging every object named in the manifest ends up carrying the job's TagSet. Role assumability is not checked at CreateJob time — the RoleArn is assumed asynchronously when the job runs, so a role that cannot be assumed makes the job terminate Failed with a FailureReasons entry rather than making CreateJob fail.

Emulator: everything up to and including CreateJob answers immediately (CreateBucket, two PutObjects, CreateRole + the inline policy, GetCallerIdentity, CreateJob), but the job's status never becomes Complete, Failed or Cancelled, so the check's bounded DescribeJob poll never terminates and the check is killed at the suite's 60 s wall clock — before the Complete assertion and the GetObjectTagging assertion are reached. Hypothesis: the job record is created in a non-terminal status with no engine advancing it (the same shape as this emulator's Glue job runs), rather than any single call hanging.

Why: Batch Operations is only meaningful as execution over stored object state. A client cannot wait for a job, read a ProgressSummary or consume a completion report; and the specific point of this check — that a role trusting batchoperations.s3.amazonaws.com is genuinely assumed and used — is unobservable, because a job that does nothing is indistinguishable from one whose trust policy is wrong. The allow path and the deny path both produce 'nothing happened'.

Fix: the s3control batch-job state machine (progress a submitted job to a terminal status) and its task execution for S3PutObjectTagging, including the asynchronous role assumption that populates FailureReasons. The open issue 20261025T000830Z-s3control-an-s3-batch-operations-s3putobjecttagging-job-never-reaches.md records the same root cause for a sibling check and does not carry this check's replay key; this check id is recorded here so it is re-replayed with that fix.

Traces: athena__glue__s3.iam-role-trusts-s3-service-positive.

Service-attribution note: the leading token of the check id is the composite 'athena__glue__s3' (the check declares SERVICES = ['athena','glue','s3'] and exercises sts/iam/s3/s3control before anything Athena- or Glue-related), which is not a single service token. The hanging operations are S3 Control's CreateJob/DescribeJob, so this issue is filed under service 's3control' (bootstrapped).

## Evidence
Expected (AWS): s3control.create_job(AccountId=<acct>, ConfirmationRequired=False, Operation={'S3PutObjectTagging': {'TagSet': [...]}}, Manifest={'Spec': {'Format': 'S3BatchOperations_CSV_20180820', 'Fields': ['Bucket','Key']}, 'Location': {'ObjectArn': 'arn:aws:s3:::<bucket>/manifest/manifest.csv', 'ETag': <etag>}}, Priority=1, RoleArn=<role trusting batchoperations.s3.amazonaws.com>) -> 200 + JobId; s3control.describe_job(AccountId, JobId)['Job']['Status'] reaches a terminal value (Complete, or Failed with a FailureReasons entry about assuming the IAM role); s3.get_object_tagging on the manifest's object then returns the job's TagSet.

Actual: 'Check timed out after 60.0s' on athena__glue__s3.iam-role-trusts-s3-service-positive. The DescribeJob poll never observed a terminal status, so neither the terminal-status assertion nor the object-tagging assertion ran. All preceding calls (create_bucket, put_object x2, create_role, put/attach policy, sts.get_caller_identity, create_job) answered promptly.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=b); s3.put_object(Bucket=b, Key='objects/target.txt', Body=b'hello-batch'); etag = s3.put_object(Bucket=b, Key='manifest/manifest.csv', Body=b'<b>,objects/target.txt\n')['ETag'].
3. iam.create_role with a trust policy for Principal {'Service': 'batchoperations.s3.amazonaws.com'} and a policy allowing s3:GetObject on the manifest, and s3:GetObject/s3:PutObject/s3:PutObjectTagging on arn:aws:s3:::<b>/*.
4. s3control.create_job(AccountId=<acct>, ConfirmationRequired=False, Priority=1, RoleArn=<role>, Operation={'S3PutObjectTagging': {'TagSet': [{'Key':'sim2real','Value':'tagged'}]}}, Manifest={'Spec': {'Format':'S3BatchOperations_CSV_20180820','Fields':['Bucket','Key']}, 'Location': {'ObjectArn':'arn:aws:s3:::<b>/manifest/manifest.csv','ETag':etag}}) -> 200 + JobId.
5. Poll s3control.describe_job(AccountId, JobId)['Job']['Status'] -> observe it never reaching Complete/Failed/Cancelled; expect a terminal status.
6. s3.get_object_tagging(Bucket=b, Key='objects/target.txt') -> expect the job's TagSet; observe [].
7. Re-replay athena__glue__s3.iam-role-trusts-s3-service-positive.
