Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__lightsail.static-ip-allocate-and-release-are-audited-around-a-read-only-get cloudtrail__lightsail.bucket-create-update-delete-are-audited-as-management-events cloudtrail__lightsail.create-bucket-unknown-bundle-rejected-and-audited-with-its-error cloudtrail__lightsail.second-allocate-of-a-held-static-ip-name-rejected-and-audited
Issue-Key: cloudtrail__lightsail.static-ip-allocate-and-release-are-audited-around-a-read-only-get|run#15|Timeout|time.sleep(_PAUSE_S)
Issue-Key: lightsail|GetBuckets|NotFoundException
Issue-Key: cloudtrail__lightsail.create-bucket-unknown-bundle-rejected-and-audited-with-its-error|run#10|AssertionError|assert got == ("InvalidInputException", f"The bundle id {_NO_SUCH_BUNDLE} you provided is not valid."), (
Issue-Key: cloudtrail__lightsail.second-allocate-of-a-held-static-ip-name-rejected-and-audited|run#9|AssertionError|assert got == ("InvalidInputException", f"Some names are already in use: {name}"), (
Issue-Commit: e2f414c8e0d435baa8e4d1479bd7220d9eecc351

# cloudtrail__lightsail: CloudTrail never audits Lightsail management events, and three Lightsail refusals carry non-AWS messages (4 causes)

