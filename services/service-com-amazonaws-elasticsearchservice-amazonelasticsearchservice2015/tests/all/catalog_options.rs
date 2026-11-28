// SPDX-License-Identifier: CC0-1.0

//! The domain's catalog integration — `CatalogOptions` — end to end, driven as real
//! restJson1 requests through the router.
//!
//! The rule under test: a configuration option the create or update request carries is
//! stored on the domain and is what every later read reports — the option value itself
//! under `ElasticsearchDomainStatus`, and the same value wrapped in its own
//! `OptionStatus` provenance under `ElasticsearchDomainConfig`; an option no request has
//! ever supplied is reported by neither.
//!
//! The cases span the forms the member's shape admits rather than one recorded call: the
//! switch alone, the switch with a catalog carrying no settings, and the catalog with
//! each form of its encryption configuration
//! (`smithy_query(query="shape",shape="CatalogOptions,CatalogConfiguration,ServiceManagedS3TablesCatalog,S3TablesEncryptionConfiguration")`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region every call in this suite is signed for.
const REGION: &str = "us-west-2";

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    call_in(router, REGION, "POST", "/2015-01-01/es/domain", Some(body)).await
}

async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/es/domain/{name}"),
        None,
    )
    .await
}

async fn describe_config(router: &MultiServiceRouter, name: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/es/domain/{name}/config"),
        None,
    )
    .await
}

async fn update_config(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    call_in(
        router,
        REGION,
        "POST",
        &format!("/2015-01-01/es/domain/{name}/config"),
        Some(body),
    )
    .await
}

/// A create body for `name`, with the cluster config every domain has.
fn baseline(name: &str) -> Value {
    json!({
        "DomainName": name,
        "ElasticsearchVersion": "7.10",
        "ElasticsearchClusterConfig": {
            "InstanceType": "t2.small.elasticsearch",
            "InstanceCount": 1
        }
    })
}

/// The `CatalogOptions` values under test, one per form the shape admits.
fn catalog_option_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("disabled", json!({"Enabled": false})),
        ("enabled-no-catalog", json!({"Enabled": true})),
        (
            "bare-catalog",
            json!({"Enabled": true, "Catalog": {"ServiceManagedS3Tables": {}}}),
        ),
        (
            "plain-tables",
            json!({
                "Enabled": true,
                "Catalog": {
                    "ServiceManagedS3Tables": {
                        "EncryptionConfiguration": {"Enabled": false}
                    }
                }
            }),
        ),
        (
            "kms-tables",
            json!({
                "Enabled": true,
                "Catalog": {
                    "ServiceManagedS3Tables": {
                        "EncryptionConfiguration": {
                            "Enabled": true,
                            "KmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/22222222-2222-2222-2222-222222222222"
                        }
                    }
                }
            }),
        ),
    ]
}

/// Every form a create supplies is persisted, and both reads answer from the record.
#[tokio::test]
async fn create_persists_the_catalog_options_and_both_reads_report_them() {
    for (case, options) in catalog_option_cases() {
        let router = router();
        // Within `DomainName`'s 3..28 length, which the case names keep room for.
        let name = format!("cat-{case}");
        let mut body = baseline(&name);
        body["CatalogOptions"] = options.clone();

        let created = create(&router, &body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: create must succeed, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.at(case, "/DomainStatus/CatalogOptions"),
            &options,
            "case {case:?}: the create response must report the submitted CatalogOptions, got {:?}",
            created.body.pointer("/DomainStatus/CatalogOptions")
        );

        let status = describe(&router, &name).await;
        assert_eq!(
            status.at(case, "/DomainStatus/CatalogOptions"),
            &options,
            "case {case:?}: DescribeElasticsearchDomain must report the stored CatalogOptions, got {:?}",
            status.body.pointer("/DomainStatus/CatalogOptions")
        );

        let config = describe_config(&router, &name).await;
        assert_eq!(
            config.at(case, "/DomainConfig/CatalogOptions/Options"),
            &options,
            "case {case:?}: DescribeElasticsearchDomainConfig must report the stored CatalogOptions, got {:?}",
            config.body.pointer("/DomainConfig/CatalogOptions/Options")
        );
        assert_eq!(
            config.at(case, "/DomainConfig/CatalogOptions/Status/UpdateVersion"),
            &json!(1),
            "case {case:?}: the option set at create must carry the first config version, got {:?}",
            config
                .body
                .pointer("/DomainConfig/CatalogOptions/Status/UpdateVersion")
        );
        assert!(
            !config
                .at(case, "/DomainConfig/CatalogOptions/Status/CreationDate")
                .is_null(),
            "case {case:?}: the option's status must carry a CreationDate, got {}",
            config.body
        );
    }
}

