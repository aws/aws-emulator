// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service package registry: `CreatePackage`
//! and the `DescribePackages` read path it is proved through.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, only the
//! package paths it drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a create mints ONE package record and answers it through the same projection a
//!   later read reports, over a table of six creates spanning every `PackageType`
//!   value, the description bound at both ends and absent, both engine lines, and
//!   the configuration, vending and encryption containers present and absent;
//! * a package is minted in a transient status and the clock owes it the status its
//!   copy leaves it in: every row is `COPYING` with no available version the instant
//!   the create returns, and `AVAILABLE` at version one once the copy span has
//!   passed, without any call having asked for the move;
//! * a second create of a name the account already holds is refused and leaves the
//!   stored record exactly as it was, over every row of the table;
//! * an engine version is accepted only for the one package type it may be given
//!   for, over every type and both engine lines;
//! * the listing reports exactly the stored set, narrowed by every filter field the
//!   model declares and cumulatively by two at once, and pages through the set
//!   without repeating or skipping a record whatever page size is asked for;
//! * the store is bounded: creates are accepted up to the quota and the one past it
//!   is refused, with the stored set left at the quota.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::PACKAGE_TYPE_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    published_packages, PACKAGE_ALREADY_IN_DESIRED_STATE, PACKAGE_QUOTA,
    PACKAGE_STATUS_AVAILABLE, PACKAGE_STATUS_COPYING, PACKAGE_TYPE_ZIP_PLUGIN,
};

use super::domain_lifecycle::{error_code, poll_until, Answer, Emulator, PREFIX};

/// `CreatePackage`: `POST /2021-01-01/packages`.
async fn create(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages"), Some(body))
        .await
}

/// `DescribePackages`: `POST /2021-01-01/packages/describe`.
async fn describe(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages/describe"), Some(body))
        .await
}

/// The `PackageDetails` of a create's answer, with the case named on failure.
fn details(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let details = &answer.body["PackageDetails"];
    assert!(
        details.is_object(),
        "case {case}: expected a PackageDetails object, got {:?}",
        answer.body
    );
    details.clone()
}

/// The entries of a listing that are packages of THIS ACCOUNT.
///
/// `DescribePackages` reports "all packages available to Amazon OpenSearch Service" —
/// the plugin packages the SERVICE publishes beside the packages the account created
/// — and a published package is owned by its publisher, not by the account, so a case
/// about what a CREATE stored narrows the listing by `PackageOwner`.
fn account_owned(reported: &[Value]) -> Vec<Value> {
    reported
        .iter()
        .filter(|package| {
            package["PackageOwner"].as_str() == Some(engine::emulator_account_id())
        })
        .cloned()
        .collect()
}

/// The names the SERVICE's published catalog reports for one filter field and value,
/// so a case about a filter expects the account's matching packages AND the published
/// ones. The mapping from a `DescribePackagesFilterName` to the `PackageDetails`
/// member it selects on is restated here as the suite's own expectation.
fn published_matching(field: &str, value: &str) -> Vec<String> {
    published_packages()
        .into_iter()
        .filter(|package| match field {
            "PackageID" => package.package_id == value,
            "PackageName" => package.package_name == value,
            "PackageStatus" => package.status == value,
            "PackageType" => package.package_type == value,
            "EngineVersion" => package.engine_version.as_deref() == Some(value),
            "PackageOwner" => package.owner == value,
            _ => false,
        })
        .map(|package| package.package_name)
        .collect()
}

