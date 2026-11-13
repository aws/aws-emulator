// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTHORIZATION of the OpenSearch Service operations that act
//! on a PACKAGE and on an application's saved-object MIGRATION jobs, driven over the real
//! wire path (a `restJson1` HTTP request through the multi-service router into the mounted
//! service), so what is asserted is what a client sees.
//!
//! The rule under test: before it reads or writes anything, each of those operations
//! evaluates the identity-based IAM policies in force on the principal the request's
//! credentials name against `es:<OperationName>` on the resource the request addresses —
//! the account's domain resource for a call addressed by package id, the named domain for
//! one addressed by domain name, and the account's application resource for a call about
//! an application's migration jobs — and a request no statement allows, or one an explicit
//! `Deny` covers, is refused with the modelled `AccessDeniedException` (HTTP 403), performs
//! nothing, and is told nothing about whether the resource it named exists.
//!
//! The rows differ along what the rule generalises over: which operation is called (nine
//! package operations and three migration ones), which principal form signs the request
//! (an assumed-role session and an IAM user signing with its own long-term key), whether
//! the resource the request names exists, and the statement's action pattern, resource
//! scope and effect.
//!
//! The principals, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents
//! under test are the ones `iam:PutRolePolicy` and `iam:PutUserPolicy` store and the
//! principal is the one `sts:GetCallerIdentity` answers for the key the request signed
//! with. The wire harness comes from [`super::domain_lifecycle`], the principals from
//! [`super::data_source_attachment_authorization`] and the refusal wording from
//! [`super::index_authorization`]; this suite adds no second copy of any of them.

use serde_json::{json, Value};

use super::data_source_attachment_authorization::{given_policy, given_principals, Principal};
use super::data_source_attachments::application;
use super::data_source_registry::created_domain;
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, REGION,
};
use super::index_authorization::{allow, assert_refused, Refusal};

/// The operations addressed by PACKAGE id, which this service's IAM actions decide
/// against the account's DOMAIN resource: a package is not a domain, and its content is
/// what a domain's indexes read (`smithy_query(query="digest")`: each models
/// `AccessDeniedException`, 403).
const PACKAGE_OPERATIONS: [&str; 6] = [
    "CreatePackage",
    "DeletePackage",
    "DescribePackages",
    "UpdatePackage",
    "GetPackageVersionHistory",
    "ListDomainsForPackage",
];

/// The package operations that name a DOMAIN, which are decided against the ARN of that
/// domain the way the index operations are.
const DOMAIN_SCOPED_OPERATIONS: [&str; 3] = [
    "AssociatePackage",
    "DissociatePackage",
    "ListPackagesForDomain",
];

/// The operations on an application's saved-object migration jobs, which are decided
/// against the account's APPLICATION resource — the resource the recorded refusal of a
/// migration read names.
const MIGRATION_OPERATIONS: [&str; 3] = ["StartMigration", "GetMigration", "ListMigrations"];

/// The domain the fixture holds, which the domain-scoped rows address.
const DOMAIN: &str = "package-authz-domain";

/// The data source a started migration exports from: the search-collection ARN
/// `StartMigration` validates a source against.
const MIGRATION_SOURCE: &str = "arn:aws:aoss:us-east-1:123456789012:collection/authzobjects";

/// A `PackageID` of the modelled form that names no package of this account
/// (`PackageID`'s pattern is `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`).
const UNKNOWN_PACKAGE_ID: &str = "pkg-00000000000000000000000000000000";

/// The `Resource` the package operations addressed by package id are decided against:
/// every domain of the account and Region the request was signed for, composed here from
/// the partition, this service's namespace, the Region, the account this emulator serves
/// and the domain resource pattern — so the assertion does not ask the service to confirm
/// its own answer.
fn every_domain_arn() -> String {
    format!(
        "arn:aws:es:{REGION}:{}:domain/*",
        ::engine::emulator_account_id()
    )
}