/// An update replaces the stored options and stamps them with the domain's new config
/// version; a later update that omits the member leaves both as stored.
#[tokio::test]
async fn update_replaces_the_catalog_options_and_omitting_them_keeps_them() {
    for (case, options) in catalog_option_cases() {
        let router = router();
        let name = format!("upd-{case}");
        let created = create(&router, &baseline(&name)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: create must succeed, got {} {}",
            created.status, created.body
        );
        assert!(
            created.missing("/DomainStatus/CatalogOptions"),
            "case {case:?}: a create supplying no CatalogOptions must report none, got {:?}",
            created.body.pointer("/DomainStatus/CatalogOptions")
        );
        assert!(
            describe_config(&router, &name)
                .await
                .missing("/DomainConfig/CatalogOptions"),
            "case {case:?}: a domain with no catalog options configured must report none"
        );

        let updated =
            update_config(&router, &name, &json!({"CatalogOptions": options.clone()})).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: update must succeed, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.at(case, "/DomainConfig/CatalogOptions/Options"),
            &options,
            "case {case:?}: the update response must report the supplied CatalogOptions, got {:?}",
            updated.body.pointer("/DomainConfig/CatalogOptions/Options")
        );
        assert_eq!(
            updated.at(case, "/DomainConfig/CatalogOptions/Status/UpdateVersion"),
            &json!(2),
            "case {case:?}: the changed option must carry the bumped config version, got {:?}",
            updated
                .body
                .pointer("/DomainConfig/CatalogOptions/Status/UpdateVersion")
        );

        // Persisted: the domain status reports the option too, from the same record.
        let status = describe(&router, &name).await;
        assert_eq!(
            status.at(case, "/DomainStatus/CatalogOptions"),
            &options,
            "case {case:?}: DescribeElasticsearchDomain must report the merged CatalogOptions, got {:?}",
            status.body.pointer("/DomainStatus/CatalogOptions")
        );

        // A later update carrying another member leaves the catalog options exactly as
        // stored, version included.
        let other = update_config(
            &router,
            &name,
            &json!({"SnapshotOptions": {"AutomatedSnapshotStartHour": 7}}),
        )
        .await;
        assert_eq!(
            other.at(case, "/DomainConfig/CatalogOptions/Options"),
            &options,
            "case {case:?}: an update omitting CatalogOptions must leave them, got {:?}",
            other.body.pointer("/DomainConfig/CatalogOptions/Options")
        );
        assert_eq!(
            other.at(case, "/DomainConfig/CatalogOptions/Status/UpdateVersion"),
            &json!(2),
            "case {case:?}: an untouched option must keep its own config version, got {:?}",
            other
                .body
                .pointer("/DomainConfig/CatalogOptions/Status/UpdateVersion")
        );
    }
}

/// A dry-run update reports what the change would produce and stores nothing — the
/// catalog options included.
#[tokio::test]
async fn a_dry_run_update_of_the_catalog_options_persists_nothing() {
    let router = router();
    let name = "catalog-dry-run";
    let created = create(&router, &baseline(name)).await;
    assert_eq!(
        created.status, 200,
        "case \"dry run\": create must succeed, got {} {}",
        created.status, created.body
    );

    let options = json!({"Enabled": true, "Catalog": {"ServiceManagedS3Tables": {}}});
    let dry_run = update_config(
        &router,
        name,
        &json!({"CatalogOptions": options.clone(), "DryRun": true}),
    )
    .await;
    assert_eq!(
        dry_run.at("dry run", "/DomainConfig/CatalogOptions/Options"),
        &options,
        "case \"dry run\": the dry run must report what the change would produce, got {:?}",
        dry_run.body.pointer("/DomainConfig/CatalogOptions/Options")
    );
    assert!(
        describe_config(&router, name)
            .await
            .missing("/DomainConfig/CatalogOptions"),
        "case \"dry run\": a dry run must store no catalog options"
    );
    assert!(
        describe(&router, name)
            .await
            .missing("/DomainStatus/CatalogOptions"),
        "case \"dry run\": a dry run must leave the domain status without catalog options"
    );
}

/// `CatalogOptions$Enabled` is `@required`, so a request supplying the structure without
/// it is refused at the boundary rather than stored.
#[tokio::test]
async fn catalog_options_without_the_required_switch_are_refused() {
    let router = router();
    let name = "catalog-missing-switch";
    let mut body = baseline(name);
    body["CatalogOptions"] = json!({"Catalog": {"ServiceManagedS3Tables": {}}});

    let refused = create(&router, &body).await;
    assert_eq!(
        refused.status, 400,
        "case \"missing Enabled\": the create must be refused, got {} {}",
        refused.status, refused.body
    );
    assert!(
        describe(&router, name).await.status == 404,
        "case \"missing Enabled\": the refused create must have stored no domain, got {}",
        describe(&router, name).await.body
    );
}
