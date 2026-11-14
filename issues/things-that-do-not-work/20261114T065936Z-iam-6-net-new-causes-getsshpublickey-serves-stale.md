Issue-Source: trace-sweep-divergence
Issue-Trace: iam-ssh-public-key-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-ssh-public-key-lifecycle iam-credential-not-found-errors iam-ssh-public-key-guard-clauses iam-cred-enable-mfa-wrong-code-invalid-authentication-code iam-ebecs-events-role-run-task-pass-role-simulation
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetSSHPublicKey|4xx->2xx|$
Issue-Key: iam|GetSSHPublicKey|4xx->2xx|$.ErrorResponse
Issue-Key: iam|GetSSHPublicKey|4xx->2xx|status
Issue-Key: iam|GetSSHPublicKey|2xx->2xx|$.GetSSHPublicKeyResponse.GetSSHPublicKeyResult.SSHPublicKey.SSHPublicKeyBody
Issue-Key: iam|GetSSHPublicKey|2xx->2xx|$.GetSSHPublicKeyResponse.GetSSHPublicKeyResult.SSHPublicKey.Status
Issue-Key: iam|GetSSHPublicKey|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|EnableMFADevice|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|SimulatePrincipalPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 7722f7a491f17adc9c16c0d1d844ff39f4bc9907

# iam: 6 net-new causes — GetSSHPublicKey serves stale SSH-public-key state and credential refusal messages are emulator prose

> _Note: this issue was identified against the code base at commit `7722f7a491f17adc9c16c0d1d844ff39f4bc9907`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSSHPublicKey

## Description
Six net-new root causes on the IAM credential surface, all found by replaying trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score 89.0%). They share one fault: the emulator substitutes its OWN state and its OWN prose for AWS's for IAM credential reads and refusals — the SSH-public-key record it serves does not reflect the deletes and status updates applied to it or AWS's canonical body form, and four refusal/lookup messages are emulator-authored text rather than the fixed text AWS sends for that error class. A fixer must close ALL SIX causes before submitting this issue; each cause names its own fixture and step.

=== Cause 1 of 6: GetSSHPublicKey still serves a DELETED SSH public key with 200 instead of refusing it 404 NoSuchEntity ===
AWS: Smithy iam#DeleteSSHPublicKey(UserName required, SSHPublicKeyId required) -> Unit, with NoSuchEntityException (awsQueryError NoSuchEntity, httpResponseCode 404) as its ONLY modelled error; iam#GetSSHPublicKey(UserName, SSHPublicKeyId, Encoding) -> SSHPublicKey, errors NoSuchEntityException (NoSuchEntity, 404) and UnrecognizedPublicKeyEncodingException (UnrecognizedPublicKeyEncoding, 400). Once the key is deleted it is no longer a credential of that user, so a GetSSHPublicKey naming that id is refused HTTP 404 with Error.Type 'Sender', Error.Code 'NoSuchEntity' and the message 'The Public Key with id <SSHPublicKeyId> cannot be found.' — fixed text 'The Public Key with id ' ... ' cannot be found.' with the id taken verbatim from the request (the recorded id value itself is a per-run value, not a requirement).
Emulator: the same GetSSHPublicKey after the delete answers HTTP 200 carrying the whole SSHPublicKey record (UserName, SSHPublicKeyId, Fingerprint, SSHPublicKeyBody, Status 'Active', UploadDate) and no ErrorResponse at all; the harness re-polled 30 times and the key never disappeared.
Why: deletion of an SSH credential is unobservable, so credential revocation silently does not take effect — a client that deletes a key and then verifies it is gone (the standard rotation/teardown check) concludes the key is still installed and usable, and never converges.
Fix: DeleteSSHPublicKey's state mutation on the user's SSH-public-key collection, and the lookup GetSSHPublicKey performs against it (the handler must raise the modelled NoSuchEntityException for a removed id). Open issue 20261111T103332Z-iam-deletesshpublickey-stub.md covers implementing the operation and 20261111T010549Z-iam-ssh-public-key-credentials-deleted-keys-stay.md covers the ListSSHPublicKeys projection of the same stale state; NEITHER carries this GetSSHPublicKey cluster's replay key (the replay reports owned_by empty, stub_path empty), so this cause must also be closed here.
Traces: iam-ssh-public-key-lifecycle#task#8.