/// The `Resource` a package operation that names a domain is decided against: that
/// domain's own ARN.
fn one_domain_arn(domain: &str) -> String {
    format!(
        "arn:aws:es:{REGION}:{}:domain/{domain}",
        ::engine::emulator_account_id()
    )
}

/// The `Resource` the migration operations are decided against: every application of the
/// account and Region the request was signed for, under the service token the OpenSearch
/// user-interface plane addresses an application with.
fn every_application_arn() -> String {
    format!(
        "arn:aws:opensearch:{REGION}:{}:application/*",
        ::engine::emulator_account_id()
    )
}

/// One operation of the three surfaces, signed with `access_key_id`, against the
/// package, the domain and the application the fixture holds.
async fn surface_call(
    emulator: &Emulator,
    access_key_id: &str,
    operation: &str,
    package_id: &str,
    application_id: &str,
) -> Answer {
    let source = json!({ "S3BucketName": "my-package-bucket", "S3Key": "packages/authz.zip" });
    match operation {
        "CreatePackage" => {
            emulator
                .call_as(
                    access_key_id,
                    "POST",
                    &format!("{PREFIX}/packages"),
                    Some(json!({
                        "PackageName": "authz-probe-package",
                        "PackageType": "TXT-DICTIONARY",
                        "PackageSource": source,
                    })),
                )
                .await
        }
        "DeletePackage" => {
            emulator
                .call_as(
                    access_key_id,
                    "DELETE",
                    &format!("{PREFIX}/packages/{package_id}"),
                    None,
                )
                .await
        }
        "DescribePackages" => {
            emulator
                .call_as(
                    access_key_id,
                    "POST",
                    &format!("{PREFIX}/packages/describe"),
                    Some(json!({})),
                )
                .await
        }
        "UpdatePackage" => {
            emulator
                .call_as(
                    access_key_id,
                    "POST",
                    &format!("{PREFIX}/packages/update"),
                    Some(json!({ "PackageID": package_id, "PackageSource": source })),
                )
                .await
        }
        "GetPackageVersionHistory" => {
            emulator
                .call_as(
                    access_key_id,
                    "GET",
                    &format!("{PREFIX}/packages/{package_id}/history"),
                    None,
                )
                .await
        }
        "ListDomainsForPackage" => {
            emulator
                .call_as(
                    access_key_id,
                    "GET",
                    &format!("{PREFIX}/packages/{package_id}/domains"),
                    None,
                )
                .await
        }
        "AssociatePackage" => {
            emulator
                .call_as(
                    access_key_id,
                    "POST",
                    &format!("{PREFIX}/packages/associate/{package_id}/{DOMAIN}"),
                    Some(json!({})),
                )
                .await
        }
        "DissociatePackage" => {
            emulator
                .call_as(
                    access_key_id,
                    "POST",
                    &format!("{PREFIX}/packages/dissociate/{package_id}/{DOMAIN}"),
                    Some(json!({})),
                )
                .await
        }
        "ListPackagesForDomain" => {
            emulator
                .call_as(
                    access_key_id,
                    "GET",
                    &format!("{PREFIX}/domain/{DOMAIN}/packages"),
                    None,
                )
                .await
        }
        "StartMigration" => {
            emulator
                .call_as(
                    access_key_id,
                    "POST",
                    &format!("{PREFIX}/opensearch/app-migrations"),
                    Some(json!({
                        "applicationId": application_id,
                        "migrationOptions": {
                            "source": { "datasourceArn": MIGRATION_SOURCE },
                            "workspace": { "workspaceId": "ws-authz" },
                        },
                    })),
                )
                .await
        }
        "GetMigration" => {
            emulator
                .call_as(
                    access_key_id,
                    "GET",
                    &format!("{PREFIX}/opensearch/app-migrations/{application_id}-1"),
                    None,
                )
                .await
        }
        "ListMigrations" => {
            emulator
                .call_as(
                    access_key_id,
                    "GET",
                    &format!(
                        "{PREFIX}/opensearch/app-migrations?applicationId={application_id}"
                    ),
                    None,
                )
                .await
        }
        other => panic!("case {other:?}: not an operation of the surfaces under test"),
    }
}

