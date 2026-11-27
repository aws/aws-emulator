Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__kms.volume-kms-key-given-by-alias-or-id-is-reported-as-the-key-arn
Issue-Key: ec2__kms.volume-kms-key-given-by-alias-or-id-is-reported-as-the-key-arn|run#13|AssertionError|assert reported == {"alias": key_arn, "key id": key_arn}, (
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# ec2__kms: DescribeVolumes echoes the KmsKeyId reference form the request used instead of the resolved key ARN

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVolumes

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: a volume created with an alias or a bare key id reports that same string as KmsKeyId.
  AWS: recorded behavior only (the EC2 model types KmsKeyId as a string). CreateVolume resolves whatever key reference it is given — alias name, alias ARN, key id or key ARN — and the volume thereafter reports the resolved key ARN: 'arn:aws:kms:<region>:<account>:key/<key id>'. DescribeVolumes answers the same ARN regardless of the form the request used.
  Emulator: DescribeVolumes echoes the request's form — for the alias reference it answers 'alias/<name>' and for the bare id it answers '<key uuid>' — instead of the key ARN.
  Why: a client comparing a volume's KmsKeyId to a key ARN (the form every other AWS surface returns) never matches; tooling that keys resources by ARN mis-groups or fails to find the key.
  Fix: the ec2 CreateVolume/DescribeVolumes handlers' key-reference resolution — store the resolved key ARN in volume state and project that, not the request string.
  Traces: ec2__kms.volume-kms-key-given-by-alias-or-id-is-reported-as-the-key-arn.

## Evidence
ec2__kms.volume-kms-key-given-by-alias-or-id-is-reported-as-the-key-arn —
AssertionError: DescribeVolumes KmsKeyId per reference form: {'alias': 'alias/sim2real-ebskeyref-<id>', 'key id': 'af10315e-...'}, expected 'arn:aws:kms:us-west-2:<account>:key/af10315e-...'.

## Steps to Reproduce
1. Create a symmetric CMK and an alias for it.
2. ec2.create_volume(..., Encrypted=True, KmsKeyId='alias/<name>') and a second volume with KmsKeyId=<bare key uuid>.
3. ec2.describe_volumes for both.
4. Observe KmsKeyId echoed as the alias / bare id; AWS reports the resolved 'arn:aws:kms:<region>:<account>:key/<id>' for both.
