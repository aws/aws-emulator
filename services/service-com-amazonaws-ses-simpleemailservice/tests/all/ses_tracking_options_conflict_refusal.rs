// SPDX-License-Identifier: CC0-1.0

//! What a `CreateConfigurationSetTrackingOptions` on a set that ALREADY carries an
//! association is refused with, end to end over the awsQuery wire.
//!
//! The rule this suite pins: the create that collides with an existing association
//! is refused 400 under the `Error.Code` Amazon SES puts on the query wire — the
//! `awsQueryError` code the error shape's model annotation carries
//! (`smithy_query(query="shape", shape="TrackingOptionsAlreadyExistsException")`),
//! which for the tracking-options pair keeps the trailing `Exception` most codes
//! of this surface drop — and with
//! the sentence that NAMES the configuration set it rejected, so a client that
//! configured several sets in one run can tell which one is already configured.
//! The existing association is left exactly as it was.
//!
//! The table spans what the rule generalises over: the form of the configuration
//! set name (one character, mixed case with both separators, the longest name the
//! naming rule admits), the form of the domain already stored (a verified apex, one
//! label under it, several labels under it, a label with digits and dashes), the
//! apex the domains sit under, and whether the colliding create names a DIFFERENT
//! domain or repeats the stored one.

use crate::harness::{
    call, element, emulator_and_engine, error_envelope, field, settle_verifications,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `VerifyDomainIdentity`, carried through to a VERIFIED domain so a tracking
/// domain under it is one Amazon SES would redirect through.
async fn verify_domain(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    domain: &str,
) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "case verify {domain}: expected 200, got {status} with {body}"
    );
    settle_verifications(engine);
}

/// `CreateConfigurationSet`.
async fn create_configuration_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await
}

/// `CreateConfigurationSetTrackingOptions`.
async fn create_tracking_options(
    router: &MultiServiceRouter,
    name: &str,
    custom_redirect_domain: &str,
) -> (u16, String) {
    call(
        router,
        "CreateConfigurationSetTrackingOptions",
        &[
            field("ConfigurationSetName", name),
            field("TrackingOptions.CustomRedirectDomain", custom_redirect_domain),
        ],
    )
    .await
}

/// The custom redirect domain `DescribeConfigurationSet` reports for `name`, or
/// `None` when the set carries no tracking options.
async fn described_tracking_domain(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeConfigurationSet",
        &[
            field("ConfigurationSetName", name),
            field("ConfigurationSetAttributeNames.member.1", "trackingOptions"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case describe {name}: expected 200, got {status} with {body}"
    );
    element(&body, "TrackingOptions")
        .as_deref()
        .and_then(|tracking_options| element(tracking_options, "CustomRedirectDomain"))
}

/// The refusal a colliding create answers: the code a query client branches on,
/// and the sentence naming the set that is already configured.
fn already_an_association(name: &str) -> (Option<String>, Option<String>) {
    (
        Some("TrackingOptionsAlreadyExistsException".to_string()),
        Some(format!(
            "There is already a tracking options for configuration set <{name}>"
        )),
    )
}

/// One row of the table: a configuration set, the domain its association is
/// created with, and the domain the colliding create names.
struct Case {
    /// The `ConfigurationSetName`, in one of the forms the naming rule admits.
    set: String,
    /// The `CustomRedirectDomain` the first create stores.
    stored: String,
    /// The `CustomRedirectDomain` the second create names — which the refusal
    /// makes no difference between, stored domain or not.
    colliding: String,
}

#[tokio::test]
async fn a_second_create_on_a_configured_set_is_refused_naming_that_set() {
    let (router, engine) = emulator_and_engine();
    // Two verified apexes, so a row's domains are not all under one identity.
    for apex in ["example.com", "example.net"] {
        verify_domain(&router, &engine, apex).await;
    }

    // `ConfigurationSet$Name`: "Contain only letters (a-z, A-Z), numbers (0-9),
    // underscores (_), or dashes (-)" and "Contain 64 characters or fewer".
    let cases = [
        Case {
            set: "a".to_string(),
            stored: "click.example.com".to_string(),
            colliding: "track.example.com".to_string(),
        },
        Case {
            set: "Dup_Set-2".to_string(),
            stored: "example.net".to_string(),
            colliding: "deep.nested.example.net".to_string(),
        },
        Case {
            set: "a".repeat(64),
            stored: "a-1.example.com".to_string(),
            colliding: "a-1.example.com".to_string(),
        },
        Case {
            set: "9".to_string(),
            stored: "deep.nested.click.example.net".to_string(),
            colliding: "example.com".to_string(),
        },
    ];

    for case in &cases {
        let Case {
            set,
            stored,
            colliding,
        } = case;
        let (status, body) = create_configuration_set(&router, set).await;
        assert_eq!(
            status, 200,
            "case {set:?}: expected the set to be created, got {status} with {body}"
        );
        let (status, body) = create_tracking_options(&router, set, stored).await;
        assert_eq!(
            status, 200,
            "case {set:?}: expected the first association to be stored, got {status} with {body}"
        );

        // The collision is refused the same way however often it is retried, and
        // whether it names another domain or the stored one.
        for round in 1..=2 {
            let (status, body) = create_tracking_options(&router, set, colliding).await;
            assert_eq!(
                status, 400,
                "case {set:?} round {round}: expected 400 for a set that is already configured, \
                 got {status} with {body}"
            );
            assert_eq!(
                error_envelope(&body),
                already_an_association(set),
                "case {set:?} round {round}: expected the already-exists refusal naming the set, \
                 got {body}"
            );
            let reported = described_tracking_domain(&router, set).await;
            assert_eq!(
                reported.as_deref(),
                Some(stored.as_str()),
                "case {set:?} round {round}: the refused create must leave the stored domain \
                 alone, got {reported:?}"
            );
        }
    }

    // The refusal is about the set the request named: a sibling set created in the
    // same run still takes its own association.
    let fresh = "sibling-set";
    let (status, body) = create_configuration_set(&router, fresh).await;
    assert_eq!(
        status, 200,
        "case {fresh:?}: expected the set to be created, got {status} with {body}"
    );
    let (status, body) = create_tracking_options(&router, fresh, "click.example.com").await;
    assert_eq!(
        status, 200,
        "case {fresh:?}: expected the association to be stored, got {status} with {body}"
    );
    assert_eq!(
        described_tracking_domain(&router, fresh).await.as_deref(),
        Some("click.example.com"),
        "case {fresh:?}: expected the stored domain to be reported back"
    );
}
