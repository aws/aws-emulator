// SPDX-License-Identifier: CC0-1.0

//! `catalog` is an ALLOWED-VALUE member, and naming no catalog is an input error.
//!
//! The rule under test: a request whose `catalog` names no catalog this service
//! serves (`state::SUPPORTED_CATALOGS`) is refused with the modelled
//! `ValidationException` (HTTP 400) BEFORE any product or agreement is resolved,
//! however well-formed the value is and whatever else the request carries — so a
//! wrong catalog is a permanent 400-class input error and nothing is written for
//! it, and it never becomes the 404 an unresolved product or agreement inside a
//! catalog this service DOES serve gets (the `put_deployment_parameter_target`
//! suite owns that rule).
//!
//! Where the set comes from: `PutDeploymentParameterRequest$catalog` is documented
//! as "The catalog related to the request. Fixed value: `AWSMarketplace`"
//! (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="catalog")`),
//! while the `Catalog` shape itself carries only the SYNTAX of a catalog name
//! (`length 1..64`, `^[a-zA-Z_-]+$`,
//! `smithy_query(query="shape",shape="Catalog")`). The two are different
//! judgements and this suite pins both: a value outside the syntax is refused as
//! that constraint (`validation_messages` states its message), and a value inside
//! the syntax but outside the set is refused as naming no catalog.
//!
//! Every case drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    is_supported_catalog, SUPPORTED_CATALOGS,
};

use crate::deployment_parameters::{
    data_as_a_service, emulator, error_code, field_name, get_configuration, get_job, list_jobs,
    list_tags, message, provision, put, start_job, tags_of, CATALOG, DELIVERY_OPTION,
};

/// A body naming `agreement_id`, in the minimal shape a write takes.
fn body(agreement_id: &str) -> Value {
    json!({
        "agreementId": agreement_id,
        "deploymentParameter": { "name": "DbPassword", "secretString": "s3cr3t-value-1" },
    })
}

#[tokio::test]
async fn a_catalog_that_names_no_catalog_is_refused_before_the_target_is_resolved() {
    let (router, engine) = emulator();
    // A target the account holds under the catalog it serves, so no row below is
    // refused for naming no product or agreement instead of for its catalog.
    provision(&engine, CATALOG, "agmt-held", &["prod-held"]);

    // The set the service reads is the model's documented one, so a row below that
    // names an unsupported catalog is unsupported for the same reason the service
    // says it is.
    assert_eq!(
        SUPPORTED_CATALOGS,
        &[CATALOG],
        "expected the documented fixed value as the only supported catalog, got {SUPPORTED_CATALOGS:?}"
    );

    // Each row is INSIDE `Catalog`'s own syntax (`^[a-zA-Z_-]+$`, len 1..64) and
    // outside the set of catalogs that exist. The rows differ along what the rule
    // generalises over: a plausible other marketplace name, a value the issue did
    // not cite, the supported name's own casing variants, its substrings and
    // extensions, the one-character minimum and the 64-character maximum.
    let rows: Vec<(&str, &str)> = vec![
        ("a plausible but non-existent catalog", "NotTheRealMarketplace"),
        ("another AWS catalog name", "PrivateMarketplace"),
        ("the supported name lower-cased", "awsmarketplace"),
        ("the supported name upper-cased", "AWSMARKETPLACE"),
        ("the supported name with a separator", "AWS_Marketplace"),
        ("the supported name with a hyphen", "AWS-Marketplace"),
        ("a prefix of the supported name", "AWSMarket"),
        ("the supported name with a suffix", "AWSMarketplaceToo"),
        ("the one-character minimum", "a"),
        (
            "64 characters, the modelled maximum",
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        ),
    ];

    for (case, catalog) in rows {
        assert!(
            !is_supported_catalog(catalog),
            "case {case:?}: the row must name no supported catalog, got {catalog:?}"
        );
        // Both sides of the target check: a target the account holds under the
        // SUPPORTED catalog, and one nothing holds at all. The answer is the same,
        // which is what "before the target is resolved" means.
        for (target, product_id, agreement_id) in [
            ("a held product and agreement", "prod-held", "agmt-held"),
            ("a target nothing holds", "prod-unknown", "agmt-unknown"),
        ] {
            let (status, answer) = put(&router, catalog, product_id, &body(agreement_id)).await;
            assert_eq!(
                status, 400,
                "case {case:?} with {target:?}: a catalog naming no catalog must be 400, \
                 got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?} with {target:?}: expected ValidationException, got {answer:?}"
            );
            assert_eq!(
                field_name(&answer),
                "catalog",
                "case {case:?} with {target:?}: the refusal must name the catalog member, \
                 got {answer:?}"
            );
            let reported = message(&answer);
            assert!(
                reported.contains(CATALOG),
                "case {case:?} with {target:?}: the refusal must list the supported catalog, \
                 got {reported:?}"
            );
            assert!(
                !reported.contains("does not exist"),
                "case {case:?} with {target:?}: the refusal must not read as a missing \
                 resource, got {reported:?}"
            );
        }
    }

    // Nothing above wrote a record: the refusal precedes every write.
    let (status, answer) = put(&router, CATALOG, "prod-held", &body("agmt-held")).await;
    assert_eq!(
        status, 200,
        "the supported catalog must still be written, got {status} {answer:?}"
    );
    let arn = answer
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    // The round trip: the one accepted write is readable back through an
    // implemented read, so the rule refuses the unsupported catalogs without
    // refusing the supported one.
    let (status, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "the written record must be readable, got {status} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        std::collections::BTreeMap::new(),
        "a create carrying no tags must store none, got {listed:?}"
    );
}

