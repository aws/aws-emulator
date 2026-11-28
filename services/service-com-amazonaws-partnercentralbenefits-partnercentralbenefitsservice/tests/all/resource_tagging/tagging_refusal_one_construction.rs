// SPDX-License-Identifier: CC0-1.0

//! The single construction of the taggable-resource not-found refusal, driven end to end
//! over the wire through one call per former hand-rolled copy.
//!
//! The rule under test: a tagging call whose `resourceArn` resolves to no resource this
//! service holds is answered the ONE refusal
//! `ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::require_taggable_resource`
//! (and its writing counterpart `require_taggable_resource_mut`) builds, so the wire
//! answer of `TagResource`, `UntagResource` and `ListTagsForResource` moves with that
//! helper and none of them can word a failed lookup its own way — the shape this crate
//! already gives the catalog lookup (`state::catalog`) and the application lookup
//! (`state::require_application`).
//!
//! Every expected message is read out of the LIVE store through the helper itself rather
//! than spelled here, so a change to what the helper builds changes what all three
//! operations must answer, and the read handle and the write handle are required to build
//! the same one.
//!
//! The table spans the class rather than one call: every tagging operation crossed with
//! the ways an ARN of the modelled `TaggableResourceArn` form can fail to resolve — an id
//! nothing minted, a resource type that holds no tag set, a catalog the service does not
//! serve, another region, account or partition, a sub-resource path and a resource type
//! this service does not serve — and, on the other side of the rule, a resource the store
//! does hold, which the helper refuses on neither handle and no operation refuses either.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    service_state, APPLICATION_RESOURCE, BENEFIT_RESOURCE, SEEDED_CATALOGS,
};

use super::{
    application_arn, benefit_id, error_message, expected_arn, taggable_arn, tags_of, ACCOUNT,
    REGION,
};
use crate::benefit_application_lifecycle::{error_code, Harness};

/// A well-formed body for each of the three tagging operations, naming `resource_arn`.
///
/// These are exactly the operations whose `resourceArn` the single lookup resolves, so the
/// table is the whole class it serves.
fn every_tagging_call(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({
                "resourceArn": resource_arn,
                "tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
        (
            "UntagResource",
            json!({ "resourceArn": resource_arn, "tagKeys": ["env"] }),
        ),
        (
            "ListTagsForResource",
            json!({ "resourceArn": resource_arn }),
        ),
    ]
}

/// The refusal the state helper builds for `resource_arn` in this harness's own store, and
/// `None` when the resource is there.
///
/// Both handles are asked, and a disagreement between them fails here: the reporting
/// lookup and the writing one are one construction, so the message a tag READ is refused
/// with cannot drift from the one a tag WRITE is refused with.
fn helper_refusal(harness: &Harness, case: &str, resource_arn: &str) -> Option<String> {
    let state = service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state");
    let mut store = state.lock();
    let reporting = store
        .require_taggable_resource(resource_arn)
        .err()
        .map(|refusal| refusal.message);
    let writing = store
        .require_taggable_resource_mut(resource_arn)
        .err()
        .map(|refusal| refusal.message);
    assert_eq!(
        reporting, writing,
        "case {case:?}: the reporting and the writing lookup must build one refusal, got \
         {reporting:?} and {writing:?}"
    );
    reporting
}

/// Every tagging operation answers an ARN that resolves to no resource with exactly the
/// refusal the state helper builds — the three former inline copies included — and the
/// refusals leave the one real resource's tag set as it was.
#[tokio::test]
async fn every_tagging_operation_answers_the_refusal_the_one_helper_builds() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];
    let existing = application_arn(
        &harness,
        "seed",
        catalog,
        "tok-one-construction",
        &[("env", "test")],
    )
    .await;
    let benefit = benefit_id(&harness, "seed", catalog).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "an application id nothing holds",
            expected_arn(catalog, APPLICATION_RESOURCE, "benappl-00000000000000"),
        ),
        (
            "a benefit that does not exist",
            taggable_arn(catalog, BENEFIT_RESOURCE, "ben-00000000000000"),
        ),
        (
            "a benefit that does exist, which holds no tag set",
            taggable_arn(catalog, BENEFIT_RESOURCE, &benefit),
        ),
        (
            "a catalog the service does not serve",
            expected_arn("NotACatalog", APPLICATION_RESOURCE, "benappl-00000000000000"),
        ),
        (
            "the application's ARN in another region",
            existing.replace(REGION, "eu-west-1"),
        ),
        (
            "the application's ARN under another account",
            existing.replace(ACCOUNT, "210987654321"),
        ),
        (
            "the application's ARN in another partition",
            existing.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        (
            "a sub-resource of the existing application",
            format!("{existing}/attachment/1"),
        ),
        (
            "a resource type this service does not serve",
            expected_arn(catalog, "opportunity", "O-1"),
        ),
    ];

    for (name, resource_arn) in &rows {
        let expected = helper_refusal(&harness, name, resource_arn).unwrap_or_else(|| {
            panic!("case {name:?}: the helper must refuse the ARN {resource_arn:?}")
        });
        for (operation, body) in every_tagging_call(resource_arn) {
            let case = format!("{operation}: {name}");
            let (status, out) = harness.call(operation, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for {resource_arn:?}, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {out:?}"
            );
            let message = error_message(&out);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the helper's refusal {expected:?}, got {message:?}"
            );
        }
    }

    // The refusals wrote nothing: the one real resource still carries exactly the tags its
    // create supplied, so no row's `TagResource` or `UntagResource` reached the tag set.
    let got = tags_of(&harness, "after the refusals", &existing).await;
    assert_eq!(
        got,
        [("env".to_string(), "test".to_string())]
            .into_iter()
            .collect::<std::collections::BTreeMap<String, String>>(),
        "case \"after the refusals\": a refused tagging call must change nothing, got {got:?}"
    );
}

/// The other side of the rule: for a resource the store holds the helper builds no refusal
/// on either handle, and every tagging operation gets past the lookup and answers 200.
///
/// One resource per seeded catalog, so the lookup is exercised wherever the store keeps
/// one rather than only in the first catalog.
#[tokio::test]
async fn a_resource_the_store_holds_is_refused_by_no_tagging_operation() {
    for catalog in SEEDED_CATALOGS {
        let harness = Harness::new();
        let arn = application_arn(
            &harness,
            catalog,
            catalog,
            &format!("tok-one-construction-held-{catalog}"),
            &[("env", "test")],
        )
        .await;
        let refusal = helper_refusal(&harness, catalog, &arn);
        assert!(
            refusal.is_none(),
            "case {catalog:?}: the helper must not refuse a resource this store holds, got \
             {refusal:?}"
        );

        for (operation, body) in every_tagging_call(&arn) {
            let case = format!("{operation}: {catalog}");
            let (status, out) = harness.call(operation, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for {arn:?}, got {status} {out:?}"
            );
        }
    }
}
