Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-media.get-media-deleted-stream-rejected kinesis-video-media.get-media-continuation-token-invalid-rejected kinesis-video-media.get-media-unauthorized-role-denied-rejected
Issue-Shape-Id: com.amazonaws.kinesisvideomedia#AWSAcuityInletService
Issue-Key: kinesis-video-media.get-media-deleted-stream-rejected|run#12|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-continuation-token-invalid-rejected|run#5|AssertionError|assert err["Message"] == (
Issue-Key: kinesis-video-media.get-media-unauthorized-role-denied-rejected|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: 7846fec8a86647e77da10e6415de80579a2805a4

# kinesis-video-media GetMedia: 3 net-new root causes — deleted-stream ResourceNotFoundException wording, unresolvable-ContinuationToken InvalidArgumentException wording, and no IAM authorization (explicit deny never yields AccessDeniedException)

> _Note: this issue was identified against the code base at commit `7846fec8a86647e77da10e6415de80579a2805a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMedia

## Description
This issue bundles THREE distinct root causes in `kinesis-video-media:GetMedia`, grounded in the three failing behavioral checks that are NOT marked OWNED by the open issue `20261029T231704Z-kinesis-video-media-getmedia-rejection-fidelity-six-divergences-in-validationexception-invalidargumentexception.md` (that issue owns the other eight kinesis-video-media failures in this replay: the four StartSelector companion-field messages, the three ValidationException message/pattern divergences and the nonexistent-stream message). A fixer must close ALL THREE causes below before submitting.

Model used throughout: `services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/smithy-model/com-amazonaws-kinesisvideomedia-awsacuityinletservice.json`, operation `GetMedia` (POST /getMedia, success 200; errors ClientLimitExceededException 400, ConnectionLimitExceededException 400, InvalidArgumentException 400, InvalidEndpointException 400, NotAuthorizedException 401, ResourceNotFoundException 404).

=== Cause 1 of 3: the ResourceNotFoundException for a stream whose DeleteStream has completed uses the emulator's own wording and echoes the identifier ===
Failing check: kinesis-video-media.get-media-deleted-stream-rejected.
AWS: Smithy models `ResourceNotFoundException` with httpError 404 and the doc "Status Code: 404, The stream with the given name does not exist." The message text is recorded behavior only: for a stream that no longer resolves — here, one that was created, read successfully, then deleted via DeleteStream and polled until DescribeStream reported it gone — GetMedia answers HTTP 404 `ResourceNotFoundException` whose message is the single fixed sentence "The requested stream is not found or not active." The whole message is fixed text: there is NO variable part, AWS does not echo the requested StreamName or StreamARN, and this one sentence covers both the absent-stream and the exists-but-not-ACTIVE case.
Emulator: answers 404 `ResourceNotFoundException` (status and code are right) with "No such stream: <the StreamName or StreamARN carried in the request>." — different wording plus interpolation of the requested identifier.
Why: a client that matches, logs or branches on the documented sentence does not recognise the emulator's error; the emulator's variant also asserts a distinction (this named stream is absent) that AWS deliberately collapses with the not-active case, so a caller cannot tell from AWS that "deleted" and "not yet ACTIVE" are the same refusal while the emulator implies it can.
Fix: the `resource_not_found(..)` construction in the GetMedia handler's stream-resolution step — it must emit the fixed sentence with no interpolation. Note this is a DIFFERENT check from the nonexistent-stream check owned by the open issue above (that one never created the stream; this one proves a completed DeleteStream is a real state transition), so the same template must be fixed for the post-deletion path too, not just for a never-created name.
Traces: kinesis-video-media.get-media-deleted-stream-rejected; see the code citations under Evidence.

=== Cause 2 of 3: a ContinuationToken the service never issued is refused with the emulator's own message instead of AWS's ===
Failing check: kinesis-video-media.get-media-continuation-token-invalid-rejected.
AWS: `StartSelector$ContinuationToken` is modeled @length 1..128 @pattern ^[a-zA-Z0-9_\.\-]+$, so a token of the form the check sends satisfies the shape constraint and is NOT a ValidationException. Recorded behavior: AWS then fails to decode the token server-side and answers HTTP 400 `InvalidArgumentException` (the modeled 400 "The value for this input parameter is invalid") with the message "Illegal embedded sign character". The entire message is fixed text — no variable part, no echo of the token and no mention of the stream. It is a 400 argument fault, not a 404.
Emulator: answers 400 `InvalidArgumentException` (status and code are now right) with "Continuation token <the token from the request> does not resolve to a position on stream <the resolved stream's name from state>." — wording AWS never emits, with two interpolated variable parts where AWS has none.
Why: a client matching on the message text to distinguish "my saved token is stale/corrupt, restart from a fresh selector" from other InvalidArgumentException causes (all of which share one error code) sees text that never occurs on AWS; the emulator additionally leaks the token and the stream name into an error channel AWS keeps free of request/state values.
Fix: the CONTINUATION_TOKEN branch of the GetMedia handler's selector resolution — the `invalid_argument(..)` call site there must render AWS's fixed sentence. Context for the fixer: the open issue named above described this check as its Cause 5 when the call still answered 404 because the stream itself did not resolve; with stream resolution repaired (fixed issue `20261022T180000Z-kinesis-video-media-getmedia-cannot-see-a-stream-createstream-made.md`) the call now reaches the token-decode step and the residual divergence is the message, which is why the suite no longer maps the check onto that issue's replay key and it is filed here.
Traces: kinesis-video-media.get-media-continuation-token-invalid-rejected.

=== Cause 3 of 3: GetMedia enforces no IAM authorization, so an explicit deny never produces AccessDeniedException ===
Failing check: kinesis-video-media.get-media-unauthorized-role-denied-rejected (verdict mismatch, "Check timed out after 60.0s").
AWS: authorization is evaluated before the operation runs. Recorded behavior: a role holding an inline Allow for `kinesisvideo:GetMedia` can call GetMedia successfully (HTTP 200); once an inline Deny for the SAME action is attached to the SAME role, the same assumed-role caller is refused with the IAM-layer error code `AccessDeniedException`, surfaced ahead of the stream-level modeled `NotAuthorizedException` (401, "Status Code: 403, The caller is not authorized to perform an operation on the given stream, or the token has expired."). The check asserts on the error CODE, so the requirement is the code `AccessDeniedException`; the message/status of the deny leg are recorded behavior only and are not asserted.
Emulator: caller identity is ignored entirely — the handler's own module doc records that the authorization (`NotAuthorizedException`) behaviour is "not modelled here" — so attaching the deny changes nothing, the check's bounded poll for `AccessDeniedException` never converges, and the check exhausts its 60s budget. The timeout is the emulator's: had the deny been honoured the poll's first probe would have returned the code immediately and the check would have finished in seconds (its positive-control leg, which only needs GetMedia to succeed, does converge at once).
Why: this is an authorization bypass, the most damaging class here. A caller validating least-privilege policy, or a test proving a deny is effective, gets a successful GetMedia from the emulator and concludes the deny works when it does not — media a policy forbids is served to any identity that can reach the data endpoint.
Fix: the request-authorization path in front of this operation's handler (the shared IAM-evaluation middleware the data-plane services use), so GetMedia refuses a caller whose effective policy denies `kinesisvideo:GetMedia` with `AccessDeniedException`, and reserves the modeled `NotAuthorizedException` (401) for the stream-level/expired-token case. The open issue named above lists this check among its trace ids as its Cause 6, but the suite reports the check as not owned by it (its recorded replay key no longer matches this failure), so the cause is filed here with its current signature.
Traces: kinesis-video-media.get-media-unauthorized-role-denied-rejected.

## Evidence
All evidence is from the replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout; kinesis-video-media scored 3/14 (21%), 11 failed, of which 8 are marked OWNED by 20261029T231704Z-kinesis-video-media-getmedia-rejection-fidelity-six-divergences-in-validationexception-invalidargumentexception.md and skipped.

Cause 1 — GetMedia(StreamName=<stream created, read OK, then DeleteStream completed>, StartSelector={'StartSelectorType':'EARLIEST'}): HTTP 404 and Code 'ResourceNotFoundException' are correct; message diverges.
  expected: "The requested stream is not found or not active."   (entire sentence is fixed text)
  actual:   "No such stream: <requested stream name>."            (AssertionError: unexpected ResourceNotFoundException message: 'No such stream: sim2real-behav-kvm-de21076c8fc9.')
  The quoted name is that run's generated stream name and is not part of the requirement — the requirement is that no identifier is interpolated at all.

Cause 2 — GetMedia(StreamName=<an ACTIVE stream>, StartSelector={'StartSelectorType':'CONTINUATION_TOKEN','ContinuationToken':<a token matching ^[a-zA-Z0-9_\.\-]+$ that the service never issued>}): HTTP 400 and Code 'InvalidArgumentException' are correct; message diverges.
  expected: "Illegal embedded sign character"   (entire message is fixed text)
  actual:   "Continuation token <token> does not resolve to a position on stream <stream name>."   (AssertionError: unexpected InvalidArgumentException message: 'Continuation token sim2real-bogus-continuation-token-000 does not resolve to a position on stream sim2real-behav-kvm-4621787f2e70.')
  The quoted token and stream name are that run's values; the requirement is that neither appears.

Cause 3 — role trusting the account root, inline Allow kinesisvideo:GetMedia on '*', assumed via sts:AssumeRole, then an inline Deny for the same action on the same role:
  expected: the assumed-role caller's GetMedia succeeds before the deny, and its error Code becomes 'AccessDeniedException' after the deny is attached.
  actual:   the error code never becomes 'AccessDeniedException' — the call behaves identically before and after the deny, identity-independently — and the check is killed by its budget: "Check timed out after 60.0s".

Code citations (orientation only), services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/src/ops/get_media.rs:
  lines 175-182 — the `resource_not_found` helper; lines 454-461 — stream resolution emitting Cause 1's "No such stream: {}." message.
  lines 166-172 — the `invalid_argument` helper; lines 512-520 — the CONTINUATION_TOKEN branch emitting Cause 2's "Continuation token {token} does not resolve to a position on stream {}." message.
  lines 22-28 — the module doc stating the authorization (`NotAuthorizedException`) behaviour is not modelled (Cause 3).

## Steps to Reproduce
Boot the emulator from this checkout. For each cause, obtain a data endpoint with kinesisvideo.get_data_endpoint(APIName='GET_MEDIA', StreamName=<name>) and build a kinesis-video-media client on it.

Shared setup: kinesisvideo.create_stream(StreamName=<name>, DataRetentionInHours=1); poll kinesisvideo.describe_stream until StreamInfo.Status == 'ACTIVE'.

Cause 1: confirm get_media(StreamName=<name>, StartSelector={'StartSelectorType':'EARLIEST'}) returns 200. Then kinesisvideo.delete_stream(StreamARN=<arn>) and poll describe_stream until it raises ResourceNotFoundException. Call get_media again with the same name and the same (now stale) data endpoint: it answers 404 ResourceNotFoundException with Message 'No such stream: <name>.' instead of 'The requested stream is not found or not active.'

Cause 2: on an ACTIVE stream call get_media(StreamName=<name>, StartSelector={'StartSelectorType':'CONTINUATION_TOKEN','ContinuationToken':'sim2real-bogus-continuation-token-000'}): it answers 400 InvalidArgumentException with Message 'Continuation token ... does not resolve to a position on stream ...' instead of the fixed 'Illegal embedded sign character'.

Cause 3: iam.create_role(<role trusting arn:aws:iam::<account>:root>); iam.put_role_policy(<inline Allow for kinesisvideo:GetMedia on '*'>); sts.assume_role and build a kinesis-video-media client with those session credentials against the same data endpoint; confirm get_media succeeds. Then iam.put_role_policy(<inline Deny for kinesisvideo:GetMedia on '*'>) on the same role and poll get_media with the same credentials: the error code never becomes 'AccessDeniedException', so the poll runs to its bound and the check times out.