/// The `PackageDetailsList` of a listing, with the case named on failure.
fn reported(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageDetailsList array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The reported packages' ids, in the order the listing reported them.
fn reported_ids(answer: &Answer, case: &str) -> Vec<String> {
    reported(answer, case)
        .iter()
        .map(|package| {
            package["PackageID"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a PackageID, got {package:?}"))
                .to_string()
        })
        .collect()
}

/// A required string member of a `PackageDetails`, with the case named on failure.
fn member(package: &Value, name: &str, case: &str) -> String {
    package[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {name}, got {package:?}"))
        .to_string()
}

/// Whether `value` is a `PackageID` of the form a created package takes: the model's
/// pattern is `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`, whose second alternative is the one
/// a caller's own package is addressed by.
fn is_created_package_id(value: &str) -> bool {
    value
        .strip_prefix("pkg-")
        .is_some_and(|body| !body.is_empty() && body.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && value == value.to_ascii_lowercase()
}

/// One create to make.
struct Row {
    /// `PackageName`.
    name: &'static str,
    /// `PackageType`.
    package_type: &'static str,
    /// `PackageDescription`, absent where `None`.
    description: Option<String>,
    /// `EngineVersion`, absent where `None`.
    engine_version: Option<&'static str>,
    /// `PackageConfiguration`, absent where `None`.
    configuration: Option<Value>,
    /// `PackageVendingOptions`, absent where `None`.
    vending: Option<Value>,
    /// `PackageEncryptionOptions`, absent where `None`.
    encryption: Option<Value>,
}

impl Row {
    /// This row as a `CreatePackage` request body: every member the row carries, and
    /// none it does not.
    fn body(&self) -> Value {
        let mut body = json!({
            "PackageName": self.name,
            "PackageType": self.package_type,
            "PackageSource": {
                "S3BucketName": "my-package-bucket",
                "S3Key": format!("packages/{}.zip", self.name),
            },
        });
        if let Some(description) = &self.description {
            body["PackageDescription"] = json!(description);
        }
        if let Some(engine_version) = self.engine_version {
            body["EngineVersion"] = json!(engine_version);
        }
        if let Some(configuration) = &self.configuration {
            body["PackageConfiguration"] = configuration.clone();
        }
        if let Some(vending) = &self.vending {
            body["PackageVendingOptions"] = vending.clone();
        }
        if let Some(encryption) = &self.encryption {
            body["PackageEncryptionOptions"] = encryption.clone();
        }
        body
    }
}

/// The create table: one row per way an accepted create differs from another — every
/// `PackageType` value, the name at its shortest and longer, no description / an
/// empty one / one at its longest, both engine lines on the type that takes one, and
/// the configuration, vending and encryption containers present and absent.
///
/// `PackageName` is `@length(min: 3, max: 256)` pattern `^[a-z][a-z0-9\-]+$`,
/// `PackageDescription` is `@length(min: 0, max: 1024)`, `EngineVersion` is pattern
/// `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$`, and
/// `PackageType`'s values are the four below — all from this service's Smithy model
/// (`smithy_query(query="operation",operation="CreatePackage")`), whose value set is
/// also asserted against `PACKAGE_TYPE_VALUES` by [`every_package_type_is_covered`].
fn create_rows() -> Vec<Row> {
    vec![
        // The shortest name the constraint allows, and nothing optional at all.
        Row {
            name: "abc",
            package_type: "TXT-DICTIONARY",
            description: None,
            engine_version: None,
            configuration: None,
            vending: None,
            encryption: None,
        },
        // A dictionary with the shortest description the bound allows (empty) and
        // encryption explicitly off.
        Row {
            name: "my-synonyms-dictionary",
            package_type: "TXT-DICTIONARY",
            description: Some(String::new()),
            engine_version: None,
            configuration: None,
            vending: None,
            encryption: Some(json!({ "EncryptionEnabled": false })),
        },
        // A plugin for the OpenSearch engine line, with every optional container.
        Row {
            name: "custom-analyzer-plugin",
            package_type: PACKAGE_TYPE_ZIP_PLUGIN,
            description: Some("Analyzer plugin".to_string()),
            engine_version: Some("OpenSearch_2.11"),
            configuration: Some(json!({
                "LicenseRequirement": "REQUIRED",
                "LicenseFilepath": "licenses/LICENSE.txt",
                "ConfigurationRequirement": "OPTIONAL",
                "RequiresRestartForConfigurationUpdate": true,
            })),
            vending: Some(json!({ "VendingEnabled": true })),
            encryption: Some(json!({
                "EncryptionEnabled": true,
                "KmsKeyIdentifier": "alias/aws/es",
            })),
        },
        // A plugin for the legacy Elasticsearch engine line, with nothing else.
        Row {
            name: "legacy-era-plugin",
            package_type: PACKAGE_TYPE_ZIP_PLUGIN,
            description: None,
            engine_version: Some("Elasticsearch_7.10"),
            configuration: None,
            vending: None,
            encryption: None,
        },
        // A configuration package with the longest description the bound allows.
        Row {
            name: "cluster-config-package",
            package_type: "PACKAGE-CONFIG",
            description: Some("d".repeat(1024)),
            engine_version: None,
            configuration: Some(json!({
                "LicenseRequirement": "NONE",
                "ConfigurationRequirement": "REQUIRED",
            })),
            vending: Some(json!({ "VendingEnabled": false })),
            encryption: None,
        },
        // A license package, vended to other users.
        Row {
            name: "plugin-license-package",
            package_type: "PACKAGE-LICENSE",
            description: Some("Licence for the analyzer plugin".to_string()),
            engine_version: None,
            configuration: Some(json!({
                "LicenseRequirement": "OPTIONAL",
                "ConfigurationRequirement": "NONE",
                "RequiresRestartForConfigurationUpdate": false,
            })),
            vending: Some(json!({ "VendingEnabled": true })),
            encryption: Some(json!({ "EncryptionEnabled": false })),
        },
    ]
}

/// The create table spans the whole `PackageType` catalog, so a value added to the
/// model is not silently left untested.
#[test]
fn every_package_type_is_covered() {
    for package_type in PACKAGE_TYPE_VALUES {
        assert!(
            create_rows()
                .iter()
                .any(|row| row.package_type == *package_type),
            "case PackageType={package_type}: the create table covers every modelled \
             package type, got {:?}",
            create_rows()
                .iter()
                .map(|row| row.package_type)
                .collect::<Vec<_>>()
        );
    }
}

/// A create answers the record it persisted, and the listing reports that same
/// record: every member the request carried is reported back, the id has the form a
/// created package's id takes, and no two packages share one.
#[tokio::test]
async fn a_create_mints_one_package_the_listing_then_reports() {
    let emulator = Emulator::new();

    let mut created: Vec<(String, Value)> = Vec::new();
    for row in create_rows() {
        let case = format!("CreatePackage({})", row.name);
        let answer = create(&emulator, row.body()).await;
        let package = details(&answer, &case);

        let id = member(&package, "PackageID", &case);
        assert!(
            is_created_package_id(&id),
            "case {case}: expected a pkg-<hex> PackageID, got {id:?}"
        );
        assert_eq!(
            member(&package, "PackageName", &case),
            row.name,
            "case {case}: the package carries the name the create gave it, got {package:?}"
        );
        assert_eq!(
            member(&package, "PackageType", &case),
            row.package_type,
            "case {case}: the package carries the type the create gave it, got {package:?}"
        );
        assert_eq!(
            package["PackageDescription"].as_str(),
            row.description.as_deref(),
            "case {case}: the package carries the description the create gave it, got {package:?}"
        );
        assert_eq!(
            package["EngineVersion"].as_str(),
            row.engine_version,
            "case {case}: the package carries the engine version the create gave it, got \
             {package:?}"
        );
        for (member_name, sent) in [
            ("AvailablePackageConfiguration", row.configuration.as_ref()),
            ("PackageVendingOptions", row.vending.as_ref()),
            ("PackageEncryptionOptions", row.encryption.as_ref()),
        ] {
            match sent {
                None => assert!(
                    package[member_name].is_null(),
                    "case {case}: a create that carried no {member_name} reports none, got {:?}",
                    package[member_name]
                ),
                Some(sent) => assert_eq!(
                    &package[member_name], sent,
                    "case {case}: {member_name} is reported as the create gave it, got {:?}",
                    package[member_name]
                ),
            }
        }
        assert_eq!(
            package["PackageStatus"].as_str(),
            Some(PACKAGE_STATUS_COPYING),
            "case {case}: a package's content is still being copied the instant the create \
             returns, got {package:?}"
        );
        assert!(
            package["AvailablePackageVersion"].is_null(),
            "case {case}: a package whose content is not in place yet has no available \
             version, got {:?}",
            package["AvailablePackageVersion"]
        );
        let created_at = package["CreatedAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: expected a numeric CreatedAt, got {package:?}")
        });
        let updated_at = package["LastUpdatedAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: expected a numeric LastUpdatedAt, got {package:?}")
        });
        assert!(
            (created_at - updated_at).abs() < f64::EPSILON,
            "case {case}: a package nothing has changed yet was last updated when it was \
             created, got {created_at} and {updated_at}"
        );
        assert_eq!(
            member(&package, "PackageOwner", &case),
            engine::emulator_account_id(),
            "case {case}: the package belongs to the account that created it, got {package:?}"
        );
        assert!(
            package["AllowListedUserList"].is_null(),
            "case {case}: a package shared with nobody reports no allow list, got {:?}",
            package["AllowListedUserList"]
        );

        assert!(
            created.iter().all(|(seen, _)| seen != &id),
            "case {case}: no two packages share a PackageID, got {id:?} twice"
        );
        created.push((id, package));
    }

    // The listing reports EXACTLY the stored set beside the catalog the service
    // publishes, each stored record as the create answered it: the create's answer and
    // a later read cannot disagree.
    let listing = describe(&emulator, json!({})).await;
    let whole_list = reported(&listing, "DescribePackages(unpaged)");
    let reported_list = account_owned(&whole_list);
    assert_eq!(
        reported_list.len(),
        created.len(),
        "case DescribePackages(unpaged): expected every created package, got {reported_list:?}"
    );
    assert_eq!(
        whole_list.len(),
        created.len() + published_packages().len(),
        "case DescribePackages(unpaged): expected the account's packages beside the \
         published catalog, got {} entries",
        whole_list.len()
    );
    for (id, package) in &created {
        let found = reported_list
            .iter()
            .find(|reported| reported["PackageID"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!("case DescribePackages({id}): the listing reports it, got {reported_list:?}")
            });
        assert_eq!(
            found, package,
            "case DescribePackages({id}): reported as the create answered it, got {found:?}"
        );
    }
    assert!(
        listing.body["NextToken"].is_null(),
        "case DescribePackages(unpaged): a page that ended the set carries no token, got {:?}",
        listing.body["NextToken"]
    );
}

