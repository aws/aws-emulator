Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates cognito-idp.e2e-user-import-job-csv-header-to-active-users
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates|_error#49|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: cognito-idp.e2e-user-import-job-csv-header-to-active-users|run#19|AssertionError|assert final_status["ImportedUsers"] == 1, (
Issue-Commit: cdaca303dd80edc32c77ea853d2e866117177c76

# cognito-idp: 2 net-new causes — a forgotten DEVICE_KEY is silently re-challenged instead of refused ResourceNotFoundException, and a user import job Succeeds with ImportedUsers=0 so the CSV row never becomes a user

> _Note: this issue was identified against the code base at commit `cdaca303dd80edc32c77ea853d2e866117177c76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Two net-new behavioral root causes in cognito-idp, each grounded in its own failing behavioral check. BOTH must be closed before submitting — they are numbered below and each carries its own failing check ids.

Neither is a wrongly-refused call that a fixed issue left refused, so neither is a REOPENS: in Cause 1 the emulator answers 200 where AWS refuses, and in Cause 2 nothing is refused at all (the job reports success while importing nothing).

=== Cause 1 of 2: after AdminForgetDevice, an InitiateAuth that still presents the forgotten DEVICE_KEY is answered with a fresh MFA challenge instead of being refused ResourceNotFoundException 'Device does not exist.' ===
AWS: a DEVICE_KEY is a reference to a stored device of that user, and AdminForgetDevice removes it. A later sign-in that still names it therefore references a resource the service cannot find: InitiateAuth(AuthFlow=USER_PASSWORD_AUTH, AuthParameters={USERNAME, PASSWORD, DEVICE_KEY=<the forgotten key>}) is REFUSED with Error.Code 'ResourceNotFoundException' and Error.Message 'Device does not exist.' — the whole sentence is fixed text, with no part taken from the request or from state (no device key, username or pool id is interpolated). Smithy: InitiateAuth declares ResourceNotFoundException among its errors (modeled @httpError 404; ResourceNotFoundException doc: "This exception is thrown when the Amazon Cognito service can't find the requested resource"), and the emulator already serves this same error class and sentence for the device-not-found path of GetDevice in this very check (both of that check's GetDevice refusal assertions — for a bogus key and for the key after AdminForgetDevice — PASS, including the exact message), so the status it serves there is the status due here. AWS does NOT silently downgrade the stale reference to a plain unremembered-device sign-in.
Emulator: that InitiateAuth answers HTTP 200 with ChallengeName='SOFTWARE_TOKEN_MFA', a Session and ChallengeParameters {USER_ID_FOR_SRP: <username>} — the forgotten DEVICE_KEY is simply ignored and the call falls back to the normal MFA challenge, so no refusal ever occurs. The surrounding legs pass, which localises this precisely: the same key was accepted twice earlier (DEVICE_SRP_AUTH was issued for it), and GetDevice on it after the forget correctly answers ResourceNotFoundException 'Device does not exist.' Only InitiateAuth's reading of the parameter diverges.
Why: a client cannot tell a revoked device from an unrecognised one. A fleet-management or device-revocation flow that forgets a device and expects the stale key to be rejected (so it can clear the key from local storage and re-register) instead sees an ordinary challenge and keeps presenting a dead key forever; conversely, security tooling that treats ResourceNotFoundException as proof the revocation took effect never gets that signal, so revocation is unverifiable through the API.
Fix: the device-tracking branch of the InitiateAuth handler (src/ops/initiate_auth.rs) and its AdminInitiateAuth sibling: a DEVICE_KEY present in AuthParameters must be RESOLVED against the user's stored devices (the per-device records AdminForgetDevice/ForgetDevice delete, read via the device state in src/state/device_auth.rs and the user's devices map in src/state.rs) and an unresolvable key must raise ResourceNotFoundException with the shared 'Device does not exist.' sentence the device read paths already use — not be dropped so the call continues as if no device key had been supplied.
Traces: cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates.

=== Cause 2 of 2: a started user import job reaches Succeeded but imports nothing — ImportedUsers stays 0 for a one-row CSV built from the pool's own GetCSVHeader columns, so the imported user never exists. ===
AWS: StartUserImportJob "instructs your user pool to start importing users from a CSV file that contains their usernames and attributes", and the job then reports what the run did: UserImportJobType.ImportedUsers is "the number of users that were successfully imported", SkippedUsers "the number of users that were skipped", FailedUsers "the number of users that couldn't be imported", with Status 'Succeeded' = "The job has completed successfully" (Smithy: UserImportJobType, UserImportJobStatusType = Created|Pending|InProgress|Stopping|Expired|Stopped|Failed|Succeeded). For a file whose header row is the pool's own GetCSVHeader column list (with the password_hash column removed, because no PasswordHashingAlgorithm was specified) and one data row carrying cognito:username, email and email_verified=TRUE, the job settles Status='Succeeded' with ImportedUsers=1, and the named user then exists in the pool with UserStatus='RESET_REQUIRED' (an imported user carries no verifiable password), becomes 'CONFIRMED' after AdminSetUserPassword(Permanent=True) and can sign in. Recorded behavior only for the duration of the Pending/InProgress transitions, which is why the check polls.
Emulator: DescribeUserImportJob settles Status='Succeeded' — so the header row WAS read (the emulator's own rule only marks a job Failed when the header is unreadable, src/state/user_import.rs ImportOutcome::terminal_status) — yet ImportedUsers is 0, the modeled default. The single data row is therefore never placed as a user, so the whole downstream sequence the check then exercises (AdminGetUser reporting RESET_REQUIRED, AdminSetUserPassword(Permanent=True) moving it to CONFIRMED, and ADMIN_USER_PASSWORD_AUTH signing the imported user in) is unreachable. Every call in the flow answers 200: GetCSVHeader, CreateUserImportJob, the PUT of the CSV to the job's PreSignedUrl and StartUserImportJob all succeed, and the counters are the only thing that betrays that nothing happened. Hypothesis (neither the model nor the recording states which branch is taken): the row is being charged to the FailedUsers/SkippedUsers branch of src/state/user_import.rs place_row rather than imported — the per-row gates there are the username column, the cognito:mfa_enabled boolean, attribute_value_defect and satisfies_schema_constraints, and the row's only non-blank attribute values are email and the verified flag email_verified='TRUE' (upper case), so the flag/boolean normalisation on the attribute path is the first thing to check; the job's SkippedUsers/FailedUsers counters identify the branch.
Why: the documented bulk-import path is a no-op that reports success. A client (or a migration runbook) that uploads a CSV, polls for Succeeded and then expects its users to exist gets a green job and an empty pool, with no error anywhere to react to — strictly worse than a Failed job, because the failure is silent and the counters are the only evidence.
Fix: the row-placement rule of the import — src/state/user_import.rs place_row (and the outcome accounting in run_user_import that feeds UserImportJob.ImportedUsers/SkippedUsers/FailedUsers), driven by src/ops/start_user_import_job.rs — must place a row whose columns come from this pool's own GetCSVHeader template (src/state/csv_header.rs) and whose values are a username plus the attributes AWS's CSV format carries, including the verified flags in the case the import template/documentation uses, as a user in RESET_REQUIRED; and the counters DescribeUserImportJob renders must reflect that run.
Traces: cognito-idp.e2e-user-import-job-csv-header-to-active-users.

## Evidence
Verbatim from the replay (expected vs actual), both checks listed under service heading `### cognito-idp`:

Cause 1 — cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates ([mismatch]):
  File "check_cognito_idp_e2e_device_remember_skips_mfa_then_forget_reinstates.py", line 257, in run
    reinstated_code, reinstated_message = _error(
  AssertionError: expected a ClientError, but the call succeeded and returned {'ChallengeName': 'SOFTWARE_TOKEN_MFA', 'Session': '<redacted per-run token>', 'ChallengeParameters': {'USER_ID_FOR_SRP': '<the username>'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  The call under assertion: initiate_auth(ClientId, AuthFlow='USER_PASSWORD_AUTH', AuthParameters={USERNAME, PASSWORD, DEVICE_KEY}) issued AFTER admin_forget_device(UserPoolId, Username, DeviceKey).
  Expected: a refusal with Error.Code 'ResourceNotFoundException' and Error.Message exactly 'Device does not exist.' (entirely fixed text — nothing interpolated), and no challenge.
  Actual:   HTTP 200 with ChallengeName='SOFTWARE_TOKEN_MFA' and a fresh Session.
  Preceding assertions in the same check that PASS and localise the cause: get_device on a bogus key -> ResourceNotFoundException 'Device does not exist.'; initiate_auth with this DEVICE_KEY before the forget -> ChallengeName='DEVICE_SRP_AUTH'; get_device on this DEVICE_KEY after the forget -> ResourceNotFoundException 'Device does not exist.' Only InitiateAuth ignores the stale key.

Cause 2 — cognito-idp.e2e-user-import-job-csv-header-to-active-users ([mismatch]):
  File "check_cognito_idp_e2e_user_import_job_csv_header_to_active_users.py", line 68, in run
    assert final_status["ImportedUsers"] == 1, (
  AssertionError: expected 1 imported user, job reported 0
  The preceding assertion `final_status["Status"] == "Succeeded"` PASSED, so the job terminated successfully with a header the import accepted; only the row count diverges.
  Request shape: GetCSVHeader -> CSVHeader; the header used is that list with every password_hash column removed; one data row with cognito:username=<generated name>, email=<that name>@example.com, email_verified='TRUE', all other columns blank; CreateUserImportJob(JobName, UserPoolId, CloudWatchLogsRoleArn) -> PreSignedUrl; PUT of the CSV body to that URL -> 200; StartUserImportJob -> 200; DescribeUserImportJob polled to a terminal status.
  Expected: UserImportJob.Status='Succeeded' with ImportedUsers=1 (and SkippedUsers=0, FailedUsers=0), then AdminGetUser(UserPoolId, Username) reporting UserStatus='RESET_REQUIRED'.
  Actual:   UserImportJob.Status='Succeeded' with ImportedUsers=0 (the modeled default), and the check stops there, so the RESET_REQUIRED -> CONFIRMED -> ADMIN_USER_PASSWORD_AUTH sign-in sequence is never observed.

Neither finding depends on a per-run value: pool ids, client ids, device keys, job ids, usernames, session tokens and timestamps are all generated per run and are not part of the requirement. The session token in the Cause 1 output above is a per-run value and is quoted only to show the shape of the wrong 200 response.

## Steps to Reproduce
Cause 1:
1. CreateUserPool with Policies.PasswordPolicy and DeviceConfiguration={'ChallengeRequiredOnNewDevice': True, 'DeviceOnlyRememberedOnUserPrompt': False}; SetUserPoolMfaConfig(MfaConfiguration='OPTIONAL', SoftwareTokenMfaConfiguration={'Enabled': True}); CreateUserPoolClient(GenerateSecret=False, ExplicitAuthFlows=['ALLOW_USER_PASSWORD_AUTH']).
2. Create a confirmed user with a permanent password; enrol TOTP (AssociateSoftwareToken + VerifySoftwareToken) and SetUserMFAPreference(SoftwareTokenMfaSettings={'Enabled': True, 'PreferredMfa': True}).
3. InitiateAuth(USER_PASSWORD_AUTH) -> SOFTWARE_TOKEN_MFA; answer it with a fresh code; read NewDeviceMetadata.DeviceKey/DeviceGroupKey.
4. ConfirmDevice(AccessToken, DeviceKey, DeviceName, DeviceSecretVerifierConfig={'PasswordVerifier','Salt'}) and confirm GetDevice(DeviceKey, AccessToken) resolves the device. Confirm InitiateAuth with that DEVICE_KEY is routed to ChallengeName='DEVICE_SRP_AUTH' (the positive control).
5. AdminForgetDevice(UserPoolId, Username, DeviceKey).
6. GetDevice(DeviceKey, AccessToken) -> ResourceNotFoundException 'Device does not exist.' (already correct).
7. InitiateAuth(ClientId, AuthFlow='USER_PASSWORD_AUTH', AuthParameters={USERNAME, PASSWORD, DEVICE_KEY=<the forgotten key>}). Expect the refusal Error.Code 'ResourceNotFoundException' with Error.Message exactly 'Device does not exist.'; observe HTTP 200 with ChallengeName='SOFTWARE_TOKEN_MFA'.

Cause 2:
1. CreateUserPool(AutoVerifiedAttributes=['email']). Create an IAM role trusted by cognito-idp.amazonaws.com with logs:CreateLogGroup/CreateLogStream/DescribeLogStreams/PutLogEvents on arn:aws:logs:<region>:<account>:log-group:/aws/cognito/*.
2. GetCSVHeader(UserPoolId) -> CSVHeader. Build a CSV whose header row is that list with every column containing 'password_hash' removed, and ONE data row with cognito:username=<a fresh name>, email=<name>@example.com, email_verified='TRUE' and every other column blank.
3. CreateUserImportJob(JobName, UserPoolId, CloudWatchLogsRoleArn=<that role>) -> JobId + PreSignedUrl (retry on PreconditionNotMetException while the role propagates).
4. PUT the CSV body to PreSignedUrl (expect 200), then StartUserImportJob(UserPoolId, JobId) (retry on PreconditionNotMetException).
5. Poll DescribeUserImportJob(UserPoolId, JobId) until Status is terminal. Expect Status='Succeeded' with ImportedUsers=1, SkippedUsers=0, FailedUsers=0; observe Status='Succeeded' with ImportedUsers=0. Record SkippedUsers/FailedUsers — they identify which per-row branch the row took.
6. AdminGetUser(UserPoolId, Username=<that name>) -> expect UserStatus='RESET_REQUIRED'; then AdminSetUserPassword(Permanent=True) -> expect 'CONFIRMED'; then AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) -> expect AuthenticationResult with a non-empty AccessToken.