=== Cause 2 of 6: GetSSHPublicKey reports Status 'Active' after UpdateSSHPublicKey set the key Inactive ===
AWS: Smithy iam#UpdateSSHPublicKey(UserName required, SSHPublicKeyId required, Status required -> statusType enum Active|Inactive|Expired) -> Unit, errors InvalidInputException (InvalidInput, 400) and NoSuchEntityException (NoSuchEntity, 404). iam#SSHPublicKey.Status is a required member documented 'Active means that the key can be used for authentication with an CodeCommit repository. Inactive means that the key cannot be used.' After a successful UpdateSSHPublicKey with Status=Inactive, GetSSHPublicKey for that id reports Status 'Inactive'.
Emulator: GetSSHPublicKey answers 200 with Status 'Active' — the status write never reached the record the read projects.
Why: a client that disables an SSH credential (instead of deleting it) cannot tell the key was disabled, and anything gating on Status keeps treating a deactivated key as usable; the enable/disable half of the credential lifecycle has no observable effect.
Fix: UpdateSSHPublicKey's handler/state write for the SSH-public-key record (shared record with cause 1). Open issue 20261112T141524Z-iam-updatesshpublickey-stub.md covers implementing the operation; it does not carry this GetSSHPublicKey cluster's replay key, so this cause's step must be replayed green here.
Traces: iam-ssh-public-key-lifecycle#task#6.

=== Cause 3 of 6: GetSSHPublicKey echoes the uploaded key's trailing comment in SSHPublicKeyBody instead of AWS's canonical body ===
AWS: recorded behavior only — the model says iam#SSHPublicKey.SSHPublicKeyBody is required, publicKeyMaterialType, @length 1-16384, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$, documented merely as 'The SSH public key'. The recording shows that for key material uploaded in OpenSSH form '<keytype> <base64-blob> <comment>' and read back with Encoding=SSH, AWS returns only the '<keytype> <base64-blob>' pair: the trailing comment token is not part of the returned body. Hypothesis: AWS stores the decoded public-key blob and re-renders the body canonically, so the comment is dropped on ingest.
Emulator: returns the request's body verbatim, including the trailing comment token, so the body read back differs from AWS's by that extra ' <comment>' suffix.
Why: a client comparing the body it gets back against the canonical key (its authorized_keys entry, or the output of ssh-keygen on the same material) gets a byte mismatch and concludes a different key is installed; byte-for-byte round-trip comparisons of the credential never match AWS.
Fix: the SSHPublicKey wire projection shared by GetSSHPublicKey (and the body normalization applied when UploadSSHPublicKey stores the material).
Traces: iam-ssh-public-key-lifecycle#task#3 (the same body divergence also appears in cause 2's cluster, iam-ssh-public-key-lifecycle#task#6).

=== Cause 4 of 6: the NoSuchEntity message for an unknown SSH public key says 'The SSH Public Key' where AWS says 'The Public Key' ===
AWS: Smithy iam#GetSSHPublicKey models NoSuchEntityException (awsQueryError NoSuchEntity, httpResponseCode 404). Recorded: an SSHPublicKeyId that names no key for the user is refused HTTP 404, Error.Code 'NoSuchEntity', with the message 'The Public Key with id <SSHPublicKeyId> cannot be found.' — fixed text 'The Public Key with id ' and ' cannot be found.', the only variable part being the id copied from the request.
Emulator: status 404 and code NoSuchEntity are correct, but the message reads 'The SSH Public Key with id <SSHPublicKeyId> cannot be found.' — the fixed text carries an extra 'SSH ' that AWS's message for this error class does not.
Why: the error message is a requirement of the error class; a client or operator surface that matches AWS's wording (log matching, user-facing display of the IAM message, fixtures asserting the refusal text) does not match.
Fix: the GetSSHPublicKey handler's key-lookup refusal message template (code and status already correct).
Traces: iam-credential-not-found-errors#task#4 (cluster of 2, also iam-ssh-public-key-guard-clauses#task#2).

