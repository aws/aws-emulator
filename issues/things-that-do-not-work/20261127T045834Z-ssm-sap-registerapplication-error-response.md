Issue-Severity: CRITICAL
Issue-Gap-Class: error-response
Issue-Commit: 3996f901063d22c400f1655bb242a5d1c6eab093

# Refusal clause echoes the @sensitive SecretId a credential carried

_Filed: 2026-11-27T04:58:34Z_
_Operation: RegisterApplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `3996f901063d22c400f1655bb242a5d1c6eab093`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service words its own pre-handler constraint refusals (`src/service.rs`'s `constraint_message`, reached for every modelled operation through `StatedPerOperation`). That clause builder decides what to print from `violation.value` ALONE and never reads `Violation::sensitive`, the flag the framework carries so an author's override can tell a redacted value from an absent one.

The collecting half it is paired with (`service_common::validation::refusing_violations_with_lists`, i.e. `with_submitted_values_up_to`) deliberately UNDOES the framework's redaction: for a violation the walk withheld it puts the submitted value back and sets `sensitive = false`. Its own doc says a service whose clause withholds such a value must read `Violation::sensitive` in its clause builder instead of relying on the value's absence — which this crate does not do.

Consequence: `ApplicationCredential$SecretId` is `@sensitive` AND `@length(min:1, max:100)`, so a `RegisterApplication` (or `UpdateApplicationSettings`) request carrying a 101-character `SecretId` is refused with a 400 whose message QUOTES the secret back. The same clause builder is also reached with un-restored violations through `override_refusal`, where a sensitive violation keeps `value: None` and is therefore stated as `Value null at '…'` — i.e. as a member the caller left ABSENT — which the builder's own doc reserves for "an ABSENT member, which has no submitted value".

The crate states the opposite rule everywhere else it touches this member: the operation record names a credential by its database and type "never the secret", and the settings message is worded so "nothing a credential's `@sensitive` `SecretId` holds is ever worded into a response". The refusal path is the one place that rule is broken.

## Evidence

Model: `smithy_query(query="shape", shape="ApplicationCredential")` -> `"SecretId": {"target": "SecretId", "required": true, "length": {"min": 1, "max": 100}, "sensitive": true, ...}`.

Framework contract — vendor/smithy-schema-server/src/constraints.rs:68-70:
    /// The member is `@sensitive`: `value` is `None` and the message names only the path, so the
    /// value never reaches a response or an author's refusal override.
    pub sensitive: bool,
and its own renderer, vendor/smithy-schema-server/src/constraints.rs:169-174:
        if self.sensitive {
            return format!(
                "Value at '{}' failed to satisfy constraint: {requirement}",
                self.path
            );
        }

This crate's clause builder — services/service-com-amazonaws-ssmsap-ssmsap/src/service.rs:255-262:
        clauses.push(match (&violation.value, held) {
            (None, _) => {
                format!("Value null at '{path}' failed to satisfy constraint: {requirement}")
            }
            (Some(submitted), held) => {
                let shown = held.as_deref().unwrap_or(submitted.as_str());
                format!("Value '{shown}' at '{path}' failed to satisfy constraint: {requirement}")
            }
(no branch reads `violation.sensitive`; its doc at service.rs:220-224 says "The value is echoed as it was SUBMITTED … and an ABSENT member, which has no submitted value, is stated as the `null` it was.")

How it is reached — services/service-com-amazonaws-ssmsap-ssmsap/src/service.rs:481-490:
    let (violations, lists) =
        service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    …
        message: constraint_message(&violations, &lists)?,
and service.rs:507-511 (`StatedPerOperation::run` serves every one of the 27 operations, per service.rs:400-430).

The un-redaction that pass performs — services/common/src/validation.rs:1413-1426:
        if violation.sensitive {
            let withheld = index
                .candidate(violation)
                .map(|submitted| quoted_echo_up_to(submitted, FRAMEWORK_ECHO_CEILING).into_owned());
            if let Some(withheld) = withheld.filter(|value| value.len() <= budget) {
                budget -= withheld.len();
                violation.value = Some(withheld);
                violation.sensitive = false;
            }
with the instruction this crate did not follow — services/common/src/validation.rs:1464-1467:
/// For a service whose published clause QUOTES the value the framework stored: the
/// redaction is what has to be undone, and nothing else about the clause. A service whose
/// clause withholds such a value reads [`Violation::sensitive`] (or its own reading of the
/// model) in its clause builder instead of relying on the value's absence.
(the leaf collector records every string leaf with no sensitivity filter — services/common/src/validation.rs:3259-3274 `fn record`, called with `withholds_sensitive` left at its `false` default by `collected_leaves`, services/common/src/validation.rs:2606-2614 — so the secret IS available to restore.)

The rule this crate states for the same member elsewhere — src/ops/register_application.rs:302-305:
        // `SecretId` is `@sensitive`, so what the record names of a credential is the SAP
        // HANA database it reaches and its type, never the secret
and src/state.rs:3566-3568:
/// than any value it carried, so nothing a credential's `@sensitive` `SecretId` holds is
/// ever worded into a response.

Test coverage: no suite exercises a refusal of a `@sensitive` member — `grep -n "secretId\|Value null" tests/all` finds only `Value null at 'operationId'` (tests/all/validation_message_rendering.rs:251) and `Value null at 'applicationId'` (src/service.rs:849), both genuinely absent members, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /register-application with a valid HANA body (`ApplicationId`, `ApplicationType: "HANA"`, one `Instances` entry, `Sid`, `SapInstanceNumber`) whose `Credentials[0]` is `{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"<101 characters>"}` — `SecretId` is `@length(min:1,max:100)` and `@sensitive`.
2. Observe the 400 `ValidationException`: its message is `1 validation error detected: Value '<the 101-character secret>' at 'credentials.1.secretId' failed to satisfy constraint: Member must have length less than or equal to 100` — the `@sensitive` value is echoed back in the response.
3. Expected (the framework's own rendering for a violation it marked `sensitive`, vendor/smithy-schema-server/src/constraints.rs:169-174): a clause naming only the path — `Value at 'credentials.1.secretId' failed to satisfy constraint: Member must have length less than or equal to 100` — with no secret in the body.
4. Repeat with POST /update-application-settings carrying `CredentialsToAddOrUpdate[0].SecretId` of 101 characters: the same shared clause builder echoes the secret there too.
5. Separately, reach the `override_refusal` path (violations handed over un-restored) for the same member and observe the opposite failure: `Value null at 'credentials.1.secretId' …`, which reports a value the caller DID send as an absent member.
