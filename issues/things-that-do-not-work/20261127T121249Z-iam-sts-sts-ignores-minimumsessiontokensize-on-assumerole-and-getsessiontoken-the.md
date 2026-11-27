Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sts.assume-role-minimum-session-token-size-pads-the-issued-token iam__sts.get-session-token-minimum-session-token-size-boundary-enforced
Issue-Key: iam__sts.assume-role-minimum-session-token-size-pads-the-issued-token|run#10|AssertionError|assert got == (2048, 50), (
Issue-Key: iam__sts.get-session-token-minimum-session-token-size-boundary-enforced|run#6|AssertionError|assert padded["SessionTokenSize"] == 4096, (
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# sts ignores MinimumSessionTokenSize on AssumeRole and GetSessionToken — the issued token is never padded and SessionTokenSize/SessionTokenUtilization report the unpadded token (2 causes)

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
Two net-new causes, one per credential-issuing operation: the modeled `MinimumSessionTokenSize` input is accepted and then discarded.

Cause 1 of 2: AssumeRole does not pad the session token to the requested floor.
AWS: Smithy: sts#AssumeRole input member `MinimumSessionTokenSize` (minimumSessionTokenSizeType, @range min 0 max 4096); output members `SessionTokenSize` (sessionTokenSizeType, @range min 0) and `SessionTokenUtilization` (sessionTokenUtilizationType, @range min 0), the latter replacing the deprecated `PackedPolicySize`. Recorded behavior: a request carrying MinimumSessionTokenSize=N has its session token PADDED so that `SessionTokenSize` is EXACTLY N, and `SessionTokenUtilization` is that size as a whole-number percentage of the 4096-byte maximum (N=2048 → 50). The padded token still authenticates as the assumed role: GetCallerIdentity with it returns the AssumedRoleUser.Arn of the issuing response. A request WITHOUT the member is not padded, so its SessionTokenSize stays below the floor.
Emulator: the request is accepted but the token is unchanged — (SessionTokenSize, SessionTokenUtilization) is (712, 17) for MinimumSessionTokenSize=2048, i.e. the unpadded token's own size and its utilization of the maximum. The requirement is the relation (size == the requested floor; utilization == round(size/4096*100)), not the particular numbers of one run.
Why: the member exists so a client can reserve session-token room before adding session policies/tags, and so proxies and middleboxes can be sized for the largest token the client will ever present. Silently returning a smaller token means a client that sized its buffers, headers or downstream limits from the response under-provisions and later fails when a real token grows; and SessionTokenUtilization — the signal that replaced PackedPolicySize for capacity planning — reports a figure for a token that was never issued.
Fix: the sts AssumeRole handler's credential-minting step — pad the session token to MinimumSessionTokenSize and compute SessionTokenSize/SessionTokenUtilization from the token actually issued.
Traces: iam__sts.assume-role-minimum-session-token-size-pads-the-issued-token.

Cause 2 of 2: GetSessionToken does not pad the session token to the requested floor.
AWS: Smithy: sts#GetSessionToken input member `MinimumSessionTokenSize` (@range min 0 max 4096); output `SessionTokenSize` and `SessionTokenUtilization`. Recorded behavior: MinimumSessionTokenSize=4096 pads the token to EXACTLY 4096 bytes with SessionTokenUtilization 100 (the maximum is fully used), while a request without the member stays below 4096.
Emulator: SessionTokenSize=712 for MinimumSessionTokenSize=4096 — the unpadded token.
Why: same as cause 1, at the boundary where it matters most: a client asking for the maximum token size gets a token one sixth of it and a utilization figure that does not describe any issued credential.
Fix: the sts GetSessionToken handler's credential-minting step — the same padding and the same derivation of the two output members.
Traces: iam__sts.get-session-token-minimum-session-token-size-boundary-enforced.

## Evidence
Cause 1 — expected (SessionTokenSize, SessionTokenUtilization) == (2048, 50) for MinimumSessionTokenSize=2048. Actual, verbatim from the replay: 'AssertionError: a MinimumSessionTokenSize=2048 request must pad the token to exactly 2048 bytes at SessionTokenUtilization 50, got (SessionTokenSize, SessionTokenUtilization)=(712, 17)'.

Cause 2 — expected SessionTokenSize == 4096 (and SessionTokenUtilization == 100) for MinimumSessionTokenSize=4096. Actual, verbatim: 'AssertionError: requesting MinimumSessionTokenSize=4096 must pad the token to exactly 4096 bytes, got SessionTokenSize=712'.

712 is the emulator's unpadded token size in these runs and is not itself a requirement; the requirement is that the padded size equals the requested floor and that SessionTokenUtilization is that size as a percentage of 4096.

## Steps to Reproduce
Cause 1: create a role assumable by the caller. Call AssumeRole with no MinimumSessionTokenSize and note SessionTokenSize < 2048. Call AssumeRole again with MinimumSessionTokenSize=2048 — expect SessionTokenSize 2048 and SessionTokenUtilization 50; observe (712, 17). Use the returned credentials for GetCallerIdentity and expect the AssumedRoleUser.Arn of the issuing response.
Cause 2: create an IAM user with a long-term access key. Call GetSessionToken(DurationSeconds=900) and note SessionTokenSize < 4096. Call GetSessionToken(DurationSeconds=900, MinimumSessionTokenSize=4096) — expect SessionTokenSize 4096 and SessionTokenUtilization 100; observe 712.