=== Cause 5 of 6: EnableMFADevice's InvalidAuthenticationCode message is emulator prose ===
AWS: Smithy iam#EnableMFADevice(UserName, SerialNumber, AuthenticationCode1, AuthenticationCode2) models InvalidAuthenticationCodeException with awsQueryError code 'InvalidAuthenticationCode' and httpResponseCode 403. Recorded: a wrong authentication-code pair is refused HTTP 403, Error.Code 'InvalidAuthenticationCode', message 'Authentication code for device is not valid.' — the whole string is fixed text with no variable part.
Emulator: status 403 and code InvalidAuthenticationCode are correct, but the message is 'The authentication code was not recognized or has expired. Please try again with a new code.' — different fixed text, and it additionally asserts an expiry that AWS does not state for this class.
Why: the message is a requirement of the error class; a client matching AWS's wording never matches, and the emulator's variant mis-describes why the call failed.
Fix: the EnableMFADevice handler's authentication-code validation error construction (message template only). The behavioral issue 20261113T094431Z-iam-5-net-new-causes-resyncmfadevice-and-senddelegationtoken.md describes the same wording defect under its Cause 3, but carries only behavioral check keys — the replay reports this wire cluster as owned_by empty, so its replay key is filed here.
Traces: iam-cred-enable-mfa-wrong-code-invalid-authentication-code#task#3.

=== Cause 6 of 6: SimulatePrincipalPolicy's InvalidInput message for a malformed ActionNames entry is emulator prose and leaks the action name ===
AWS: Smithy iam#SimulatePrincipalPolicy(PolicySourceArn required, ActionNames required, ...) models InvalidInputException (awsQueryError InvalidInput, httpResponseCode 400); ActionNames is documented 'A list of names of API operations to evaluate in the simulation. Each operation is evaluated for each resource. Each operation must include the service identifier, such as iam:CreateUser.' Recorded: an ActionNames entry that is not of the form <service>:<action> is refused HTTP 400 with Error.Code 'InvalidInput' and the message 'The action is invalid since it is missing names' — fixed text with NO variable part; the offending action name does not appear in the message.
Emulator: status 400 and code InvalidInput are correct, but the message is 'Invalid Action name: <ActionName> must name a service and an action, as in iam:CreateUser' — entirely different fixed text, and it interpolates the request's action name, which AWS's message for this class does not.
Why: the message is a requirement of the error class; a client matching AWS's wording fails, and the emulator's variant reflects request input back in a message AWS keeps value-free.
Fix: the SimulatePrincipalPolicy input-validation path's InvalidInputException construction for malformed action names (message template only).
Traces: iam-ebecs-events-role-run-task-pass-role-simulation#task#5 (cluster of 2, also iam-ebecs-events-role-run-task-pass-role-simulation#task#14).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, steps_total=1695, steps_passed=1508, steps_mismatched=186, divergence_count=74, score_pct=89.0. All six clusters below report stub_path empty and owned_by empty, i.e. no open issue carries their replay keys.

Cause 1 — iam-ssh-public-key-lifecycle#task#8, GetSSHPublicKey, expected status 404, got 200:
  status: expected 404, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'NoSuchEntity', 'Message': 'The Public Key with id <SSHPublicKeyId> cannot be found.'}, ...}, got None)
  $: poll: error code None not in ['NoSuchEntity'] after 30 attempt(s) — the emulator kept answering GetSSHPublicKeyResponse with SSHPublicKey{Status: 'Active', ...} for the deleted id.
  Expected: 404 / NoSuchEntity / 'The Public Key with id <SSHPublicKeyId> cannot be found.'

Cause 2 — iam-ssh-public-key-lifecycle#task#6, GetSSHPublicKey, 200 vs 200:
  $.GetSSHPublicKeyResponse.GetSSHPublicKeyResult.SSHPublicKey.Status: value_mismatch (expected 'Inactive', got 'Active')

