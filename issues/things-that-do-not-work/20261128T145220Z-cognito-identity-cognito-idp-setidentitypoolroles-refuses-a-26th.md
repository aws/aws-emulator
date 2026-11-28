Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp.role-mapping-rules-above-25-per-provider-rejected
Issue-Key: cognito-identity__cognito-idp.role-mapping-rules-above-25-per-provider-rejected|run#25|AssertionError|assert got == want, f"26 rules for one provider answered {got!r}, want {want!r}"
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-identity__cognito-idp: SetIdentityPoolRoles refuses a 26th role-mapping rule with an invented message instead of AWS's 'The number of rules in the request exceeded 25.'

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetIdentityPoolRoles

## Description
Cause 1 of 1: the per-provider role-mapping rule limit is enforced with the wrong message. AWS: SetIdentityPoolRoles accepts up to 25 Rules per identity-provider key in RoleMappings; a mapping carrying 26 rules for one provider is refused with InvalidParameterException and the message 'The number of rules in the request exceeded 25.' — the only variable part is the limit (25), the rest is fixed text; the message names neither the RoleMappings key nor the member path nor the submitted count, and the stored mappings are left unchanged. Smithy (com.amazonaws.cognitoidentity#AWSCognitoIdentityService, awsJson1_1): SetIdentityPoolRoles input RoleMappings = RoleMappingMap @length 0..10, Roles = RolesMap required @length 0..2; RulesConfigurationType.Rules is required, @length 1..400, documented as 'An array of rules. You can specify up to 25 rules per identity provider.'; errors include InvalidParameterException (@httpError 400). The message text is recorded behavior only. Emulator: the right class is raised but the message is "RoleMappings['cognito-idp.<region>.amazonaws.com/<user-pool-id>:<client-id>'].RulesConfiguration.Rules carries 26 rules: at most 25 rules can be specified per identity provider." — a validator-style sentence whose fixed text is entirely different and which embeds the request's RoleMappings key, member path and rule count, none of which AWS includes. Why: the HTTP status, error code and error message are all part of the contract; a client or negative test that matches AWS's sentence (or logs/alarms on it) sees an unrecognised message, and the emulator also discloses the full provider:client key in an error body where AWS does not. Fix: the error-message text produced by the RoleMappings rule-count validation in the SetIdentityPoolRoles handler — emit AWS's fixed sentence with the limit as its only variable part. Traces: cognito-identity__cognito-idp.role-mapping-rules-above-25-per-provider-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1, check cognito-identity__cognito-idp.role-mapping-rules-above-25-per-provider-rejected [mismatch]: after SetIdentityPoolRoles stored 25 rules for each of two provider:client keys, resubmitting with 26 rules for one key gave ('InvalidParameterException', "RoleMappings['cognito-idp.us-west-2.amazonaws.com/us-west-2_659e7f579:186cb1904ac8444bbb9397ba70'].RulesConfiguration.Rules carries 26 rules: at most 25 rules can be specified per identity provider."), want ('InvalidParameterException', 'The number of rules in the request exceeded 25.'). The provider id, client id and the count 26 in the emulator's message are this run's request values, not part of AWS's template; the limit 25 is the requirement. Smithy: RulesConfigurationType.Rules required, @length 1..400, doc 'You can specify up to 25 rules per identity provider.'; SetIdentityPoolRoles errors include InvalidParameterException (@httpError 400), RoleMappings = RoleMappingMap @length 0..10.

## Steps to Reproduce
1. Create an IAM role, a user pool with two app clients, and an identity pool registering provider 'cognito-idp.<region>.amazonaws.com/<pool-id>' for both clients.
2. Call SetIdentityPoolRoles with Roles={'authenticated': roleArn} and RoleMappings carrying a Rules-type mapping with 25 rules for each '<provider>:<clientId>' key — this succeeds and GetIdentityPoolRoles shows 25 + 25 rules.
3. Resubmit the same call with 26 rules for one of the two keys. Compare the error code and message against ('InvalidParameterException', 'The number of rules in the request exceeded 25.'); the emulator answers InvalidParameterException with a member-path/count message instead.
4. Re-read GetIdentityPoolRoles to confirm the refused call changed nothing (this part already matches).