/// The clock owes a freshly created package the status its copy leaves it in: no call
/// asks for the move, and once the span has passed every package is reported
/// available at its first version.
#[tokio::test]
async fn a_package_becomes_available_once_the_copy_span_has_passed() {
    let emulator = Emulator::new();

    let mut created_at: Vec<(String, f64)> = Vec::new();
    for row in create_rows() {
        let case = format!("CreatePackage({})", row.name);
        let package = details(&create(&emulator, row.body()).await, &case);
        created_at.push((
            member(&package, "PackageID", &case),
            package["CreatedAt"].as_f64().unwrap_or_default(),
        ));
    }

    // The CONDITION the clock owes, polled rather than waited out: every created
    // package is reported in the status its copy left it in, however fast the host runs.
    poll_until("the copies the clock owed have landed", || async {
        account_owned(&reported(
            &describe(&emulator, json!({})).await,
            "DescribePackages(settling)",
        ))
        .iter()
        .all(|package| package["PackageStatus"].as_str() == Some(PACKAGE_STATUS_AVAILABLE))
    })
    .await;

    let listing = describe(&emulator, json!({})).await;
    for package in account_owned(&reported(&listing, "DescribePackages(settled)")) {
        let id = member(&package, "PackageID", "DescribePackages(settled)");
        let case = format!("DescribePackages(settled {id})");
        assert_eq!(
            package["PackageStatus"].as_str(),
            Some(PACKAGE_STATUS_AVAILABLE),
            "case {case}: the copy has fallen due, so the package is available, got {package:?}"
        );
        assert_eq!(
            package["AvailablePackageVersion"].as_str(),
            Some("1"),
            "case {case}: an available package carries its first version, got {package:?}"
        );
        let created = created_at
            .iter()
            .find(|(seen, _)| seen == &id)
            .map(|(_, created)| *created)
            .unwrap_or_else(|| panic!("case {case}: the listing reports only created packages"));
        let updated = package["LastUpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated > created,
            "case {case}: the status move the clock owed changed the record, so it was last \
             updated after it was created, got {updated} and {created}"
        );
    }

    // A second read reports the same settled record: materialising what the clock owed
    // is not a change a later caller sees differently.
    let again = describe(&emulator, json!({})).await;
    assert_eq!(
        reported(&again, "DescribePackages(settled twice)"),
        reported(&listing, "DescribePackages(settled)"),
        "case DescribePackages(settled twice): two reads of a settled set agree"
    );
}