Cause 3 — iam-ssh-public-key-lifecycle#task#3, GetSSHPublicKey, 200 vs 200:
  $.GetSSHPublicKeyResponse.GetSSHPublicKeyResult.SSHPublicKey.SSHPublicKeyBody: value_mismatch — expected 'ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB...otbzF', got the identical body with a trailing ' <comment>' token appended (the recording's comment token is a per-run probe name; the requirement is that no comment token is returned). The same divergence recurs in cause 2's cluster (task#6).

Cause 4 — iam-credential-not-found-errors#task#4, GetSSHPublicKey, 404 vs 404 (affected_count=2, also iam-ssh-public-key-guard-clauses#task#2):
  $.ErrorResponse.Error.Message: value_mismatch (expected 'The Public Key with id <id> cannot be found.', got 'The SSH Public Key with id <id> cannot be found.')

Cause 5 — iam-cred-enable-mfa-wrong-code-invalid-authentication-code#task#3, EnableMFADevice, 403 vs 403:
  $.ErrorResponse.Error.Message: value_mismatch (expected 'Authentication code for device is not valid.', got 'The authentication code was not recognized or has expired. Please try again with a new code.')

Cause 6 — iam-ebecs-events-role-run-task-pass-role-simulation#task#5, SimulatePrincipalPolicy, 400 vs 400 (affected_count=2, also #task#14):
  $.ErrorResponse.Error.Message: value_mismatch (expected 'The action is invalid since it is missing names', got 'Invalid Action name: <ActionName> must name a service and an action, as in iam:CreateUser')

Model grounding (smithy_query on services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json):
  GetSSHPublicKey(UserName req, SSHPublicKeyId req @length 20-128 @pattern ^[\w]+$, Encoding req enum SSH|PEM) -> SSHPublicKey; errors NoSuchEntityException(NoSuchEntity,404), UnrecognizedPublicKeyEncodingException(UnrecognizedPublicKeyEncoding,400).
  DeleteSSHPublicKey(UserName req, SSHPublicKeyId req) -> Unit; errors NoSuchEntityException(NoSuchEntity,404) only.
  UpdateSSHPublicKey(UserName req, SSHPublicKeyId req, Status req -> statusType enum [Active, Inactive, Expired]) -> Unit; errors InvalidInputException(InvalidInput,400), NoSuchEntityException(NoSuchEntity,404).
  SSHPublicKey.Status required ('Active means that the key can be used ...; Inactive means that the key cannot be used'); SSHPublicKeyBody required publicKeyMaterialType @length 1-16384 ('The SSH public key').
  EnableMFADevice errors include InvalidAuthenticationCodeException(InvalidAuthenticationCode,403).
  SimulatePrincipalPolicy errors InvalidInputException(InvalidInput,400), NoSuchEntityException(404), PolicyEvaluationException(500); ActionNames doc: 'Each operation must include the service identifier, such as iam:CreateUser.'

## Steps to Reproduce
1. git_clone the repo and build/boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay iam-ssh-public-key-lifecycle step task#8 — CreateUser, UploadSSHPublicKey, DeleteSSHPublicKey, then GetSSHPublicKey(UserName, that SSHPublicKeyId, Encoding=SSH): the emulator answers 200 with the key instead of 404 NoSuchEntity 'The Public Key with id <id> cannot be found.'
4. Cause 2: replay iam-ssh-public-key-lifecycle step task#6 — after UpdateSSHPublicKey(Status=Inactive), read GetSSHPublicKeyResult.SSHPublicKey.Status; it is still 'Active'.
5. Cause 3: replay iam-ssh-public-key-lifecycle step task#3 — upload key material in OpenSSH form with a trailing comment token, then GetSSHPublicKey with Encoding=SSH and compare SSHPublicKeyBody: the emulator returns the comment token, AWS returns only '<keytype> <base64-blob>'.
6. Cause 4: replay iam-credential-not-found-errors step task#4 (or iam-ssh-public-key-guard-clauses#task#2) — GetSSHPublicKey with an SSHPublicKeyId that names no key and read $.ErrorResponse.Error.Message.
7. Cause 5: replay iam-cred-enable-mfa-wrong-code-invalid-authentication-code step task#3 — EnableMFADevice for a valid user/serial with a wrong authentication-code pair and read $.ErrorResponse.Error.Message on the 403 InvalidAuthenticationCode.
8. Cause 6: replay iam-ebecs-events-role-run-task-pass-role-simulation step task#5 (or #task#14) — SimulatePrincipalPolicy with an ActionNames entry that is not <service>:<action> and read $.ErrorResponse.Error.Message on the 400 InvalidInput.