#[tokio::test]
async fn the_catalog_rule_holds_for_every_operation_of_the_deployment_configuration_slice() {
    let (router, engine) = emulator();
    // A product the account holds under the catalog it serves, so no row below is
    // refused for naming no product instead of for its catalog.
    provision(&engine, CATALOG, "agmt-config-catalog", &["prod-config-catalog"]);

    // The four operations that name a catalog and address a deployment
    // configuration. A `catalog` naming no catalog is judged by each of them before
    // the product, the job or the configuration is resolved, so every row is the
    // same 400 whichever operation it reaches.
    let unsupported = ["NotTheRealMarketplace", "awsmarketplace", "AWSMarketplaceToo"];
    for catalog in unsupported {
        assert!(
            !is_supported_catalog(catalog),
            "the row must name no supported catalog, got {catalog:?}"
        );
        let answers = vec![
            (
                "StartDeploymentConfigurationJob",
                start_job(
                    &router,
                    catalog,
                    "prod-config-catalog",
                    DELIVERY_OPTION,
                    &json!({
                        "configuration": data_as_a_service("https://catalog.example.com/start"),
                    }),
                )
                .await,
            ),
            (
                "GetDeploymentConfigurationJob",
                get_job(
                    &router,
                    catalog,
                    "prod-config-catalog",
                    DELIVERY_OPTION,
                    "0123456789abcdef",
                )
                .await,
            ),
            (
                "ListDeploymentConfigurationJobs",
                list_jobs(&router, catalog, "prod-config-catalog", DELIVERY_OPTION, "").await,
            ),
            (
                "GetDeploymentConfiguration",
                get_configuration(&router, catalog, "prod-config-catalog", DELIVERY_OPTION).await,
            ),
        ];
        for (operation, (status, answer)) in answers {
            assert_eq!(
                status, 400,
                "{operation} with catalog {catalog:?}: a catalog naming no catalog must be 400, \
                 got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "{operation} with catalog {catalog:?}: expected ValidationException, \
                 got {answer:?}"
            );
            assert_eq!(
                field_name(&answer),
                "catalog",
                "{operation} with catalog {catalog:?}: the refusal must name the catalog member, \
                 got {answer:?}"
            );
            let reported = message(&answer);
            assert!(
                reported.contains(CATALOG),
                "{operation} with catalog {catalog:?}: the refusal must list the supported \
                 catalog, got {reported:?}"
            );
        }
    }

    // Nothing above started a job: the refusal precedes every write, and the
    // supported catalog is still served.
    let (status, listed) = list_jobs(
        &router,
        CATALOG,
        "prod-config-catalog",
        DELIVERY_OPTION,
        "",
    )
    .await;
    assert_eq!(
        status, 200,
        "the supported catalog must still be served, got {status} {listed:?}"
    );
    assert_eq!(
        listed.get("jobs").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "a refused start must have recorded no job, got {listed:?}"
    );
}

#[tokio::test]
async fn a_catalog_outside_its_syntax_is_refused_as_that_constraint_not_as_an_unknown_catalog() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-held", &["prod-held"]);

    // Outside `Catalog`'s own `@pattern` / `@length`: these are refused by the
    // modelled constraint, which is a DIFFERENT clause of the same 400 — it quotes
    // the constraint the value failed rather than the catalogs that exist.
    let over_long = "c".repeat(65);
    let rows: Vec<(&str, &str)> = vec![
        ("a space and a bang, as the pattern excludes", "AWS Marketplace!"),
        ("a digit, which the pattern excludes", "AWSMarketplace1"),
        ("one character past the modelled maximum", &over_long),
    ];

    for (case, catalog) in rows {
        let (status, answer) = put(&router, catalog, "prod-held", &body("agmt-held")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a catalog outside its syntax must be 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let reported = message(&answer);
        assert!(
            reported.starts_with("1 validation error detected: "),
            "case {case:?}: expected the modelled-constraint report, got {reported:?}"
        );
    }
}
