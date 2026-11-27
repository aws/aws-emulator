// SPDX-License-Identifier: CC0-1.0

//! What the calls that FOLLOW a refused domain patch observe — the half of the Identity
//! Center precondition that is about ORDERING rather than about the refusal itself.
//!
//! The RULE, one level above any single request: a `UpdateDomain` that would leave a domain
//! listing the `IDC` identity provider with no Identity Center instance behind it is decided
//! BEFORE the record is touched. Two consequences belong to the calls that come after it,
//! and are what this suite asserts:
//!
//! * the refusal is REPEATABLE — `UpdateDomain` is `@idempotent`, and a refusal that wrote
//!   nothing has nothing to make the second answer differ — and the reader that runs after
//!   each attempt still reports the domain the account held before the patch;
//! * the refusal is the PRECONDITION rather than a latch: the same patch, resubmitted naming
//!   an Identity Center instance, is served, mints the Identity Center application under
//!   that instance and is reported by the reader and the listing from then on.
//!
//! The rule is the model's own: `IdentityProviderConfiguration.identityCenterConfiguration`
//! is documented "Identity Center configuration. Required when identityProviders includes
//! IDC" and `Domain.identityCenterApplicationArn` "The ARN of the Identity Center
//! application. Absent for IAM-only domains" (`smithy_query(query="shape",
//! shape="IdentityProviderConfiguration,Domain")`); `UpdateDomain` is `@idempotent` and
//! declares `ValidationException` with `@httpError 400`, and `GetDomain` is `@readonly` with
//! a required `domain` member (`smithy_query(query="operation",
//! operation="UpdateDomain,GetDomain")`). The model fixes no message text, so the wording is
//! recorded behaviour and lives, written out, in
//! [`crate::identity_center_patch_precondition`].
//!
//! The rows, the wire harness and the refusal's wording are that suite's — it is where the
//! refusal ITSELF is asserted, over the same table (the resulting provider list's form, how
//! the instance comes to be missing, and whether the patch also renames the domain), so this
//! suite adds the sequences rather than a second copy of the harness.

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::types::UpdateDomainOutput;

use crate::identity_center_patch_precondition::{
    decode, instance_arn, refused_rows, Configuration, Wire, INSTANCE_ARN, PRECONDITION_MESSAGE,
    VALIDATION_EXCEPTION, VALIDATION_STATUS,
};
use crate::validation_message_wording::error_envelope;

/// The identical request earns the identical refusal, and the reader after each one reports
/// the unpatched domain.
#[tokio::test]
async fn the_refusal_repeats_and_the_reader_after_it_reports_the_unpatched_domain() {
    for row in refused_rows() {
        let case = row.case;
        // An account holds at most ONE domain, so each row patches the domain of an emulator
        // of its own.
        let wire = Wire::new();
        let created = wire.create_domain(&row).await;
        // What the store holds BEFORE the patch, read through the reader that reports it, so
        // every comparison below is between two reads of the same record.
        let before = format!("{:?}", wire.get_domain(case, &created.domain_id).await);
        let stored_providers: Vec<String> = row
            .stored_providers
            .iter()
            .map(|provider| (*provider).to_string())
            .collect();

        for attempt in 1..=2 {
            let (status, body) = wire.update_domain(&row, &created.domain_id).await;
            let (code, message) = error_envelope(case, &body);
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (VALIDATION_STATUS, VALIDATION_EXCEPTION, PRECONDITION_MESSAGE),
                "case {case:?} attempt {attempt}: expected {VALIDATION_STATUS} \
                 {VALIDATION_EXCEPTION} {PRECONDITION_MESSAGE:?}, got {status} {code:?}: \
                 {message:?}"
            );

            // The read a client makes next: the whole record, and the provider list the
            // patch asked to move.
            let after = wire.get_domain(case, &created.domain_id).await;
            assert_eq!(
                format!("{after:?}"),
                before,
                "case {case:?} attempt {attempt}: the refused patch changed the domain"
            );
            assert_eq!(
                after.identity_providers, stored_providers,
                "case {case:?} attempt {attempt}: expected the stored providers \
                 {stored_providers:?}, got {:?}",
                after.identity_providers
            );
            assert_eq!(
                after.name.as_deref(),
                Some(row.stored_name),
                "case {case:?} attempt {attempt}: expected the stored name {:?}, got {:?}",
                row.stored_name,
                after.name
            );
        }
    }
}

/// Each refused patch, resubmitted naming an Identity Center instance, is served and
/// persisted.
#[tokio::test]
async fn a_refused_patch_is_served_once_it_names_an_instance() {
    for row in refused_rows() {
        let case = row.case;
        let wire = Wire::new();
        let created = wire.create_domain(&row).await;

        let (refused, _) = wire.update_domain(&row, &created.domain_id).await;
        assert_eq!(
            refused, VALIDATION_STATUS,
            "case {case:?}: the patch with no instance behind it must be refused, got {refused}"
        );

        let (status, body) = wire
            .update_domain_carrying(
                &row,
                &created.domain_id,
                Configuration::Instance(INSTANCE_ARN),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the patch naming an instance must be served, got {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let patched = decode::<UpdateDomainOutput>(case, "UpdateDomain", &body).domain;

        let expected_providers = row.resulting_providers();
        assert_eq!(
            patched.identity_providers, expected_providers,
            "case {case:?}: expected the patched providers {expected_providers:?}, got {:?}",
            patched.identity_providers
        );
        assert_eq!(
            patched.name.as_deref(),
            Some(row.resulting_name()),
            "case {case:?}: expected the patched name {:?}, got {:?}",
            row.resulting_name(),
            patched.name
        );
        assert_eq!(
            instance_arn(&patched),
            Some(INSTANCE_ARN),
            "case {case:?}: expected the instance {INSTANCE_ARN:?}, got {:?}",
            instance_arn(&patched)
        );
        // `identityCenterApplicationArn` is "Absent for IAM-only domains", so a domain left
        // listing IDC reports one, minted under the instance it federates through.
        let application = patched
            .identity_center_application_arn
            .as_deref()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a domain listing IDC must report an Identity Center \
                     application"
                )
            });
        let instance = INSTANCE_ARN.split('/').next_back().unwrap_or_default();
        assert!(
            application.contains(instance),
            "case {case:?}: the application {application:?} must name the instance \
             {instance:?} the domain federates through"
        );

        // The served patch persisted: the reader and the listing answer from the same store.
        let read_back = wire.get_domain(case, &created.domain_id).await;
        assert_eq!(
            format!("{read_back:?}"),
            format!("{patched:?}"),
            "case {case:?}: GetDomain must report the patched domain"
        );
        let listed = wire.listed_instance_arn(case, &created.domain_id).await;
        assert_eq!(
            listed.as_deref(),
            Some(INSTANCE_ARN),
            "case {case:?}: expected the listing to report {INSTANCE_ARN:?}, got {listed:?}"
        );
    }
}
