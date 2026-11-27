Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__cognito-idp.admin-created-users-count-as-sign-ups-under-the-admin-client cloudwatch__cognito-idp.sign-in-naming-an-unknown-app-client-counts-under-the-invalid-client cloudwatch__cognito-idp.user-authentication-calls-are-counted-in-usage-call-count
Issue-Key: cloudwatch__cognito-idp.admin-created-users-count-as-sign-ups-under-the-admin-client|_poll_until#16|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last {last!r}")
Issue-Key: cloudwatch__cognito-idp.sign-in-naming-an-unknown-app-client-counts-under-the-invalid-client|_poll_until#17|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last {last!r}")
Issue-Key: cloudwatch__cognito-idp.user-authentication-calls-are-counted-in-usage-call-count|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudwatch__cognito-idp: 3 net-new causes — AdminCreateUser publishes no SignUpSuccesses under UserPoolClient=Admin, an unknown-app-client AdminInitiateAuth publishes no zero SignInSuccesses under UserPoolClient=Invalid, and sign-ins are never counted in the AWS/Usage UserAuthentication CallCount

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminCreateUser

## Description
Three independent gaps in the AWS/Cognito and AWS/Usage metric emission of Cognito user pools. All three checks pass their control assertions (the pool, client and users are created, the self-service SignUp and the real client's sign-ins DO publish their own series), so the divergence is confined to the three series below.

Cause 1 of 3: administrative user creation is not counted as a sign-up.
  AWS: Cognito publishes AWS/Cognito SignUpSuccesses for a user pool under the dimension pair (UserPool, UserPoolClient); a user created by an administrative call has no app client, so it is counted under the literal UserPoolClient value 'Admin'. Two AdminCreateUser calls therefore produce SignUpSuccesses Sum 2, SampleCount 2 under UserPoolClient=Admin, beside the self-service SignUp counted under the app client's own id. Recorded behavior only for the series' existence (the metric is not in the Smithy model; the dimension vocabulary is Cognito's documented metrics-for-user-pools set).
  Emulator: the self-service SignUp's series under the real client id appears with (1.0, 1.0) — the positive control — but GetMetricStatistics for SignUpSuccesses with Dimensions [(UserPool, <pool>), (UserPoolClient, 'Admin')] returns no datapoints at all.
  Why: a client that watches administrative provisioning (onboarding automation, bulk account creation) sees no activity, so dashboards and alarms on the Admin dimension read zero while users are being created.
  Fix: the AdminCreateUser handler's metric emission — publish SignUpSuccesses under (UserPool, UserPoolClient='Admin').
  Traces: cloudwatch__cognito-idp.admin-created-users-count-as-sign-ups-under-the-admin-client ('SignUpSuccesses under UserPoolClient=Admin not met within 30.0s; last None').

Cause 2 of 3: a sign-in naming an app client the pool does not have is not metered under UserPoolClient=Invalid.
  AWS: an AdminInitiateAuth naming an unknown client id is refused (ResourceNotFoundException, message 'User pool client <clientId> does not exist.') AND is published as a SignInSuccesses sample of value 0 under the dimension pair (UserPool, UserPoolClient='Invalid') — Sum 0, SampleCount 1 — beside the real client's successful sign-in sample; the raw unknown client id is never used as a dimension value.
  Emulator: the refusal itself matches (the check asserts code and message before the metric read and passes it), and the real client's SignInSuccesses sample is present (control), but no datapoint is ever published under UserPoolClient='Invalid'.
  Why: the operational signal for clients calling with a wrong/retired app client id is absent, so a misconfigured caller is invisible in metrics and the documented 'Invalid' bucket cannot be alarmed on.
  Fix: the AdminInitiateAuth/InitiateAuth refusal path's metric emission — publish the zero-valued SignInSuccesses sample under UserPoolClient='Invalid' (never under the raw client id).
  Traces: cloudwatch__cognito-idp.sign-in-naming-an-unknown-app-client-counts-under-the-invalid-client ('SignInSuccesses under UserPoolClient=Invalid not met within 30.0s; last None').

Cause 3 of 3: user-pool authentication calls are not counted in the account-level AWS/Usage CallCount.
  AWS: besides the pool's own SignInSuccesses, each sign-in is counted in the account-level AWS/Usage CallCount metric under the dimensions Service='Cognito User Pool', Type='API', Resource='UserAuthentication', Class='None'; three InitiateAuth calls make that Sum at least 3.
  Emulator: the check cannot get past this read and is cut at the 60 s runner cap — the AWS/Usage CallCount series for those dimensions never reaches the driven count (the pool-level SignInSuccesses series is the control the same check reads first).
  Why: service-quota and usage dashboards and the quota-based alarms AWS documents on AWS/Usage read nothing for Cognito authentication, so a client cannot see or alarm on its request usage at all.
  Fix: the user-pool authentication path must also emit the AWS/Usage CallCount datum under the four documented dimensions, alongside the per-pool metric.
  Traces: cloudwatch__cognito-idp.user-authentication-calls-are-counted-in-usage-call-count (timed out at the runner cap on the AWS/Usage CallCount poll).

No open issue covers these: 20261127T155721Z-cognito-idp-iam-logs-cognito-idp-a-user-import-job-never-reaches.md concerns user-import jobs and CloudWatch Logs delivery, a different operation and plane, and carries none of these replay keys.

## Evidence
Suite: service cloudwatch__cognito-idp scored 3/6 (50%), 3 failed; none is OWNED or ALREADY FILED.

[mismatch] cloudwatch__cognito-idp.admin-created-users-count-as-sign-ups-under-the-admin-client
  AssertionError: SignUpSuccesses under UserPoolClient=Admin not met within 30.0s; last None
  Control that passed: SignUpSuccesses for the app client == (Sum 1.0, SampleCount 1.0) after one self-service SignUp.
  Expected (AWS): GetMetricStatistics(Namespace=AWS/Cognito, MetricName=SignUpSuccesses, Dimensions=[UserPool=<pool>, UserPoolClient=Admin]) -> (Sum 2.0, SampleCount 2.0) for two AdminCreateUser calls.
  Actual (emulator): no datapoints.

[mismatch] cloudwatch__cognito-idp.sign-in-naming-an-unknown-app-client-counts-under-the-invalid-client
  AssertionError: SignInSuccesses under UserPoolClient=Invalid not met within 30.0s; last None
  Controls that passed: the unknown-client AdminInitiateAuth was refused ('ResourceNotFoundException', 'User pool client <clientId> does not exist.'), and the real client's SignInSuccesses == (1.0, 1.0).
  Expected (AWS): a datapoint under UserPoolClient=Invalid with (Sum 0.0, SampleCount 1.0), and nothing under the raw unknown client id.
  Actual (emulator): no datapoint under Invalid.

[mismatch] cloudwatch__cognito-idp.user-authentication-calls-are-counted-in-usage-call-count
  Check timed out after 60.0s polling GetMetricStatistics(Namespace=AWS/Usage, MetricName=CallCount, Dimensions=[Service='Cognito User Pool', Type='API', Resource='UserAuthentication', Class='None']) for Sum >= 3 after three InitiateAuth calls.

Pool ids, client ids and user names in the recorded runs are per-run values; the requirements are the namespaces, metric names, dimension names and the literal dimension values 'Admin', 'Invalid', 'Cognito User Pool', 'API', 'UserAuthentication' and 'None'.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateUserPool and CreateUserPoolClient.
3. Cause 1: call AdminCreateUser twice (MessageAction=SUPPRESS) and SignUp once through the app client; then GetMetricStatistics(AWS/Cognito, SignUpSuccesses, [UserPool=<pool>, UserPoolClient=Admin], Period 3600, Statistics [Sum, SampleCount]) -> AWS (2.0, 2.0); emulator no datapoints (while the app-client series shows (1.0, 1.0)).
4. Cause 2: AdminSetUserPassword a user permanently, AdminInitiateAuth once with the real client (succeeds) and once with a client id the pool does not have (refused ResourceNotFoundException 'User pool client <id> does not exist.'); then GetMetricStatistics(AWS/Cognito, SignInSuccesses, [UserPool=<pool>, UserPoolClient=Invalid]) -> AWS (Sum 0.0, SampleCount 1.0); emulator no datapoints. Also confirm nothing is published under the raw unknown client id.
5. Cause 3: InitiateAuth three times with USER_PASSWORD_AUTH; then GetMetricStatistics(AWS/Usage, CallCount, [Service='Cognito User Pool', Type='API', Resource='UserAuthentication', Class='None'], Period 3600, Statistics [Sum]) -> AWS Sum >= 3; emulator no datapoints.
6. Re-replay all three checks in trace_ids to confirm the fix.
