Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.albos-health-check-matcher-code-above-499-rejected ec2__elbv2.albos-health-check-path-with-a-space-rejected ec2__elbv2.asghc-delete-target-group-in-use-by-a-listener-rejected
Issue-Key: ec2__elbv2.albos-health-check-matcher-code-above-499-rejected|run#13|AssertionError|assert got_absent == want_absent, f"the refused group must not exist, describe returned {got_absent!r}, expected {want_absent!r}"
Issue-Key: ec2__elbv2.albos-health-check-path-with-a-space-rejected|run#12|AssertionError|assert absent_message == "One or more target groups not found", f"MSG={absent_message!r}"
Issue-Key: ec2__elbv2.asghc-delete-target-group-in-use-by-a-listener-rejected|run#18|AssertionError|assert msg == expected_gone_msg, f"gone message: expected {expected_gone_msg!r}, got {msg!r}"
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# elbv2: the TargetGroupNotFound refusal message interpolates the requested identifiers instead of AWS's fixed "One or more target groups not found"

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetGroups

## Description
Cause 1 of 1: DescribeTargetGroups refuses an unknown target group with the right error code but a message built from the request instead of AWS's fixed sentence.

AWS: Smithy (com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): DescribeTargetGroups declares error TargetGroupNotFoundException, errorKind client, httpError 400, awsQueryError {code: TargetGroupNotFound, httpResponseCode: 400}. The model does not carry the message text, so the message is recorded behavior only: for any DescribeTargetGroups lookup that resolves no target group — whether the caller passed TargetGroupArns or Names, one identifier or several — the Query-protocol error body carries Code `TargetGroupNotFound` and the message `One or more target groups not found`. That sentence is entirely fixed text: it has NO variable part, and in particular it never names the ARN(s) or name(s) that were asked for.

Emulator: answers the correct code and status (TargetGroupNotFound, 400) but a templated message that interpolates the request's identifiers, of the form `Target groups '[<comma-separated identifiers as sent>]' not found`. Replayed, with a name identifier: `Target groups '[sim2real-albos-c26294390915]' not found`; with an ARN identifier: `Target groups '[arn:aws:elasticloadbalancing:<region>:<account>:targetgroup/<name>/<id>]' not found`. Only the fixed-text part diverges — the bracketed list is the request echo, the region/account/name/id inside it are per-run values and are not the defect.

Why: a client that grades a deletion or a negative control on ELBv2's refusal text — the suite's "the refused/deleted group must be gone" assertions are exactly that pattern, and so is any script or test harness matching on the message AWS documents — reads a message AWS never emits and concludes the API behaved differently. Because the emulator additionally leaks the caller's identifiers back into the message, any consumer that logs or compares refusal strings sees a per-request string where AWS guarantees a constant one.

Fix: the elbv2 DescribeTargetGroups handler's not-found refusal path — the TargetGroupNotFoundException construction / message builder used by the target-group lookup (shared by lookups by ARN and by Names) — must emit the fixed sentence with no request interpolation.

Traces: ec2__elbv2.albos-health-check-matcher-code-above-499-rejected (name identifier), ec2__elbv2.albos-health-check-path-with-a-space-rejected (name identifier), ec2__elbv2.asghc-delete-target-group-in-use-by-a-listener-rejected (ARN identifier, after a successful DeleteTargetGroup).

## Evidence
ec2__elbv2.albos-health-check-matcher-code-above-499-rejected:
  expected ('TargetGroupNotFound', 'One or more target groups not found')
  got      ('TargetGroupNotFound', "Target groups '[sim2real-albos-c26294390915]' not found")

ec2__elbv2.albos-health-check-path-with-a-space-rejected:
  assert absent_message == "One or more target groups not found"
  AssertionError: MSG="Target groups '[sim2real-albos-67a4858d363a]' not found"

ec2__elbv2.asghc-delete-target-group-in-use-by-a-listener-rejected:
  gone message: expected 'One or more target groups not found',
  got "Target groups '[arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-asghc-fc5b6acf7497/a0026fd72c756e61]' not found"

In all three the HTTP status (400) and the error code (TargetGroupNotFound) match AWS; only the message's fixed text diverges.

## Steps to Reproduce
1. CreateTargetGroup, then DeleteTargetGroup it (or simply pick a target-group name/ARN that was never created).
2. Call elbv2 DescribeTargetGroups with that identifier — once via Names=[<name>] and once via TargetGroupArns=[<arn>].
3. Read the error: AWS answers HTTP 400, Code TargetGroupNotFound, Message "One or more target groups not found" for both forms; the emulator answers the same status and code with Message "Target groups '[<identifier>]' not found".
