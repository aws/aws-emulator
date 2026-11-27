Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.data-key-pair-without-plaintext-ignores-malformed-grant-token-under-key-policy kms.derive-shared-secret-ignores-malformed-grant-token-under-key-policy kms.unissued-grant-token-does-not-authorize-data-key-generation kms.unissued-grant-token-does-not-authorize-sign-or-verify-mac kms.forged-grant-token-refused-for-caller-without-key-access kms.data-key-pair-dry-run-denied-until-granted kms.re-encrypt-from-an-rsa-ciphertext-to-a-symmetric-key-round-trips kms.retire-grant-by-the-retiring-principal
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|GenerateDataKeyPairWithoutPlaintext|InvalidGrantTokenException
Issue-Key: kms|DeriveSharedSecret|InvalidGrantTokenException
Issue-Key: kms|GenerateDataKey|InvalidGrantTokenException
Issue-Key: kms|Sign|InvalidGrantTokenException
Issue-Key: kms.forged-grant-token-refused-for-caller-without-key-access|_assert_denied#27|AssertionError|assert code == "AccessDeniedException", f"{what} answered {code!r}: {msg!r}"
Issue-Key: kms.data-key-pair-dry-run-denied-until-granted|run#23|AssertionError|assert (code, msg) == ("DryRunOperationException", _DRY_RUN_MESSAGE), (
Issue-Key: kms|ReEncrypt|InvalidCiphertextException
Issue-Key: kms.retire-grant-by-the-retiring-principal|run#22|AssertionError|assert observed[(who, grant)] == want, (
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms: 4 causes — unrecognized grant tokens are refused instead of ignored, the DryRun message is the shape doc, ReEncrypt cannot read an RSA ciphertext, and RetireGrant authorizes the wrong principals

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDataKey

## Description
Four distinct root causes, each grounded in failing behavioral checks.

Cause 1 of 4: a grant token the key never issued (or an unparseable one) is refused InvalidGrantTokenException instead of being ignored.
  AWS: Smithy: InvalidGrantTokenException is documented as 'The request was rejected because the specified grant token is not valid.' but AWS applies it to a token it cannot process as a token at all, not to every token it does not recognize. The recordings show: with the key policy already authorizing the operation, a call carrying an unparseable GrantTokens entry succeeds exactly as the tokenless call does (same DeriveSharedSecret shared secret, same GenerateDataKeyPairWithoutPlaintext result); and for a principal with no key-policy or IAM permission, a token the key never issued does NOT authorize the call — it is refused AccessDeniedException ('User: <caller> is not authorized to perform: kms:<Operation> on resource: <key arn> because no identity-based policy allows the kms:<Operation> action'), and the same call succeeds once the key owner's real grant token is presented.
  Emulator: every such call is refused 400 InvalidGrantTokenException 'The specified grant token is not valid.' — on GenerateDataKeyPairWithoutPlaintext, DeriveSharedSecret, GenerateDataKey, Sign, GenerateDataKeyWithoutPlaintext — so authorized callers are refused outright and unauthorized callers get the wrong error class.
  Why: a client that passes a cached, stale or opaque grant token alongside a request its key policy already allows is hard-refused by the emulator where AWS serves it; and a client distinguishing 'not allowed' (AccessDeniedException) from 'bad token' sees the wrong code.
  Fix: the KMS authorization path's grant-token handling (shared grant-token resolution used by the data-plane handlers): an unresolvable token must not fail the request — fall through to key-policy/IAM evaluation and deny with AccessDeniedException when nothing else authorizes it.
  Traces: kms.data-key-pair-without-plaintext-ignores-malformed-grant-token-under-key-policy, kms.derive-shared-secret-ignores-malformed-grant-token-under-key-policy, kms.unissued-grant-token-does-not-authorize-data-key-generation, kms.unissued-grant-token-does-not-authorize-sign-or-verify-mac, kms.forged-grant-token-refused-for-caller-without-key-access.

Cause 2 of 4: DryRunOperationException carries the Smithy shape's documentation text instead of AWS's runtime message.
  AWS: recorded behavior only for the message (Smithy documents the shape as 'The request was rejected because the DryRun parameter was specified.', which is the doc string, not the wire message). AWS answers a DryRun request that would have succeeded with 412 DryRunOperationException and the message 'The request would have succeeded, but the DryRun option is set.' — fixed text, no variable parts. (The already-fixed issue 20261117T122437Z-kms-4-net-new-causes-ml-kem-encapsulate.md records the same wording for Encapsulate/Decapsulate.)
  Emulator: GenerateDataKeyPair/GenerateDataKeyPairWithoutPlaintext with DryRun=true and an authorizing grant answer DryRunOperationException with 'The request was rejected because the DryRun parameter was specified.'
  Why: a client asserting on, logging or branching by KMS's dry-run message reads text AWS never sends for this error class.
  Fix: the shared DryRun refusal construction used by the KMS data-plane handlers (the error message, not the code).
  Traces: kms.data-key-pair-dry-run-denied-until-granted.

