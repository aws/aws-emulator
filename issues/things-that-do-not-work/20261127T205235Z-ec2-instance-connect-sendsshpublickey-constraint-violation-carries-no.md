Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap
Issue-Step: task#22
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2-instance-connect|SendSSHPublicKey|4xx->4xx|$.Message
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ec2-instance-connect SendSSHPublicKey constraint violation carries no Message member

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendSSHPublicKey

## Description
Cause 1 of 1: SendSSHPublicKey refuses a too-short SSHPublicKey with the right status but an empty body — the constraint-violation Message is not serialised.
AWS: Smithy: SendSSHPublicKey.SSHPublicKey is @required with @length {min: 80, max: 4096} (and InstanceId @length 10-32 @pattern '^i-[a-f0-9]+$', InstanceOSUser @length 1-32). A value shorter than the minimum is refused 400 with a constraint-violation body whose Message is 'N validation error detected: Value '<supplied value>' at '<memberName-lowerCamel>' failed to satisfy constraint: Member must have length greater than or equal to <min>'. Fixed text: '<N> validation error detected: Value ', ' at ', ' failed to satisfy constraint: Member must have length greater than or equal to ', with the variable parts taken from the request (the rejected value), the model (the member name as the wire spells it — 'sSHPublicKey' — and the @length minimum, 80) and the count of violations.
Emulator: 400 with no Message member in the body at all, so the refusal carries no reason.
Why: an SDK surfaces the modeled exception's Message to the caller; with no Message the client is told only 'bad request' and cannot tell a malformed key from a wrong instance id, an unsupported OS user or a stopped instance — all of which are 400-class refusals on this operation.
Fix: the SendSSHPublicKey handler's input-constraint validation and the error serializer for ec2-instance-connect (emit the modeled constraint-violation Message alongside the 400).
Traces: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#task#22.

## Evidence
SendSSHPublicKey, expected 400 / actual 400: $.Message missing — expected "1 validation error detected: Value 'ssh-rsa not-a-key' at 'sSHPublicKey' failed to satisfy constraint: Member must have length greater than or equal to 80", got None. The quoted key text is the recording's request value filling the template; the fixed text, the member name 'sSHPublicKey' and the minimum 80 (Smithy: SSHPublicKey @length min 80) are the requirement.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Fixture ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap, step task#22: call ec2-instance-connect SendSSHPublicKey with SSHPublicKey='ssh-rsa not-a-key' (17 characters, below the modeled minimum of 80).
3. Observe HTTP 400 with no Message member instead of the constraint-violation message.