/// A package name is unique across the account's packages: a second create of a name
/// the account holds is refused with the modelled `ResourceAlreadyExistsException`
/// and leaves the stored record exactly as it was.
#[tokio::test]
async fn a_duplicate_package_name_is_refused() {
    let emulator = Emulator::new();

    for row in create_rows() {
        let case = format!("CreatePackage({})", row.name);
        let first = details(&create(&emulator, row.body()).await, &case);

        // The same name again, with everything else the request may vary changed, so
        // what is refused is the NAME rather than a repeat of one request.
        let mut second_body = row.body();
        second_body["PackageDescription"] = json!("a second create of the same name");
        second_body["PackageSource"] = json!({
            "S3BucketName": "another-package-bucket",
            "S3Key": "packages/another.zip",
        });
        let second = create(&emulator, second_body).await;
        assert_eq!(
            second.status, 400,
            "case {case} again: expected 400, got {} with {:?}",
            second.status, second.body
        );
        assert_eq!(
            error_code(&second),
            "BaseException",
            "case {case} again: expected the generic envelope, got {:?}",
            second.body
        );
        assert_eq!(
            second.body["message"],
            json!(PACKAGE_ALREADY_IN_DESIRED_STATE),
            "case {case} again: expected the recorded sentence, got {:?}",
            second.body
        );

        let id = member(&first, "PackageID", &case);
        let listing = describe(
            &emulator,
            json!({ "Filters": [{ "Name": "PackageName", "Value": [row.name] }] }),
        )
        .await;
        let held = reported(&listing, &case);
        assert_eq!(
            held.len(),
            1,
            "case {case} again: the refused create added no record, got {held:?}"
        );
        assert_eq!(
            held[0]["PackageID"].as_str(),
            Some(id.as_str()),
            "case {case} again: the stored record is the one the first create minted, got {:?}",
            held[0]
        );
        assert_eq!(
            held[0]["PackageDescription"].as_str(),
            first["PackageDescription"].as_str(),
            "case {case} again: the refused create changed nothing, got {:?}",
            held[0]
        );
    }
}

