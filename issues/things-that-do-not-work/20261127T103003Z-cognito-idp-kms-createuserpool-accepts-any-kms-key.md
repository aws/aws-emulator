Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__kms.user-pool-asymmetric-key-rejected-multi-region-key-serves-sign-in cognito-idp__kms.user-pool-key-configuration-alias-arn-rejected cognito-idp__kms.user-pool-with-a-key-cognito-may-not-use-rejected
Issue-Key: cognito-idp__kms.user-pool-asymmetric-key-rejected-multi-region-key-serves-sign-in|_refused#24|AssertionError|raise AssertionError(f"CreateUserPool accepted {what} for encryption at rest")
Issue-Key: cognito-idp__kms.user-pool-key-configuration-alias-arn-rejected|_refusal#17|AssertionError|raise AssertionError("CreateUserPool accepted an alias ARN as KeyConfiguration.KmsKeyArn")
Issue-Key: cognito-idp__kms.user-pool-with-a-key-cognito-may-not-use-rejected|_refused_create#5|AssertionError|raise AssertionError(f"CreateUserPool was expected to be refused but created {made['Name']!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# cognito-idp__kms: CreateUserPool accepts any KMS key for encryption at rest — asymmetric keys, alias ARNs and keys Cognito may not use

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPool

## Description
Three distinct root causes, each grounded in a failing behavioral check. All three are the KeyConfiguration.KmsKeyArn admission check on CreateUserPool, but each is a separate rule with its own refusal.

Cause 1 of 3: an asymmetric key is accepted as the pool's encryption key.
  AWS: recorded behavior only (the cognito-idp model types KmsKeyArn as an ARN string). CreateUserPool resolves the key in KMS and requires KeySpec SYMMETRIC_DEFAULT, refusing otherwise with 400 InvalidParameterException 'KMS Key <key arn> does not meet expected KeySpec:SYMMETRIC_DEFAULT.' — fixed text with <key arn> taken from the request — and no pool is created. (The same check also refuses a key in another Region with 'KMS Key <key arn> does not exist'.)
  Emulator: the create succeeds and a user pool is returned.
  Why: a pool is created against a key that can never generate a data key, so encryption at rest is silently broken instead of the configuration being rejected.
  Fix: the CreateUserPool handler's KeyConfiguration validation (resolve the key in KMS, enforce the symmetric key spec and the same-Region rule).
  Traces: cognito-idp__kms.user-pool-asymmetric-key-rejected-multi-region-key-serves-sign-in.

Cause 2 of 3: an alias ARN is accepted where only a key ARN is allowed.
  AWS: recorded behavior only. CreateUserPool refuses an alias ARN with 400 InvalidParameterException 'KMS key <alias arn> is an alias. Specify the ARN of the KMS key instead.' — fixed text with the alias ARN from the request.
  Emulator: the alias ARN is accepted as KeyConfiguration.KmsKeyArn.
  Why: a client that supplies an alias ARN is not told to use the key ARN; the stored configuration names a resource AWS would never store, so later reads and key resolution diverge.
  Fix: the CreateUserPool handler's KeyConfiguration validation (reject alias ARNs before resolution).
  Traces: cognito-idp__kms.user-pool-key-configuration-alias-arn-rejected.

Cause 3 of 3: a key whose policy does not let Cognito use it is accepted.
  AWS: recorded behavior only. CreateUserPool performs a kms:DescribeKey as the Cognito service principal and, when the key policy does not allow it, refuses with 400 InvalidParameterException 'Cognito is not authorized to perform: kms:DescribeKey on KMS Key <key arn> because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access' — fixed text with <key arn> from the request — and no pool is created.
  Emulator: the pool is created ('sim2real-xsvc-cmk-denied-<id>').
  Why: a client gets a pool bound to a key the service cannot use; the authorization failure that AWS reports at create time never surfaces.
  Fix: the CreateUserPool handler's KeyConfiguration validation — evaluate the key policy for the Cognito service principal before accepting the key.
  Traces: cognito-idp__kms.user-pool-with-a-key-cognito-may-not-use-rejected.

## Evidence
cognito-idp__kms.user-pool-asymmetric-key-rejected-multi-region-key-serves-sign-in — AssertionError: CreateUserPool accepted an asymmetric key for encryption at rest. Expected ('InvalidParameterException', 'KMS Key <key arn> does not meet expected KeySpec:SYMMETRIC_DEFAULT.').

cognito-idp__kms.user-pool-key-configuration-alias-arn-rejected — AssertionError: CreateUserPool accepted an alias ARN as KeyConfiguration.KmsKeyArn. Expected ('InvalidParameterException', 'KMS key <alias arn> is an alias. Specify the ARN of the KMS key instead.').

cognito-idp__kms.user-pool-with-a-key-cognito-may-not-use-rejected — AssertionError: CreateUserPool was expected to be refused but created 'sim2real-xsvc-cmk-denied-3fc85be347c0'. Expected ('InvalidParameterException', 'Cognito is not authorized to perform: kms:DescribeKey on KMS Key <key-arn> because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access').

## Steps to Reproduce
1. Cause 1: create an RSA_2048 key; cognito-idp.create_user_pool(PoolName=..., KeyConfiguration={'KmsKeyArn': <rsa key arn>}) -> expect 400 InvalidParameterException naming the expected KeySpec; observe 200.
2. Cause 2: create a symmetric key with an alias; pass the alias ARN as KeyConfiguration.KmsKeyArn -> expect the 'is an alias' refusal; observe 200.
3. Cause 3: create a symmetric key whose key policy omits cognito-idp.amazonaws.com; pass its key ARN -> expect the kms:DescribeKey authorization refusal; observe a created pool.
