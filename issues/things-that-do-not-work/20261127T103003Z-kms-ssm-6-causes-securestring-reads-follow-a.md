Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__ssm.securestring-stays-under-its-original-key-after-the-alias-moves kms__ssm.securestring-version-stays-on-its-key-after-alias-retarget kms__ssm.securestring-versions-written-through-a-retargeted-alias-keep-their-own-keys kms__ssm.securestring-ciphertext-decrypt-naming-retargeted-alias-rejected kms__ssm.put-securestring-without-kms-encrypt-on-the-key-rejected kms__ssm.securestring-under-an-asymmetric-or-hmac-key-refused-as-a-client-error kms__ssm.advanced-securestring-is-an-envelope-whose-data-key-only-the-chosen-key-opens kms__ssm.put-parameter-key-id-outside-one-to-256-chars-rejected kms__ssm.session-log-in-s3-is-encrypted-under-the-bucket-default-key
Issue-Key: ssm|GetParameter|InvalidKeyId
Issue-Key: kms__ssm.put-securestring-without-kms-encrypt-on-the-key-rejected|_denial#25|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {accepted!r}")
Issue-Key: ssm|PutParameter|InternalServerError
Issue-Key: kms__ssm.advanced-securestring-is-an-envelope-whose-data-key-only-the-chosen-key-opens|_parse#13|AssertionError|assert kind == 0x80, f"version 1 message type must be 0x80; got {kind:#x}"
Issue-Key: kms__ssm.put-parameter-key-id-outside-one-to-256-chars-rejected|run#22|AssertionError|assert over == (
Issue-Key: kms__ssm.session-log-in-s3-is-encrypted-under-the-bucket-default-key|__init__#17|WebSocketBadStatusException|self.ws = websocket.create_connection(
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms__ssm: 6 causes — SecureString reads follow a retargeted alias instead of the encrypting key, kms:Encrypt is not authorized, asymmetric keys 500, the advanced envelope byte is wrong, validation messages elide the value, and the Session Manager stream is unroutable

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Six distinct root causes, grounded in nine failing behavioral checks.

Cause 1 of 6: a SecureString is decrypted under the alias's CURRENT target rather than the key that encrypted the version, so every read after an alias retarget fails.
  AWS: recorded behavior only. A SecureString written through an alias is encrypted under the key the alias named at write time, and the ciphertext records that key. Moving the alias to another key (UpdateAlias) does not change existing versions: GetParameter(WithDecryption=True) keeps returning the stored value, each version stays readable under its own key, and a read that explicitly names the retargeted alias as the decryption key is refused as not the key that can open that ciphertext.
  Emulator: the plain GetParameter(WithDecryption=True) read itself raises 400 InvalidKeyId 'the value of <parameter arn> is not a ciphertext <key arn> opens' — the emulator resolves the alias at read time and then fails against the wrong key, so the parameter becomes unreadable after any alias move.
  Why: data written under an alias is permanently lost to the client as soon as the alias is retargeted — a routine key-rotation step — instead of staying readable under the key that encrypted it.
  Fix: the ssm parameter state and GetParameter decryption path — record the resolved key id on each SecureString version at write time and decrypt under that, never re-resolving the alias at read time.
  Traces: kms__ssm.securestring-stays-under-its-original-key-after-the-alias-moves, kms__ssm.securestring-version-stays-on-its-key-after-alias-retarget, kms__ssm.securestring-versions-written-through-a-retargeted-alias-keep-their-own-keys, kms__ssm.securestring-ciphertext-decrypt-naming-retargeted-alias-rejected.

Cause 2 of 6: PutParameter of a SecureString is served for a caller with no kms:Encrypt on the key.
  AWS: recorded behavior only. The write encrypts under the named CMK as the caller, so a role without kms:Encrypt is refused ('AccessDeniedException', 'User: <session arn> is not authorized to perform: kms:Encrypt on resource: <key arn> because no identity-based policy allows the kms:Encrypt action') — fixed text with the session and key ARNs from the request — and no parameter is left behind (GetParameter -> ('ParameterNotFound', '')). After kms:Encrypt is granted the identical write succeeds at version 1.
  Emulator: the write succeeds, answering {'Version': 1, 'Tier': 'Standard'}.
  Why: a caller AWS denies can write encrypted parameters; the identity-based boundary on the key has no effect and the expected AccessDeniedException never appears.
  Fix: the ssm PutParameter SecureString path — perform/authorize the kms:Encrypt call as the caller and surface KMS's denial.
  Traces: kms__ssm.put-securestring-without-kms-encrypt-on-the-key-rejected.

Cause 3 of 6: a SecureString under an asymmetric or HMAC key fails with a server error instead of a client refusal.
  AWS: recorded behavior only. PutParameter of a SecureString naming an RSA_2048 key is refused 400 ('InvalidKeyId', 'Invalid keyId.') — fixed text, no variable parts.
  Emulator: it answers ('InternalServerError', 'the value could not be encrypted under <key arn>') — a 500-class error with emulator-authored prose.
  Why: a client sees a retryable server fault for a permanent, caller-side configuration error; retries never succeed and the error class is wrong.
  Fix: the ssm PutParameter SecureString path's key validation — classify an unusable key spec as the client-side InvalidKeyId refusal.
  Traces: kms__ssm.securestring-under-an-asymmetric-or-hmac-key-refused-as-a-client-error.