/// An engine version may be given only for the one package type it applies to: every
/// other type is refused with the modelled `InvalidTypeException`, and nothing is
/// stored for the refused create.
#[tokio::test]
async fn an_engine_version_is_accepted_only_for_the_plugin_package_type() {
    let emulator = Emulator::new();

    // Both engine lines, at the bounds the version pattern allows for each.
    for engine_version in [
        "OpenSearch_2.11",
        "OpenSearch_1.0",
        "Elasticsearch_7.10",
        "Elasticsearch_1.5",
    ] {
        for package_type in PACKAGE_TYPE_VALUES {
            let name = format!(
                "engine-version-{}-{}",
                engine_version.to_ascii_lowercase().replace(['_', '.'], "-"),
                package_type.to_ascii_lowercase()
            );
            let case = format!("CreatePackage({package_type} with {engine_version})");
            let answer = create(
                &emulator,
                json!({
                    "PackageName": name,
                    "PackageType": package_type,
                    "EngineVersion": engine_version,
                    "PackageSource": { "S3BucketName": "my-package-bucket", "S3Key": "p.zip" },
                }),
            )
            .await;

            if *package_type == PACKAGE_TYPE_ZIP_PLUGIN {
                let package = details(&answer, &case);
                assert_eq!(
                    package["EngineVersion"].as_str(),
                    Some(engine_version),
                    "case {case}: the plugin carries the engine version it was created for, got \
                     {package:?}"
                );
            } else {
                assert_eq!(
                    answer.status, 409,
                    "case {case}: expected 409, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    error_code(&answer),
                    "InvalidTypeException",
                    "case {case}: expected the modelled invalid-type refusal, got {:?}",
                    answer.body
                );
                let listing = describe(
                    &emulator,
                    json!({ "Filters": [{ "Name": "PackageName", "Value": [name] }] }),
                )
                .await;
                assert!(
                    reported(&listing, &case).is_empty(),
                    "case {case}: a refused create stores nothing, got {:?}",
                    listing.body
                );
            }
        }
    }
}

