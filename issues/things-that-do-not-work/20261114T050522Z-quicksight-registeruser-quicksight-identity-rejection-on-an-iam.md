Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.register-user-quicksight-identity-lifecycle
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight.register-user-quicksight-identity-lifecycle|run#22|AssertionError|assert native_err["Message"] == (
Issue-Commit: abcf655460e962663cf1a42e4ceed79db956738d

# quicksight RegisterUser: QUICKSIGHT-identity rejection on an IAM_ONLY account returns a generic AccessDeniedException boilerplate instead of AWS's "Account <id> is not authorized to create QuickSight IdentityType user." message

> _Note: this issue was identified against the code base at commit `abcf655460e962663cf1a42e4ceed79db956738d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterUser

## Description
Cause 1 of 1: RegisterUser's AccessDeniedException message for a QUICKSIGHT IdentityType registration on an account whose subscription AuthenticationMethod is IAM_ONLY is emulator-authored generic credential boilerplate, not the specific message AWS returns for this error class. Failing check: quicksight.register-user-quicksight-identity-lifecycle.

AWS: Smithy (services/service-com-amazonaws-quicksight-quicksight-20180401) models RegisterUser (POST /accounts/{AwsAccountId}/namespaces/{Namespace}/users) with error AccessDeniedException, errorKind client, httpError 401, and requires IdentityType (enum IdentityType: IAM|QUICKSIGHT). The model does not state the message text, so this is recorded behavior: when the account's QuickSight subscription uses IAM_ONLY authentication, registering a native QUICKSIGHT identity is refused with Code AccessDeniedException and a message whose FIXED text is "Account " ... " is not authorized to create QuickSight IdentityType user." immediately followed, with no separating space, by the standard QuickSight error suffix "AWS account id: " ... ", Namespace: " ... ". " (note the trailing space after the final period). Its VARIABLE parts are taken from the request: the 12-digit AwsAccountId (appearing twice — once after "Account " and once after "AWS account id: ") and the Namespace path label. No user is created, so a subsequent DescribeUser for that UserName must answer ResourceNotFoundException.

Emulator: RegisterUser with IdentityType=QUICKSIGHT does return Code AccessDeniedException, but the message is the unrelated generic text "You don't have access to this item. The provided credentials couldn't be validated. You might not be authorized to carry out the request. Make sure that your account is authorized to use the Amazon Quick Sight service, that your policies have the correct permissions, and that you are using the correct credentials." — it names neither the account id, nor the namespace, nor the QuickSight IdentityType rule that caused the refusal, and it omits the "AWS account id: ..., Namespace: ..." suffix entirely.

Why: the refusal reason is carried only in the message for this error class — AWS reuses AccessDeniedException for credential/authorization failures too. A client (or an operator reading logs) that distinguishes "this account's authentication method forbids native QuickSight identities, register an IAM identity instead" from "my credentials are bad / my IAM policy is missing quicksight:RegisterUser" by matching on the message gets the wrong diagnosis and retries or re-credentials instead of switching IdentityType. Tests and provisioning tools that assert on the message fail against the emulator while passing against AWS.

Fix: the RegisterUser handler's identity-type validation path in the quicksight service crate — where the account subscription's AuthenticationMethod is checked against the requested IdentityType — must raise AccessDeniedException with the account-and-namespace-templated message above, built from the request's AwsAccountId and Namespace, rather than falling through to the service's generic access-denied text. Note this is a different root cause from the already-open message-fidelity issues 20261111T073736Z-quicksight-resourcenotfoundexception-messages-for-namespaces-and-themes-are.md, 20261103T040231Z-quicksight-data-set-resourcenotfoundexception-message-is-invented-wording.md and 20261104T051804Z-quicksight-createnamespace-duplicate-name-conflictexception-message-is-emulator.md: none of them covers RegisterUser or AccessDeniedException.

Traces: quicksight.register-user-quicksight-identity-lifecycle (assertion at check line 62, after the IAM-identity positive control registered successfully and DescribeUser reported Role=READER).

## Evidence
Check: quicksight.register-user-quicksight-identity-lifecycle [mismatch]

Call: quicksight RegisterUser(AwsAccountId=<account>, Namespace="default", IdentityType="QUICKSIGHT", UserName=<native-user>, Email=<native-user>@example.com, UserRole="READER") on an account subscribed with AuthenticationMethod=IAM_ONLY, Edition=ENTERPRISE.

Expected (AWS, recorded): Error.Code = "AccessDeniedException" (HTTP 401 per the model) and Error.Message = "Account {AwsAccountId} is not authorized to create QuickSight IdentityType user.AWS account id: {AwsAccountId}, Namespace: {Namespace}. "
  - fixed text: "Account ", " is not authorized to create QuickSight IdentityType user.", "AWS account id: ", ", Namespace: ", ". " (trailing space)
  - values from the request: {AwsAccountId} (twice), {Namespace}

Actual (emulator): {'Code': 'AccessDeniedException', 'Message': "You don't have access to this item. The provided credentials couldn't be validated. You might not be authorized to carry out the request. Make sure that your account is authorized to use the Amazon Quick Sight service, that your policies have the correct permissions, and that you are using the correct credentials."}

AssertionError raised at check_quicksight_register_user_quicksight_identity_lifecycle.py line 62 on `assert native_err["Message"] == (...)`. The Code assertion on the preceding line passed, so only the message diverges.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Ensure a QuickSight account subscription exists: quicksight CreateAccountSubscription(AwsAccountId=<account>, AccountName=..., NotificationEmail=..., AuthenticationMethod="IAM_ONLY", Edition="ENTERPRISE") and wait for DescribeAccountSubscription to report ACCOUNT_CREATED.
3. Positive control: create an IAM user, then quicksight RegisterUser(AwsAccountId=<account>, Namespace="default", IdentityType="IAM", IamArn=<iam-user-arn>, Email=..., UserRole="READER") — this succeeds and DescribeUser reports Role=READER.
4. Call quicksight RegisterUser(AwsAccountId=<account>, Namespace="default", IdentityType="QUICKSIGHT", UserName=<new-name>, Email=<new-name>@example.com, UserRole="READER").
5. Inspect the ClientError: the Code is AccessDeniedException as expected, but the Message is the generic "You don't have access to this item. ..." boilerplate instead of "Account <account> is not authorized to create QuickSight IdentityType user.AWS account id: <account>, Namespace: default. ".