Cause 4 of 6: the advanced-tier SecureString envelope's message-type byte is wrong.
  AWS: recorded behavior only. An advanced SecureString's stored value is an envelope whose version-1 message-type byte is 0x80 (the data key is wrapped for the chosen CMK and only that key opens it).
  Emulator: the first version's message type byte is 0x24.
  Why: a client parsing the envelope (as the AWS Encryption SDK does) cannot read it — the format differs from the one AWS writes.
  Fix: the ssm advanced-tier SecureString envelope serializer.
  Traces: kms__ssm.advanced-securestring-is-an-envelope-whose-data-key-only-the-chosen-key-opens.

Cause 5 of 6: constraint-violation messages elide the offending value instead of echoing it.
  AWS: Smithy: PutParameter.KeyId targets ParameterKeyId with @length min 1 max 256 and @pattern ^([a-zA-Z0-9:/_-]+)$. A 257-character KeyId is refused ('ValidationException', "1 validation error detected: Value '<the whole 257-character value>' at 'keyId' failed to satisfy constraint: Member must have length less than or equal to 256") — the value appears in full; an empty KeyId is refused with the two-error message naming the pattern and the minimum length.
  Emulator: the message truncates the value at roughly 256 characters and appends an elision marker — "Value 'alias/sim2real-behav-kid-…kkkk… [257 bytes]' at 'keyId' failed to satisfy constraint: Member must have length less than or equal to 256".
  Why: a client or operator matching/diagnosing the refusal gets a message AWS never sends, and the rejected value is not recoverable from it.
  Fix: the shared validation-message formatter used by the ssm handlers — echo the offending value verbatim with no length budget.
  Traces: kms__ssm.put-parameter-key-id-outside-one-to-256-chars-rejected.

Cause 6 of 6: the Session Manager data-plane stream is not routable, so an S3 session log is never produced.
  AWS: recorded behavior only. A started session's StreamUrl accepts a WebSocket handshake (HTTP 101) on the session data-plane endpoint, and the session's log is then written to the configured S3 bucket under the bucket's default key.
  Emulator: the handshake is refused HTTP 404 with x-amzn-errortype UnknownService and body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the router cannot resolve the stream URL to a service, so no session can be opened.
  Why: no client can open a Session Manager session, so nothing that depends on session output (including encrypted S3 session logs) works.
  Fix: the request router's handling of the session data-plane WebSocket endpoint (the ssmmessages/stream route the StreamUrl points at).
  Traces: kms__ssm.session-log-in-s3-is-encrypted-under-the-bucket-default-key.

## Evidence
kms__ssm.securestring-stays-under-its-original-key-after-the-alias-moves / -version-stays-on-its-key-after-alias-retarget / -versions-written-through-a-retargeted-alias-keep-their-own-keys / -ciphertext-decrypt-naming-retargeted-alias-rejected — botocore.errorfactory.InvalidKeyId: An error occurred (InvalidKeyId) when calling the GetParameter operation: the value of arn:aws:ssm:us-west-2:<account>:parameter/<name> is not a ciphertext arn:aws:kms:us-west-2:<account>:key/<id> opens. Expected: the stored value is returned, decrypted under the key that encrypted that version.

kms__ssm.put-securestring-without-kms-encrypt-on-the-key-rejected — AssertionError: the call was expected to be refused but returned {'Version': 1, 'Tier': 'Standard', 'HTTPStatusCode': 200}. Expected ('AccessDeniedException', 'User: <session arn> is not authorized to perform: kms:Encrypt on resource: <key arn> because no identity-based policy allows the kms:Encrypt action') and GetParameter -> ('ParameterNotFound', '').

kms__ssm.securestring-under-an-asymmetric-or-hmac-key-refused-as-a-client-error — AssertionError: PutParameter SecureString under a RSA_2048 key must fail with ('InvalidKeyId', 'Invalid keyId.'); got ('InternalServerError', 'the value could not be encrypted under <key arn>').

kms__ssm.advanced-securestring-is-an-envelope-whose-data-key-only-the-chosen-key-opens — AssertionError: version 1 message type must be 0x80; got 0x24.

kms__ssm.put-parameter-key-id-outside-one-to-256-chars-rejected — AssertionError: a 257-character KeyId answered ('ValidationException', "1 validation error detected: Value 'alias/sim2real-behav-kid-…kkkk… [257 bytes]' at 'keyId' failed to satisfy constraint: Member must have length less than or equal to 256") — the value is elided where AWS echoes all 257 characters.

kms__ssm.session-log-in-s3-is-encrypted-under-the-bucket-default-key — websocket._exceptions.WebSocketBadStatusException: Handshake status 404 Not Found ... {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}.

## Steps to Reproduce
1. Cause 1: create keys A and B and an alias pointing at A; ssm.put_parameter(Name=..., Type='SecureString', KeyId=<alias>, Value=v1); kms.update_alias to B; ssm.get_parameter(Name=..., WithDecryption=True) -> expect v1; observe InvalidKeyId 'is not a ciphertext ... opens'.
2. Cause 2: as a role without kms:Encrypt on the key, put a SecureString -> expect the kms:Encrypt AccessDeniedException and no parameter; observe {'Version': 1}.
3. Cause 3: put a SecureString naming an RSA_2048 key -> expect ('InvalidKeyId', 'Invalid keyId.'); observe InternalServerError.
4. Cause 4: put an advanced-tier SecureString and inspect the stored envelope's version-1 message-type byte -> expect 0x80; observe 0x24.
5. Cause 5: put a SecureString with a 257-character KeyId -> expect the full value echoed in the ValidationException message.
6. Cause 6: ssm.start_session and open the returned StreamUrl as a WebSocket -> expect HTTP 101; observe 404 UnknownService.