Cause 3 of 4: ReEncrypt cannot read a ciphertext made under an RSA key.
  AWS: Smithy: ReEncrypt takes SourceKeyId and SourceEncryptionAlgorithm (EncryptionAlgorithmSpec) precisely so an asymmetric source ciphertext can be decrypted; with those supplied, ReEncrypt moves an RSAES_OAEP_SHA_* ciphertext made under an RSA key to a symmetric destination key and the result decrypts to the original plaintext, answering SourceKeyId/KeyId and both algorithms.
  Emulator: the call is refused 400 InvalidCiphertextException with an empty message, so an asymmetric source ciphertext can never be re-encrypted.
  Why: a client rotating data off an RSA key onto a symmetric key through ReEncrypt — the documented path — cannot do it at all.
  Fix: the ReEncrypt handler's source-decryption path: honour SourceKeyId plus SourceEncryptionAlgorithm and decrypt asymmetric ciphertexts before re-wrapping under the destination key.
  Traces: kms.re-encrypt-from-an-rsa-ciphertext-to-a-symmetric-key-round-trips.

Cause 4 of 4: RetireGrant's authorization is wrong in both directions.
  AWS: Smithy: RetireGrant takes GrantToken or KeyId+GrantId and declares AccessDeniedException-class refusals. Recorded behavior: the grant's retiring principal and its grantee principal may retire the grant (the call succeeds), while an unrelated bystander in the account is refused AccessDeniedException 'User: <bystander session arn> is not authorized to perform: kms:RetireGrant on resource: <key arn> because no identity-based policy allows the kms:RetireGrant action' — fixed text with the session ARN and key ARN from the request/state.
  Emulator: inverted — the bystander's RetireGrant succeeds ('OK'), while the grantee and the retiring principal are refused AccessDeniedException with the 'on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access' wording.
  Why: a principal AWS denies can retire other principals' grants (a real access-control hole), and the principals AWS allows cannot retire their own grant, so grant lifecycle management fails.
  Fix: the RetireGrant handler's authorization — identify the grant's retiring and grantee principals and authorize exactly those (plus identity-based kms:RetireGrant), and use the identity-based denial wording for everyone else.
  Traces: kms.retire-grant-by-the-retiring-principal.

## Evidence
kms.data-key-pair-without-plaintext-ignores-malformed-grant-token-under-key-policy / kms.derive-shared-secret-ignores-malformed-grant-token-under-key-policy — botocore.errorfactory.InvalidGrantTokenException: An error occurred (InvalidGrantTokenException) when calling the GenerateDataKeyPairWithoutPlaintext / DeriveSharedSecret operation: The specified grant token is not valid. Expected: the call succeeds, returning the same result as the tokenless call.
kms.unissued-grant-token-does-not-authorize-data-key-generation / -sign-or-verify-mac — same InvalidGrantTokenException on GenerateDataKey / Sign; expected AccessDeniedException naming the caller, the key and the kms:<Operation> action.
kms.forged-grant-token-refused-for-caller-without-key-access — AssertionError: GenerateDataKeyWithoutPlaintext with a forged token answered 'InvalidGrantTokenException': 'The specified grant token is not valid.' (expected the AccessDeniedException denial).

kms.data-key-pair-dry-run-denied-until-granted — AssertionError: GenerateDataKeyPairWithoutPlaintext DryRun with a grant answered 'DryRunOperationException': 'The request was rejected because the DryRun parameter was specified.' Expected message: 'The request would have succeeded, but the DryRun option is set.'

kms.re-encrypt-from-an-rsa-ciphertext-to-a-symmetric-key-round-trips — botocore.errorfactory.InvalidCiphertextException: An error occurred (InvalidCiphertextException) when calling the ReEncrypt operation: (empty message).

kms.retire-grant-by-the-retiring-principal — AssertionError: RetireGrant of the first grant by the bystander: expected ('AccessDeniedException', 'User: <bystander session> is not authorized to perform: kms:RetireGrant on resource: <key arn> because no identity-based policy allows the kms:RetireGrant action'), got ('OK', None); every attempt: {('bystander','first'): ('OK', None), ('grantee','first'): ('AccessDeniedException', '... on this resource because the resource does not exist in this Region ...'), ('retirer','first'): ('AccessDeniedException', '...'), ('grantee','second'): ('OK', None)}.

## Steps to Reproduce
1. Cause 1: create a key whose policy authorizes the caller; call kms.derive_shared_secret(..., GrantTokens=['sim2real-not-a-key-agreement-grant']) and kms.generate_data_key_pair_without_plaintext(..., GrantTokens=[<unparseable>]) -> expect success; observe InvalidGrantTokenException. Then, as a role with no key access, call GenerateDataKey/Sign with a base64 token the key never issued -> expect AccessDeniedException; observe InvalidGrantTokenException.
2. Cause 2: grant a role the data-key-pair operations, then call GenerateDataKeyPair/…WithoutPlaintext with DryRun=True and the grant token -> expect DryRunOperationException 'The request would have succeeded, but the DryRun option is set.'
3. Cause 3: Encrypt under an RSA_2048 key with RSAES_OAEP_SHA_256, then kms.re_encrypt(CiphertextBlob=..., SourceKeyId=<rsa arn>, SourceEncryptionAlgorithm='RSAES_OAEP_SHA_256', DestinationKeyId=<symmetric arn>) -> expect 200; observe InvalidCiphertextException.
4. Cause 4: create a grant with a RetiringPrincipal and a GranteePrincipal; attempt RetireGrant as the retiring principal, the grantee, and an unrelated role -> expect the first two to succeed and the bystander to be denied; observe the inverse.