/// The `Resource` `operation` is decided against.
fn resource_of(operation: &str) -> String {
    if MIGRATION_OPERATIONS.contains(&operation) {
        every_application_arn()
    } else if DOMAIN_SCOPED_OPERATIONS.contains(&operation) {
        one_domain_arn(DOMAIN)
    } else {
        every_domain_arn()
    }
}

/// One emulator holding a package, a domain and an application, with both principals
/// recorded — the precondition each case below starts from.
struct Fixture {
    emulator: Emulator,
    /// The `PackageID` of the package every package row addresses.
    package_id: String,
    /// The application every migration row addresses.
    application_id: String,
}

impl Fixture {
    async fn new(package_name: &str) -> Self {
        let emulator = Emulator::new();
        given_principals(&emulator);
        created_domain(&emulator, DOMAIN).await;
        let application_id = application(&emulator, "packageauthz").await;
        let created = emulator
            .call(
                "POST",
                &format!("{PREFIX}/packages"),
                Some(json!({
                    "PackageName": package_name,
                    "PackageType": "TXT-DICTIONARY",
                    "PackageSource": {
                        "S3BucketName": "my-package-bucket",
                        "S3Key": format!("packages/{package_name}.txt"),
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "the fixture's privileged create must be served, got {} with {:?}",
            created.status, created.body
        );
        let package_id = string_member(&created.body["PackageDetails"], "PackageID", package_name);
        Self {
            emulator,
            package_id,
            application_id,
        }
    }

    /// The ids of the packages this account holds, read as the account's own identity so
    /// the read is never the call under test.
    async fn held_packages(&self, case: &str) -> Vec<String> {
        let answer = self
            .emulator
            .call(
                "POST",
                &format!("{PREFIX}/packages/describe"),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: the privileged listing must be served, got {} with {:?}",
            answer.status, answer.body
        );
        answer.body["PackageDetailsList"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry["PackageID"].as_str().unwrap_or_default().to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The migration jobs the fixture's application holds, read as the account's own
    /// identity for the same reason.
    async fn held_migrations(&self, case: &str) -> Vec<String> {
        let answer = self
            .emulator
            .call(
                "GET",
                &format!(
                    "{PREFIX}/opensearch/app-migrations?applicationId={}",
                    self.application_id
                ),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: the privileged listing must be served, got {} with {:?}",
            answer.status, answer.body
        );
        answer.body["migrations"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry["migrationId"].as_str().unwrap_or_default().to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// RULE: every operation of the package and migration surfaces takes the decision — not
/// only the one a report happened to name — so a principal no identity-based policy
/// permits is refused with 403 on each of them, in IAM's own wording and naming the
/// resource that operation is decided against, and the refused calls leave the account's
/// packages and the application's jobs exactly as they were. Both principal forms this
/// emulator reads policies for are driven through it.
#[tokio::test]
async fn every_package_and_migration_operation_refuses_a_principal_no_policy_permits() {
    for principal in [Principal::Session, Principal::User] {
        let fixture = Fixture::new("authz-held-package").await;
        given_policy(&fixture.emulator, principal, None);
        let packages_before = fixture.held_packages(principal.case()).await;
        let migrations_before = fixture.held_migrations(principal.case()).await;

        for operation in PACKAGE_OPERATIONS
            .iter()
            .chain(DOMAIN_SCOPED_OPERATIONS.iter())
            .chain(MIGRATION_OPERATIONS.iter())
        {
            let case = format!("{} calling {operation}", principal.case());
            let answer = surface_call(
                &fixture.emulator,
                principal.access_key_id(),
                operation,
                &fixture.package_id,
                &fixture.application_id,
            )
            .await;
            assert_refused(
                &answer,
                &case,
                &principal.arn(),
                &format!("es:{operation}"),
                &resource_of(operation),
                Refusal::DefaultDeny,
            );
            assert_eq!(
                fixture.held_packages(&case).await,
                packages_before,
                "case {case}: the refused call must leave the account's packages untouched"
            );
            assert_eq!(
                fixture.held_migrations(&case).await,
                migrations_before,
                "case {case}: the refused call must leave the application's jobs untouched"
            );
        }
    }
}

/// RULE: the decision follows the identity policies in force on the calling principal in
/// IAM's own order — an `Allow` matching the action AND the resource permits, an explicit
/// `Deny` wins whichever side of an `Allow` it is written on, and nothing matching is
/// IAM's default deny — and it is the same decision for either principal form. The rows
/// differ along the action pattern, the resource a statement is scoped to and the
/// statement's effect; the call is a package read, which changes nothing, so a row's
/// answer depends on its policy alone.
#[tokio::test]
async fn a_package_read_is_decided_by_the_policies_in_force_on_the_principal() {
    let account = ::engine::emulator_account_id();
    // Each row: the case, the policy the principal carries (`None` takes every policy
    // off it), and the expected HTTP status of the read.
    let rows: Vec<(&str, Option<Value>, u16)> = vec![
        ("no policy at all is IAM's default deny", None, 403),
        (
            "the exact action on the domain resource of this account",
            Some(allow(
                json!("es:GetPackageVersionHistory"),
                json!(every_domain_arn()),
            )),
            200,
        ),
        (
            "every action of this service on every resource",
            Some(allow(json!("es:*"), json!("*"))),
            200,
        ),
        (
            "an action pattern matching this one by prefix",
            Some(allow(json!("es:GetPackage*"), json!("*"))),
            200,
        ),
        (
            "only another operation of the same surface",
            Some(allow(json!("es:DescribePackages"), json!("*"))),
            403,
        ),
        (
            "another service's actions",
            Some(allow(json!("s3:GetObject"), json!("*"))),
            403,
        ),
        (
            "an Allow scoped to one domain rather than the account's domains",
            Some(allow(
                json!("es:GetPackageVersionHistory"),
                json!(format!("arn:aws:es:{REGION}:{account}:domain/{DOMAIN}")),
            )),
            403,
        ),
        (
            "an explicit deny of this action beside an allow of everything",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "es:*", "Resource": "*" },
                    {
                        "Effect": "Deny",
                        "Action": "es:GetPackageVersionHistory",
                        "Resource": "*"
                    }
                ]
            })),
            403,
        ),
    ];

    for principal in [Principal::Session, Principal::User] {
        let fixture = Fixture::new("authz-decided-package").await;
        for (case, policy, expected) in &rows {
            let case = format!("{} with {case}", principal.case());
            given_policy(&fixture.emulator, principal, policy.as_ref());
            let answer = surface_call(
                &fixture.emulator,
                principal.access_key_id(),
                "GetPackageVersionHistory",
                &fixture.package_id,
                &fixture.application_id,
            )
            .await;
            assert_eq!(
                answer.status, *expected,
                "case {case}: expected HTTP {expected}, got {} with {:?}",
                answer.status, answer.body
            );
            if *expected == 200 {
                assert!(
                    answer.body["PackageVersionHistoryList"].is_array(),
                    "case {case}: a permitted read reports the history, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// RULE: the decision is taken BEFORE the resource the request names is resolved, so a
/// principal no policy permits is refused with 403 whether or not that resource exists,
/// and the same request under a principal the policy permits reaches the operation's own
/// answer — the missing-resource refusal for a resource this account does not hold, and
/// the served answer for one it does.
#[tokio::test]
async fn authorization_is_decided_before_the_resource_is_resolved() {
    let fixture = Fixture::new("authz-ordering-package").await;
    let principal = Principal::Session;

    // A job this account DOES hold, started as the account's own identity so the setup is
    // never the call under test, and an id of the same shape that names no job.
    let started = fixture
        .emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/app-migrations"),
            Some(json!({
                "applicationId": fixture.application_id,
                "migrationOptions": {
                    "source": { "datasourceArn": MIGRATION_SOURCE },
                    "workspace": { "workspaceId": "ws-authz" },
                },
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "the fixture's privileged start must be served, got {} with {:?}",
        started.status, started.body
    );
    let migration_id = string_member(&started.body, "migrationId", "the started job");
    let unknown_migration = format!("{}-zzzzzzzz", fixture.application_id);

    // With no policy at all, the read of the job that EXISTS and the read of the one that
    // does not are the same authorization refusal — the caller is told nothing about
    // which is which.
    given_policy(&fixture.emulator, principal, None);
    for (case, uri, operation) in [
        (
            "a migration this account holds",
            format!("{PREFIX}/opensearch/app-migrations/{migration_id}"),
            "GetMigration",
        ),
        (
            "a migration this account does not hold",
            format!("{PREFIX}/opensearch/app-migrations/{unknown_migration}"),
            "GetMigration",
        ),
        (
            "a package this account holds",
            format!("{PREFIX}/packages/{}/history", fixture.package_id),
            "GetPackageVersionHistory",
        ),
        (
            "a package this account does not hold",
            format!("{PREFIX}/packages/{UNKNOWN_PACKAGE_ID}/history"),
            "GetPackageVersionHistory",
        ),
    ] {
        let answer = fixture
            .emulator
            .call_as(principal.access_key_id(), "GET", &uri, None)
            .await;
        assert_refused(
            &answer,
            case,
            &principal.arn(),
            &format!("es:{operation}"),
            &resource_of(operation),
            Refusal::DefaultDeny,
        );
    }

    // With the action allowed, the SAME requests reach the lookup and are answered on
    // what the account holds: the job it holds is reported, the one it does not is the
    // modelled not-found.
    given_policy(
        &fixture.emulator,
        principal,
        Some(&allow(json!("es:*"), json!("*"))),
    );
    let absent = fixture
        .emulator
        .call_as(
            principal.access_key_id(),
            "GET",
            &format!("{PREFIX}/opensearch/app-migrations/{unknown_migration}"),
            None,
        )
        .await;
    assert_eq!(
        (absent.status, error_code(&absent).as_str()),
        (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case an authorized read of a migration this account does not hold: expected the \
         modelled not-found, got {} with {:?}",
        absent.status,
        absent.body
    );

    let read = fixture
        .emulator
        .call_as(
            principal.access_key_id(),
            "GET",
            &format!("{PREFIX}/opensearch/app-migrations/{migration_id}"),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case an authorized read of the started job: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        string_member(&read.body, "applicationId", "the started job"),
        fixture.application_id,
        "case an authorized read of the started job: expected the job of this application, got \
         {:?}",
        read.body
    );

    // The package read of an id the account holds nothing under reaches its own refusal
    // too, which is this operation's client-input error rather than the 403.
    let unknown_package = fixture
        .emulator
        .call_as(
            principal.access_key_id(),
            "GET",
            &format!("{PREFIX}/packages/{UNKNOWN_PACKAGE_ID}/history"),
            None,
        )
        .await;
    assert_eq!(
        unknown_package.status, 400,
        "case an authorized read of a package this account does not hold: expected the \
         operation's own refusal, got {} with {:?}",
        unknown_package.status,
        unknown_package.body
    );

    // And the package the account holds is served to that caller.
    let history = surface_call(
        &fixture.emulator,
        principal.access_key_id(),
        "GetPackageVersionHistory",
        &fixture.package_id,
        &fixture.application_id,
    )
    .await;
    assert_eq!(
        history.status, 200,
        "case an authorized read of the held package: expected 200, got {} with {:?}",
        history.status, history.body
    );
}