/// The listing selects on every filter field the model declares, on at least one of
/// each filter's values, and cumulatively across filters.
#[tokio::test]
async fn the_listing_selects_on_every_modelled_filter_field() {
    let emulator = Emulator::new();

    let mut ids: Vec<(String, String)> = Vec::new();
    for row in create_rows() {
        let case = format!("CreatePackage({})", row.name);
        let package = details(&create(&emulator, row.body()).await, &case);
        ids.push((
            row.name.to_string(),
            member(&package, "PackageID", &case),
        ));
    }

    // One filter per field `DescribePackagesFilterName` declares, each asked with the
    // value of one stored package and expected to select exactly the packages that
    // carry it.
    let account = engine::emulator_account_id().to_string();
    let rows = create_rows();
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "PackageID",
            ids[2].1.clone(),
            vec![ids[2].0.clone()],
        ),
        (
            "PackageName",
            rows[0].name.to_string(),
            vec![rows[0].name.to_string()],
        ),
        (
            "PackageType",
            PACKAGE_TYPE_ZIP_PLUGIN.to_string(),
            rows.iter()
                .filter(|row| row.package_type == PACKAGE_TYPE_ZIP_PLUGIN)
                .map(|row| row.name.to_string())
                .collect(),
        ),
        (
            "PackageStatus",
            PACKAGE_STATUS_COPYING.to_string(),
            rows.iter().map(|row| row.name.to_string()).collect(),
        ),
        (
            "EngineVersion",
            "OpenSearch_2.11".to_string(),
            rows.iter()
                .filter(|row| row.engine_version == Some("OpenSearch_2.11"))
                .map(|row| row.name.to_string())
                .collect(),
        ),
        (
            "PackageOwner",
            account.clone(),
            rows.iter().map(|row| row.name.to_string()).collect(),
        ),
        // A value no stored package carries selects nothing, rather than everything.
        ("PackageName", "no-such-package".to_string(), Vec::new()),
        (
            "PackageStatus",
            PACKAGE_STATUS_AVAILABLE.to_string(),
            Vec::new(),
        ),
    ];

    for (field, value, expected) in cases {
        let case = format!("DescribePackages({field}={value})");
        let listing = describe(
            &emulator,
            json!({ "Filters": [{ "Name": field, "Value": [value] }] }),
        )
        .await;
        let mut names: Vec<String> = reported(&listing, &case)
            .iter()
            .map(|package| member(package, "PackageName", &case))
            .collect();
        names.sort();
        let mut expected = expected;
        expected.extend(published_matching(field, &value));
        expected.sort();
        assert_eq!(
            names, expected,
            "case {case}: the filter selects exactly the packages carrying that value"
        );
    }

    // A filter naming a field outside `DescribePackagesFilterName`'s value set is
    // refused at the request boundary, so no such filter ever reaches the selection.
    let case = "DescribePackages(unmodelled filter field)";
    let answer = describe(
        &emulator,
        json!({ "Filters": [{ "Name": "PackageSource", "Value": [rows[0].name] }] }),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case {case}: expected the modelled validation refusal, got {:?}",
        answer.body
    );

    // The values within ONE filter are alternatives, and two filters are cumulative:
    // the plugin rows named by either of two names, narrowed to the one that is also
    // a plugin.
    let case = "DescribePackages(two filters)";
    let listing = describe(
        &emulator,
        json!({
            "Filters": [
                { "Name": "PackageName", "Value": [rows[0].name, rows[2].name] },
                { "Name": "PackageType", "Value": [PACKAGE_TYPE_ZIP_PLUGIN] },
            ],
        }),
    )
    .await;
    let names: Vec<String> = reported(&listing, case)
        .iter()
        .map(|package| member(package, "PackageName", case))
        .collect();
    assert_eq!(
        names,
        vec![rows[2].name.to_string()],
        "case {case}: a package must match every filter and one value of each"
    );
}