> _Note: this issue was identified against the code base at commit `e2f414c8e0d435baa8e4d1479bd7220d9eecc351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Four distinct root causes found by replaying the behavioral suite's cloudtrail__lightsail checks against the emulator. All four are reported under the `cloudtrail__lightsail` heading and all four must be closed before this issue is submitted. Cause 1 is CloudTrail's; Causes 2-4 are the Lightsail refusals these audit checks assert before they reach CloudTrail.

=== Cause 1 of 4: CloudTrail LookupEvents never reports Lightsail control-plane calls, so no Lightsail activity is auditable ===
AWS: Smithy/recorded behavior: CloudTrail LookupEvents filtered by LookupAttributes [{AttributeKey: 'EventSource', AttributeValue: 'lightsail.amazonaws.com'}] with a StartTime returns one `Events` entry per Lightsail API call made in that window, each carrying a `CloudTrailEvent` JSON document with eventSource `lightsail.amazonaws.com`, eventType `AwsApiCall`, the eventName of the operation, `readOnly` false for mutating calls (AllocateStaticIp, ReleaseStaticIp, CreateBucket, UpdateBucket, DeleteBucket) and true for reads (GetStaticIp), `awsRegion` = the caller's region, `recipientAccountId` = the caller's account, `errorCode` absent on a successful call and present with the service's error code on a refused one, `requestParameters` echoing the request members (e.g. `staticIpName`), and an `eventTime` ordered by call order. Delivery to event history is documented as about 5 minutes on average, not a bound.
Emulator: no such record ever appears. After AllocateStaticIp -> GetStaticIp -> ReleaseStaticIp on one static IP (each call succeeding, and the release verified by GetStaticIps no longer listing the name), the LookupEvents poll filtered on EventSource=lightsail.amazonaws.com never yields any of the three event names; the check exhausted its 60 s budget in that poll loop. Hypothesis: the emulator's Lightsail handlers do not emit CloudTrail records at all, so LookupEvents has nothing to return for that event source.
Why: auditing is unobservable. Any client, control, or test that proves an action happened by finding its CloudTrail record — compliance checks, change-detection pipelines, incident timelines, "who deleted this" queries — gets an empty answer and concludes the action never occurred. Because the gap is total it also hides every per-record field above (readOnly classification, errorCode on refusals, region/account attribution, call ordering).
Fix: Lightsail control-plane operations must emit CloudTrail management-event records into the store LookupEvents reads — i.e. the CloudTrail event-recording path invoked from the lightsail service crate's operation handlers (or the shared middleware that should record every authenticated API call), plus LookupEvents' EventSource lookup over that store.
Traces: cloudtrail__lightsail.static-ip-allocate-and-release-are-audited-around-a-read-only-get.

=== Cause 2 of 4: reading a deleted bucket is refused with the wrong NotFoundException message template ===
AWS: Smithy: GetBuckets.bucketName -> BucketName (@length 3..54, @pattern ^[a-z0-9][a-z0-9-]{1,52}[a-z0-9]$); errors include NotFoundException (client, httpError 404, members {code, docs, message, tip}). The message is recorded behavior only: a bucket the account no longer holds is refused with code `NotFoundException` and message `Requested resource not found: {<bucketName>}` — fixed text `Requested resource not found: {` ... `}` around the single variable part `<bucketName>`, copied from the request.
Emulator: code `NotFoundException` (matching) but message `The Bucket does not exist: <bucketName>` — a different template, without the braces and with a resource-type word AWS does not use here.
Why: a client that shows or matches the service's not-found text sees wording AWS never emits, and one that extracts the braced name from the message to decide which bucket to recreate finds nothing to extract.
Fix: the not-found error construction in the bucket operations' handlers (GetBuckets and the bucket metric reader) in the lightsail service crate's ops module.
Traces: cloudtrail__lightsail.bucket-create-update-delete-are-audited-as-management-events.

=== Cause 3 of 4: CreateBucket's unknown-bundle refusal carries the emulator's own wording ===
AWS: Smithy: CreateBucket.bundleId -> NonEmptyString (@pattern \S), required; the only modelled client input error is InvalidInputException (httpError 400, members {code, docs, message, tip}). A bundleId that is not a bucket bundle this account/region offers is refused with HTTP 400, code `InvalidInputException`, message `The bundle id <bundleId> you provided is not valid.` — fixed text `The bundle id ` ... ` you provided is not valid.` around the single variable part `<bundleId>`, copied verbatim and unquoted from the request.
Emulator: HTTP 400 `InvalidInputException` (type and status match) with message `bundleId '<bundleId>' is not an active bucket bundle this service offers; use GetBucketBundles to list the bundle IDs you can specify` — its own sentence, the value quoted, and advice AWS does not give.
Why: the refusal is the one a caller surfaces to a user or asserts on; text AWS never produces breaks message-level handling and misleads anyone reading the emulator's output as AWS's. The audit check additionally needs this error's code to appear as the CloudTrail record's `errorCode`, which Cause 1 prevents entirely.
Fix: CreateBucket's bundle validation and error construction in the lightsail ops module.
Traces: cloudtrail__lightsail.create-bucket-unknown-bundle-rejected-and-audited-with-its-error.

=== Cause 4 of 4: AllocateStaticIp's name-in-use refusal carries the emulator's own wording ===
AWS: Smithy: AllocateStaticIp.staticIpName -> ResourceName (@pattern ^\w[\w\-]*\w$), required; errors include InvalidInputException (400). Allocating a static IP under a name the account already holds is refused with HTTP 400, code `InvalidInputException`, message `Some names are already in use: <staticIpName>` — fixed text `Some names are already in use: ` followed by the conflicting name(s) taken from the request, unquoted.
Emulator: HTTP 400 `InvalidInputException` with message `a static IP named '<staticIpName>' already exists` — its own sentence, singular and quoted, where AWS uses the plural name-list form.
Why: as Cause 3 — the documented refusal text is part of the contract a caller handles or displays, and the list form is what tells a client that the conflict is a set of names rather than one.
Fix: AllocateStaticIp's uniqueness check and error construction in the lightsail ops module.
Traces: cloudtrail__lightsail.second-allocate-of-a-held-static-ip-name-rejected-and-audited.

## Evidence
Behavioral suite replay against the emulator built from this checkout: 25/49 checks passed; service `cloudtrail__lightsail` 0/4 (0%), 4 failed. Verbatim replay output per cause:

Cause 1 — cloudtrail__lightsail.static-ip-allocate-and-release-are-audited-around-a-read-only-get [mismatch]:
  Error: Check timed out after 60.0s
  The check had already completed AllocateStaticIp, GetStaticIp (name matched, isAttached False) and ReleaseStaticIp successfully, and verified via the GetStaticIps paginator that the released name is no longer listed. It then entered the delivery-wait loop
    seen = _trail_for(ct, since, name)                       # LookupEvents, LookupAttributes=[{EventSource: lightsail.amazonaws.com}], StartTime=now-2min
    while not {'AllocateStaticIp','GetStaticIp','ReleaseStaticIp'} <= set(seen): ... time.sleep(2)
  and never left it, so LookupEvents returned no record whose CloudTrailEvent had requestParameters.staticIpName == the static IP. The loop's own assertion message would have read "after 900 s CloudTrail held only [] of ['AllocateStaticIp','GetStaticIp','ReleaseStaticIp'] for the static IP". The per-record assertions it never reached require, for each of the three events: (eventSource, eventType, readOnly, awsRegion, errorCode) == ('lightsail.amazonaws.com', 'AwsApiCall', {AllocateStaticIp: False, GetStaticIp: True, ReleaseStaticIp: False}[event], ctx.region, None), recipientAccountId == ctx.account, exactly one record per event name, and eventTimes strictly increasing in call order. The 60 s cap is the suite's per-check budget; the failure is that zero records existed on the first and every poll, not that delivery was slow.

Cause 2 — cloudtrail__lightsail.bucket-create-update-delete-are-audited-as-management-events [mismatch]:
  AssertionError: reading the deleted bucket answered ('NotFoundException', 'The Bucket does not exist: sim2real-ct-bkt-d64561421127')
  assertion: `got == ("NotFoundException", f"Requested resource not found: {{{bucket}}}")` — i.e. the code matched and only the message template diverged. CreateBucket/UpdateBucket/DeleteBucket themselves succeeded and the bucket reached state OK beforehand.

Cause 3 — cloudtrail__lightsail.create-bucket-unknown-bundle-rejected-and-audited-with-its-error [mismatch]:
  AssertionError: CreateBucket with an unknown bundle answered ('InvalidInputException', "bundleId 'sim2real_no_such_bundle_9_9' is not an active bucket bundle this service offers; use GetBucketBundles to list the bundle IDs you can specify")
  assertion: `got == ("InvalidInputException", f"The bundle id {_NO_SUCH_BUNDLE} you provided is not valid.")` where _NO_SUCH_BUNDLE is the bundleId sent in the request.

Cause 4 — cloudtrail__lightsail.second-allocate-of-a-held-static-ip-name-rejected-and-audited [mismatch]:
  AssertionError: allocating a held name again answered ('InvalidInputException', "a static IP named 'sim2real-ct-dup-46a3e56ac7a4' already exists")
  assertion: `got == ("InvalidInputException", f"Some names are already in use: {name}")`; the first AllocateStaticIp of that name had succeeded.

Model check (services/service-com-amazonaws-lightsail-lightsail-20161128/smithy-model/com-amazonaws-lightsail-lightsail-20161128.json): InvalidInputException = client error, httpError 400, members {code, docs, message, tip}, and is the only modelled client input error on CreateBucket; NotFoundException = client error, httpError 404, same members. AllocateStaticIp.staticIpName -> ResourceName @pattern ^\w[\w\-]*\w$ required; CreateBucket.bundleId -> NonEmptyString @pattern \S required; GetBuckets.bucketName -> BucketName @length 3..54.

The resource names, bucket names, operation ids and timestamps quoted above are this run's values filling the message templates; only the fixed text and the request-derived placeholders named in the description are requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 suite.
2. Cause 1: call lightsail AllocateStaticIp(staticIpName=N), then GetStaticIp(staticIpName=N), then ReleaseStaticIp(staticIpName=N), pausing ~1.5 s between calls. Then page cloudtrail LookupEvents with LookupAttributes=[{AttributeKey:'EventSource', AttributeValue:'lightsail.amazonaws.com'}] and StartTime a few minutes earlier, and json-decode each Events[*].CloudTrailEvent. Expect one record each for AllocateStaticIp (readOnly false), GetStaticIp (readOnly true) and ReleaseStaticIp (readOnly false), all with eventSource lightsail.amazonaws.com, eventType AwsApiCall, awsRegion = caller's region, recipientAccountId = caller's account, no errorCode, requestParameters.staticIpName = N, and eventTimes in call order. The emulator returns no record for that event source at all, so the poll never terminates. Check: cloudtrail__lightsail.static-ip-allocate-and-release-are-audited-around-a-read-only-get.
3. Cause 2: CreateBucket(bucketName=B, bundleId='small_1_0'), wait for state OK, UpdateBucket, DeleteBucket(forceDelete=True), then GetBuckets(bucketName=B). Expect NotFoundException with message `Requested resource not found: {B}`; the emulator answers `The Bucket does not exist: B`. Check: cloudtrail__lightsail.bucket-create-update-delete-are-audited-as-management-events.
4. Cause 3: CreateBucket(bucketName=<fresh>, bundleId='sim2real_no_such_bundle_9_9'). Expect 400 InvalidInputException with message `The bundle id sim2real_no_such_bundle_9_9 you provided is not valid.`; the emulator answers its own "is not an active bucket bundle this service offers..." sentence. Check: cloudtrail__lightsail.create-bucket-unknown-bundle-rejected-and-audited-with-its-error.
5. Cause 4: AllocateStaticIp(staticIpName=N) twice. The second call must be refused 400 InvalidInputException with message `Some names are already in use: N`; the emulator answers `a static IP named 'N' already exists`. Check: cloudtrail__lightsail.second-allocate-of-a-held-static-ip-name-rejected-and-audited.
6. After Causes 3 and 4 are fixed, the refused call must also appear in LookupEvents with errorCode = the service's error code — which Cause 1 must be fixed for first.