/// The listing pages through the set without repeating or skipping a record, however
/// the set is cut, and refuses a token that names no place in it.
#[tokio::test]
async fn the_listing_pages_the_whole_set_once() {
    let emulator = Emulator::new();

    for row in create_rows() {
        let case = format!("CreatePackage({})", row.name);
        details(&create(&emulator, row.body()).await, &case);
    }

    let whole = describe(&emulator, json!({})).await;
    let everything = reported_ids(&whole, "unpaged");

    // Every page size from one to past the whole set, plus the model's own maximum:
    // the pages concatenate to the unpaged answer exactly once however the set is cut.
    for size in [1, 2, 3, 5, 6, 7, 100] {
        let case = format!("MaxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let answer = describe(&emulator, request).await;
            let page = reported_ids(&answer, &case);
            assert!(
                page.len() <= size,
                "case {case} page {pages}: a page is never longer than the size asked for, got \
                 {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= everything.len() + 1,
                "case {case}: paging must terminate, walked {walked:?}"
            );
            match answer.body["NextToken"].as_str() {
                None => break,
                Some(next) => token = Some(next.to_string()),
            }
        }
        assert_eq!(
            walked, everything,
            "case {case}: the pages concatenate to the whole set, in the same order"
        );
    }

    // A token this operation never issued names no place in the set: refused, rather
    // than answered with a page.
    for token in ["pkg-deadbeef", "", "0", "not a token"] {
        let case = format!("NextToken={token:?}");
        let answer = describe(&emulator, json!({ "NextToken": token })).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

/// The store is bounded: creates are accepted up to the quota and the one past it is
/// refused with the modelled `LimitExceededException`, leaving the stored set at the
/// quota.
#[tokio::test]
async fn the_package_store_is_bounded() {
    let emulator = Emulator::new();

    for index in 0..PACKAGE_QUOTA {
        let case = format!("CreatePackage(quota-{index})");
        let answer = create(
            &emulator,
            json!({
                "PackageName": format!("quota-package-{index}"),
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": { "S3BucketName": "my-package-bucket", "S3Key": "p.txt" },
            }),
        )
        .await;
        details(&answer, &case);
    }

    let case = "CreatePackage(past the quota)";
    let answer = create(
        &emulator,
        json!({
            "PackageName": "quota-package-past-the-end",
            "PackageType": "TXT-DICTIONARY",
            "PackageSource": { "S3BucketName": "my-package-bucket", "S3Key": "p.txt" },
        }),
    )
    .await;
    assert_eq!(
        answer.status, 409,
        "case {case}: expected 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "LimitExceededException",
        "case {case}: expected the modelled limit refusal, got {:?}",
        answer.body
    );

    let listing = describe(&emulator, json!({ "MaxResults": 100 })).await;
    assert_eq!(
        account_owned(&reported(&listing, case)).len(),
        PACKAGE_QUOTA,
        "case {case}: the refused create left the stored set at the quota"
    );
}
