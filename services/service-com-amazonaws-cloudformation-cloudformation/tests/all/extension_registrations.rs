// SPDX-License-Identifier: CC0-1.0

//! `RegisterType` end to end over the awsQuery wire: the private extension version a
//! registration adds to this account and Region's registry, and the request it is monitored
//! by.
//!
//! The rule every case here holds the service to: **`RegisterType` registers ONE version of
//! the extension the request names onto this account and Region's registry — creating the
//! `PRIVATE` record when the Region holds none, making the FIRST version registered that
//! record's default version, and taking the version's schema, its description and (for a
//! resource type) its provisioning behaviour out of the ZIP schema handler package the
//! request addresses in Amazon S3 — and answers the registration token the request is
//! monitored by, which `DescribeTypeRegistration` reports as `IN_PROGRESS` until the clock
//! makes the conclusion due and as `COMPLETE`, with the extension's and the version's ARNs,
//! after; registering "includes validating the extension schema", so a package the
//! registration can read no schema of the extension out of — an object Amazon S3 will not
//! hand over, an object that is not a ZIP archive, an archive carrying no schema of this
//! extension — registers nothing and its request concludes `FAILED` instead; a name in one of
//! the organization namespaces AWS reserves, and text that addresses no Amazon S3 object at
//! all, are each refused with the modelled `CFNRegistryException` (400) and start no
//! registration; and a resubmission under one `ClientRequestToken` is that same registration
//! rather than a second version.**
//!
//! The rows differ along what the rule generalises over: the extension KINDS the model admits
//! (`RESOURCE`, `MODULE`, `HOOK`), which handlers the package's schema declares (every
//! handler, no `update`, no `delete`), each organization namespace AWS reserves and the names
//! that merely resemble one, each Amazon S3 addressing style a `SchemaHandlerPackage` may
//! spell, how many versions of one extension have been registered, and what about a package
//! or the text addressing it cannot be used.
//!
//! Every call goes through the real request path — the CloudFormation calls as form-encoded
//! bodies answered with the XML envelope, and the package staged with a client's own Amazon
//! S3 `PutObject` through the same router — so the schema a registration records is one a
//! client really uploaded, which is what `cfn submit` does before it registers.

use std::io::{Cursor, Write};
use std::time::SystemTime;

use serde_json::json;

use crate::harness::{
    call, member, refusal, router_with_engine, store, xml_blocks, xml_text, xml_texts, REGION,
};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    deprecated_status, provisioning_type, registration_status, registry_type, type_arn,
    type_version_arn, visibility, StoredExtension, StoredTypeVersion, MAX_REGISTERED_VERSIONS,
    REGISTRATION_SPAN, RESERVED_NAMESPACES,
};
use smithy_server_runtime::MultiServiceRouter;

/// Wait past the instant a registration's conclusion becomes due, which is the only wait any
/// case here makes: the conclusion is owed at an instant, and nothing in the service sleeps
/// for it.
async fn wait_for_the_conclusion() {
    tokio::time::sleep(REGISTRATION_SPAN + std::time::Duration::from_millis(250)).await;
}

/// The bucket a client stages its handler packages in.
const PACKAGE_BUCKET: &str = "cfn-extension-packages";

/// The error code the registry's own modelled refusal carries on the wire
/// (`CFNRegistryException`'s `awsQueryError`).
const REGISTRY_REFUSAL_CODE: &str = "CFNRegistryException";

/// The error code the registry answers a request naming an extension it does not hold
/// (`TypeNotFoundException`'s `awsQueryError`), which is what "nothing was registered" reads
/// as.
const NOT_FOUND_CODE: &str = "TypeNotFoundException";

/// The execution role a registration names, in the form `ExecutionRoleArn`'s own pattern
/// states (`^arn:.+:iam::[0-9]{12}:role/.+$`).
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/cfn-extension-handler";

/// The logging configuration a registration names, which `DescribeType` reports back for a
/// private extension.
const LOG_ROLE: &str = "arn:aws:iam::000000000000:role/cfn-extension-logging";
const LOG_GROUP: &str = "cfn-extension-handler-logs";

/// The `TypeVersionId` of the first version of a record, in the zero-padded form the registry
/// gives its version ids.
const FIRST_VERSION: &str = "00000001";

/// The handlers a package's schema may declare, named as the provider definition
/// meta-schema names them (`create`, `read`, `update`, `delete`, `list`).
///
/// Source: `smithy_query(query="doc", shape="DescribeTypeOutput", member="ProvisioningType")`,
/// which states the provisioning behaviour each set gives a resource type.
const EVERY_HANDLER: &[&str] = &["create", "read", "update", "delete", "list"];
const NO_UPDATE_HANDLER: &[&str] = &["create", "read", "delete"];
const NO_DELETE_HANDLER: &[&str] = &["create", "read", "update"];

/// One provider definition schema: the document that declares `type_name`, the handlers
/// named, and a description no request member carries — so an answer reporting it can only
/// have come from the package that was read.
fn schema_of(type_name: &str, handlers: &[&str]) -> String {
    let declared: serde_json::Map<String, serde_json::Value> = handlers
        .iter()
        .map(|handler| {
            (
                (*handler).to_string(),
                json!({"permissions": [format!("cloudformation:{handler}Extension")]}),
            )
        })
        .collect();
    json!({
        "typeName": type_name,
        "description": description_of(type_name),
        "properties": {"Name": {"type": "string"}},
        "handlers": declared,
    })
    .to_string()
}

/// The description the package's schema carries for `type_name`.
fn description_of(type_name: &str) -> String {
    format!("{type_name}, defined by the package a client staged")
}

/// A handler package as a client builds one: a ZIP archive carrying the schema document
/// beside the files that are not it, so the registration has to FIND the schema rather than
/// read the only entry.
///
/// The archive format is the `zip` crate's, here and in the handler that reads it back.
fn package_of(schema: Option<&str>) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buffer);
    for (name, content) in [
        (".rpdk-config", "{\"artifact_type\": \"RESOURCE\"}".to_string()),
        ("README.md", "# an extension project".to_string()),
        (
            "schema.json",
            schema.unwrap_or("{\"notATypeName\": true}").to_string(),
        ),
    ] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .expect("an archive entry starts");
        writer
            .write_all(content.as_bytes())
            .expect("an archive entry is written");
    }
    writer.finish().expect("the archive closes");
    buffer.into_inner()
}

/// An archive carrying no entries at all, which is still a readable ZIP archive.
fn package_of_nothing() -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    zip::ZipWriter::new(&mut buffer)
        .finish()
        .expect("the archive closes");
    buffer.into_inner()
}

/// Create the package bucket, once per router.
async fn create_package_bucket(router: &MultiServiceRouter) {
    let created = s3(router, "PUT", &format!("/{PACKAGE_BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the package bucket must be created first, got {} {}",
        created.status, created.body
    );
}

/// Stage one package object, as a client's own `PutObject` stores it.
async fn stage(router: &MultiServiceRouter, key: &str, body: &[u8]) {
    let stored = s3(router, "PUT", &format!("/{PACKAGE_BUCKET}/{key}"), body).await;
    assert_eq!(
        stored.status, 200,
        "the package {key} must be staged first, got {} {}",
        stored.status, stored.body
    );
}

/// The addressing styles a `SchemaHandlerPackage` may spell, each naming the SAME object.
///
/// Source: Amazon S3's documented request styles — virtual-hosted and path-style — beside the
/// `s3://<bucket>/<key>` URI form the member's own `S3Url` target admits.
#[derive(Clone, Copy, Debug)]
enum Style {
    S3Uri,
    PathAddressed,
    VirtualHosted,
}

impl Style {
    /// The URL this style spells for one object of the package bucket.
    fn url(self, key: &str) -> String {
        match self {
            Style::S3Uri => format!("s3://{PACKAGE_BUCKET}/{key}"),
            Style::PathAddressed => {
                format!("https://s3.{REGION}.amazonaws.com/{PACKAGE_BUCKET}/{key}")
            }
            Style::VirtualHosted => {
                format!("https://{PACKAGE_BUCKET}.s3.{REGION}.amazonaws.com/{key}")
            }
        }
    }
}

/// One `RegisterType` call, as an SDK sends it.
async fn register(
    router: &MultiServiceRouter,
    type_kind: Option<&str>,
    type_name: &str,
    package_url: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("TypeName", type_name),
        member("SchemaHandlerPackage", package_url),
    ];
    if let Some(type_kind) = type_kind {
        members.push(member("Type", type_kind));
    }
    members.extend_from_slice(extra);
    call(router, "RegisterType", &members).await
}

/// The members a registration that names an execution role and a logging configuration
/// carries.
fn with_handler_configuration() -> Vec<(String, String)> {
    vec![
        member("ExecutionRoleArn", EXECUTION_ROLE),
        member("LoggingConfig.LogRoleArn", LOG_ROLE),
        member("LoggingConfig.LogGroupName", LOG_GROUP),
    ]
}

/// `RegisterType` for a case that expects it to SUCCEED: the registration token it answered.
async fn register_ok(
    case: &str,
    router: &MultiServiceRouter,
    type_kind: Option<&str>,
    type_name: &str,
    package_url: &str,
    extra: &[(String, String)],
) -> String {
    let (status, body) = register(router, type_kind, type_name, package_url, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the registration to succeed, got {status} {body}"
    );
    xml_text(&body, "RegistrationToken").unwrap_or_else(|| {
        panic!("case {case:?}: expected a registration token, got {body}")
    })
}

/// What one `DescribeTypeRegistration` read reports about a registration request.
struct Monitored {
    progress: String,
    /// `Description`, "the description of the extension registration request".
    description: Option<String>,
    type_arn: Option<String>,
    type_version_arn: Option<String>,
}

/// One `DescribeTypeRegistration` read of a request the service issued a token for.
async fn describe_registration(
    case: &str,
    router: &MultiServiceRouter,
    token: &str,
) -> Monitored {
    let (status, body) = call(
        router,
        "DescribeTypeRegistration",
        &[member("RegistrationToken", token)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the registration read to succeed, got {status} {body}"
    );
    Monitored {
        progress: xml_text(&body, "ProgressStatus").unwrap_or_default(),
        description: xml_text(&body, "Description"),
        type_arn: xml_text(&body, "TypeArn"),
        type_version_arn: xml_text(&body, "TypeVersionArn"),
    }
}

/// One `DescribeType` read of a registered extension, by kind and name, optionally at one
/// version: the HTTP status and the XML body.
async fn describe_type(
    router: &MultiServiceRouter,
    type_kind: &str,
    type_name: &str,
    version_id: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("Type", type_kind), member("TypeName", type_name)];
    if let Some(version_id) = version_id {
        members.push(member("VersionId", version_id));
    }
    call(router, "DescribeType", &members).await
}

/// The registration tokens `ListTypeRegistrations` reports for one extension, at the status
/// it filters on.
async fn registration_tokens(
    router: &MultiServiceRouter,
    type_kind: &str,
    type_name: &str,
    status_filter: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListTypeRegistrations",
        &[
            member("Type", type_kind),
            member("TypeName", type_name),
            member("RegistrationStatusFilter", status_filter),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the registration listing to succeed, got {status} {body}"
    );
    match xml_blocks(&body, "RegistrationTokenList").into_iter().next() {
        Some(block) => xml_texts(&block, "member"),
        None => Vec::new(),
    }
}

/// One row of the registration table: the extension a package declares, and what the registry
/// must then report for the version the registration added.
struct Registered {
    case: &'static str,
    /// The kind the request names, or `None` for a request that names none at all.
    type_kind: Option<&'static str>,
    /// The kind the registry must report, which is the resource type a request naming no kind
    /// registers.
    reported_kind: &'static str,
    type_name: &'static str,
    /// The handlers the package's schema declares, which is what the provisioning behaviour
    /// is read from.
    handlers: &'static [&'static str],
    /// The provisioning behaviour the registry must report, where it reports one: "for
    /// resource type extensions, the provisioning behavior of the resource type".
    provisioning: Option<&'static str>,
    /// The style the request addresses the staged package in.
    style: Style,
}

#[tokio::test]
async fn a_registration_adds_the_version_the_handler_package_declares() {
    let (router, _engine) = router_with_engine();
    create_package_bucket(&router).await;

    let rows: &[Registered] = &[
        Registered {
            case: "a resource type whose schema declares every handler",
            type_kind: Some(registry_type::RESOURCE),
            reported_kind: registry_type::RESOURCE,
            type_name: "My::Sample::Resource",
            handlers: EVERY_HANDLER,
            provisioning: Some(provisioning_type::FULLY_MUTABLE),
            style: Style::S3Uri,
        },
        Registered {
            case: "a resource type whose schema declares no update handler",
            type_kind: Some(registry_type::RESOURCE),
            reported_kind: registry_type::RESOURCE,
            type_name: "My::Sample::Immutable",
            handlers: NO_UPDATE_HANDLER,
            provisioning: Some(provisioning_type::IMMUTABLE),
            style: Style::PathAddressed,
        },
        Registered {
            case: "a resource type whose schema declares no delete handler",
            type_kind: Some(registry_type::RESOURCE),
            reported_kind: registry_type::RESOURCE,
            type_name: "My::Sample::Undeletable",
            handlers: NO_DELETE_HANDLER,
            provisioning: Some(provisioning_type::NON_PROVISIONABLE),
            style: Style::VirtualHosted,
        },
        Registered {
            case: "a request that names no kind registers a resource type",
            type_kind: None,
            reported_kind: registry_type::RESOURCE,
            type_name: "My::Sample::Default",
            handlers: EVERY_HANDLER,
            provisioning: Some(provisioning_type::FULLY_MUTABLE),
            style: Style::PathAddressed,
        },
        Registered {
            case: "a module, which has no provisioning behaviour of its own",
            type_kind: Some(registry_type::MODULE),
            reported_kind: registry_type::MODULE,
            type_name: "My::Sample::Thing::MODULE",
            handlers: EVERY_HANDLER,
            provisioning: None,
            style: Style::S3Uri,
        },
        Registered {
            case: "a Hook, which has no provisioning behaviour of its own",
            type_kind: Some(registry_type::HOOK),
            reported_kind: registry_type::HOOK,
            type_name: "My::Sample::Hook",
            handlers: NO_UPDATE_HANDLER,
            provisioning: None,
            style: Style::VirtualHosted,
        },
    ];

    // Every row's package is staged and registered first, and each registration is read back
    // while it is still running: the conclusion is due at an instant, so the request has to
    // be observable before that instant as well as after it.
    let mut tokens: Vec<String> = Vec::new();
    for row in rows {
        let schema = schema_of(row.type_name, row.handlers);
        let key = format!("{}.zip", row.type_name.replace("::", "-"));
        stage(&router, &key, &package_of(Some(&schema))).await;
        let token = register_ok(
            row.case,
            &router,
            row.type_kind,
            row.type_name,
            &row.style.url(&key),
            &with_handler_configuration(),
        )
        .await;
        let monitored = describe_registration(row.case, &router, &token).await;
        assert_eq!(
            monitored.progress,
            registration_status::IN_PROGRESS,
            "case {:?}: a registration whose conclusion is not due is still running, got {:?}",
            row.case, monitored.progress
        );
        assert_eq!(
            (
                monitored.type_arn.as_deref(),
                monitored.type_version_arn.as_deref()
            ),
            (None, None),
            "case {:?}: a registration other than COMPLETE reports neither ARN, got {:?} and \
             {:?}",
            row.case, monitored.type_arn, monitored.type_version_arn
        );
        // The request is the one `ListTypeRegistrations` serves from, under the extension it
        // is a registration of.
        let listed = registration_tokens(
            &router,
            row.reported_kind,
            row.type_name,
            registration_status::IN_PROGRESS,
        )
        .await;
        assert_eq!(
            listed,
            vec![token.clone()],
            "case {:?}: expected the listing to report the request still running, got \
             {listed:?}",
            row.case
        );
        tokens.push(token);
    }

    // Past the instant each conclusion became due.
    wait_for_the_conclusion().await;

    for (row, token) in rows.iter().zip(tokens.iter()) {
        let expected_arn = type_arn(REGION, row.reported_kind, row.type_name);
        let expected_version_arn = type_version_arn(&expected_arn, FIRST_VERSION);
        let monitored = describe_registration(row.case, &router, token).await;
        assert_eq!(
            monitored.progress,
            registration_status::COMPLETE,
            "case {:?}: a conclusion the clock made due is reported, got {:?}",
            row.case, monitored.progress
        );
        assert_eq!(
            monitored.type_arn.as_deref(),
            Some(expected_arn.as_str()),
            "case {:?}: expected the extension ARN {expected_arn:?}, got {:?}",
            row.case, monitored.type_arn
        );
        assert_eq!(
            monitored.type_version_arn.as_deref(),
            Some(expected_version_arn.as_str()),
            "case {:?}: expected the version ARN {expected_version_arn:?}, got {:?}",
            row.case, monitored.type_version_arn
        );

        // The version itself, read back through `DescribeType`: what the registration wrote
        // is what a client reads.
        let (status, body) = describe_type(&router, row.reported_kind, row.type_name, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the registered extension to be readable, got {status} {body}",
            row.case
        );
        let expected_schema = schema_of(row.type_name, row.handlers);
        for (member_name, expected) in [
            ("Arn", Some(expected_version_arn.clone())),
            ("Type", Some(row.reported_kind.to_string())),
            ("TypeName", Some(row.type_name.to_string())),
            ("Visibility", Some(visibility::PRIVATE.to_string())),
            ("DeprecatedStatus", Some(deprecated_status::LIVE.to_string())),
            ("DefaultVersionId", Some(FIRST_VERSION.to_string())),
            ("IsDefaultVersion", Some("true".to_string())),
            ("ExecutionRoleArn", Some(EXECUTION_ROLE.to_string())),
            ("LogRoleArn", Some(LOG_ROLE.to_string())),
            ("LogGroupName", Some(LOG_GROUP.to_string())),
            ("ProvisioningType", row.provisioning.map(str::to_string)),
            ("Description", Some(description_of(row.type_name))),
            ("Schema", Some(expected_schema)),
        ] {
            let reported = xml_text(&body, member_name);
            assert_eq!(
                reported, expected,
                "case {:?}: expected {member_name} to be {expected:?}, got {reported:?} in \
                 {body}",
                row.case
            );
        }
        // "When the specified private extension version was registered in your account."
        let created = xml_text(&body, "TimeCreated");
        assert!(
            created.is_some(),
            "case {:?}: expected the registration instant to be reported, got {body}",
            row.case
        );
    }
}

#[tokio::test]
async fn each_registration_of_one_extension_adds_a_version_and_leaves_the_default() {
    let (router, _engine) = router_with_engine();
    create_package_bucket(&router).await;
    let type_name = "My::Versioned::Resource";

    // Three registrations of ONE extension, each from a package declaring a different handler
    // set, so the version a read selects is observable from what it reports.
    let registrations: &[(&str, &'static [&'static str], &str, &str)] = &[
        (
            "the first version registered",
            EVERY_HANDLER,
            "00000001",
            provisioning_type::FULLY_MUTABLE,
        ),
        (
            "the second version registered",
            NO_UPDATE_HANDLER,
            "00000002",
            provisioning_type::IMMUTABLE,
        ),
        (
            "the third version registered",
            NO_DELETE_HANDLER,
            "00000003",
            provisioning_type::NON_PROVISIONABLE,
        ),
    ];

    for (case, handlers, expected_version, expected_provisioning) in registrations {
        let key = format!("versioned-{expected_version}.zip");
        let schema = schema_of(type_name, handlers);
        stage(&router, &key, &package_of(Some(&schema))).await;
        let token = register_ok(
            case,
            &router,
            Some(registry_type::RESOURCE),
            type_name,
            &Style::S3Uri.url(&key),
            &[],
        )
        .await;
        wait_for_the_conclusion().await;
        let expected_arn = type_arn(REGION, registry_type::RESOURCE, type_name);
        let monitored = describe_registration(case, &router, &token).await;
        assert_eq!(
            monitored.progress,
            registration_status::COMPLETE,
            "case {case:?}: expected the registration to have concluded, got {:?}",
            monitored.progress
        );
        assert_eq!(
            monitored.type_version_arn.as_deref(),
            Some(type_version_arn(&expected_arn, expected_version).as_str()),
            "case {case:?}: expected the registration to report version {expected_version}, \
             got {:?}",
            monitored.type_version_arn
        );

        // That version, selected by its own id: its schema is the package it came from.
        let (status, body) = describe_type(
            &router,
            registry_type::RESOURCE,
            type_name,
            Some(expected_version),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected version {expected_version} to be readable, got {status} \
             {body}"
        );
        for (member_name, expected) in [
            ("Schema", schema.clone()),
            ("ProvisioningType", (*expected_provisioning).to_string()),
            // "The first version registered becomes the default version": a later
            // registration leaves it where it is, since moving it is `SetTypeDefaultVersion`'s
            // job.
            ("DefaultVersionId", FIRST_VERSION.to_string()),
        ] {
            let reported = xml_text(&body, member_name);
            assert_eq!(
                reported.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {member_name} to be {expected:?}, got {reported:?} \
                 in {body}"
            );
        }
    }

    // A request naming no version still resolves the default one, which is the first.
    let (status, body) = describe_type(&router, registry_type::RESOURCE, type_name, None).await;
    assert_eq!(
        status, 200,
        "case \"the default version\": expected the extension to be readable, got {status} \
         {body}"
    );
    let reported = xml_text(&body, "Arn");
    let expected = type_version_arn(
        &type_arn(REGION, registry_type::RESOURCE, type_name),
        FIRST_VERSION,
    );
    assert_eq!(
        reported.as_deref(),
        Some(expected.as_str()),
        "case \"the default version\": expected {expected:?}, got {reported:?} in {body}"
    );
    // Every registration of the extension is a request of its own, in the order they were
    // started.
    let listed = registration_tokens(
        &router,
        registry_type::RESOURCE,
        type_name,
        registration_status::COMPLETE,
    )
    .await;
    assert_eq!(
        listed.len(),
        registrations.len(),
        "case \"one request per registration\": expected {} requests, got {listed:?}",
        registrations.len()
    );
}

/// One `ListTypeVersions` read: the HTTP status, the body, and the `(VersionId,
/// IsDefaultVersion, Arn)` triple each summary reports, in wire order.
async fn list_type_versions(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String, Vec<(String, String, String)>) {
    let (status, body) = call(router, "ListTypeVersions", members).await;
    // The summaries are read INSIDE their own block, so an element of another list the
    // envelope carries cannot be swept up with them.
    let summaries = match xml_blocks(&body, "TypeVersionSummaries").into_iter().next() {
        Some(block) => xml_blocks(&block, "member")
            .into_iter()
            .map(|summary| {
                (
                    xml_text(&summary, "VersionId").unwrap_or_default(),
                    xml_text(&summary, "IsDefaultVersion").unwrap_or_default(),
                    xml_text(&summary, "Arn").unwrap_or_default(),
                )
            })
            .collect(),
        None => Vec::new(),
    };
    (status, body, summaries)
}

/// `ListTypeVersions` reports every version the identified extension holds — each with its
/// own id and ARN, and the default one marked — narrowed by the deprecation status the
/// request asks for and paged as the model's `@paginated` trait states.
///
/// The rows differ along what the rule generalises over: how the request identifies the
/// extension (its kind and name, its own ARN, one of its VERSION ARNs), how many versions
/// the extension holds, which of them is the default, the deprecation status asked for, and
/// the page size.
#[tokio::test]
async fn every_version_an_extension_holds_is_listed() {
    let (router, _engine) = router_with_engine();
    create_package_bucket(&router).await;
    let type_name = "My::Listed::Resource";
    let extension_arn = type_arn(REGION, registry_type::RESOURCE, type_name);

    // Three registrations of ONE extension: the versions the listing must report, in the
    // order they were added.
    let versions: &[&str] = &["00000001", "00000002", "00000003"];
    for version in versions {
        let key = format!("listed-{version}.zip");
        stage(
            &router,
            &key,
            &package_of(Some(&schema_of(type_name, EVERY_HANDLER))),
        )
        .await;
        register_ok(
            "the versions of one extension",
            &router,
            Some(registry_type::RESOURCE),
            type_name,
            &Style::S3Uri.url(&key),
            &[],
        )
        .await;
        wait_for_the_conclusion().await;
    }

    // Each of the three ways a request may identify that extension names the same versions.
    let identifiers: &[(&str, Vec<(String, String)>)] = &[
        (
            "by kind and name",
            vec![
                member("Type", registry_type::RESOURCE),
                member("TypeName", type_name),
            ],
        ),
        (
            "by the extension's own ARN",
            vec![member("Arn", &extension_arn)],
        ),
        (
            // "The version ID is the value at the end of the ARN assigned to the extension
            // version": such an ARN names the extension it is a version OF, and the listing
            // is of that extension's versions.
            "by one of its version ARNs",
            vec![member("Arn", &type_version_arn(&extension_arn, "00000002"))],
        ),
    ];
    let expected: Vec<(String, String, String)> = versions
        .iter()
        .map(|version| {
            (
                (*version).to_string(),
                // "The first version registered becomes the default version", and a later
                // registration leaves it there.
                (*version == FIRST_VERSION).to_string(),
                type_version_arn(&extension_arn, version),
            )
        })
        .collect();
    for (case, members) in identifiers {
        let (status, body, listed) = list_type_versions(&router, members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the version listing to succeed, got {status} {body}"
        );
        assert_eq!(
            listed, expected,
            "case {case:?}: expected every version the extension holds, got {listed:?} in \
             {body}"
        );
    }

    // `DeprecatedStatus` is "the deprecation status of the extension versions that you want
    // to get summary information about … The default is LIVE": the registered versions are
    // LIVE, and asking for the DEPRECATED ones reports none of them.
    let by_status: &[(&str, &str, usize)] = &[
        ("the status the default admits", deprecated_status::LIVE, 3),
        (
            "the status no registered version is in",
            deprecated_status::DEPRECATED,
            0,
        ),
    ];
    for (case, status_filter, expected_count) in by_status {
        let (status, body, listed) = list_type_versions(
            &router,
            &[
                member("Type", registry_type::RESOURCE),
                member("TypeName", type_name),
                member("DeprecatedStatus", status_filter),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the version listing to succeed, got {status} {body}"
        );
        assert_eq!(
            listed.len(),
            *expected_count,
            "case {case:?}: expected {expected_count} summaries, got {listed:?} in {body}"
        );
    }

    // `@paginated(pageSize: MaxResults)`: a page carries at most that many summaries and the
    // token that resumes just past them, and the pages of one collection partition it.
    for page_size in [1usize, 2, 3] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<(String, String, String)> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut members = vec![
                member("Type", registry_type::RESOURCE),
                member("TypeName", type_name),
                member("MaxResults", &page_size.to_string()),
            ];
            if let Some(token) = token.as_deref() {
                members.push(member("NextToken", token));
            }
            let (status, body, listed) = list_type_versions(&router, &members).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the page to be served, got {status} {body}"
            );
            assert!(
                listed.len() <= page_size,
                "case {case:?}: a page must carry at most {page_size} summaries, got \
                 {listed:?}"
            );
            seen.extend(listed);
            token = xml_text(&body, "NextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages of one collection must partition it, got {seen:?}"
        );
    }

    // And the refusals: an extension the registry does not hold, and a request that
    // identifies no single extension.
    let refusals: &[(&str, Vec<(String, String)>)] = &[
        (
            "an extension nothing registered",
            vec![
                member("Type", registry_type::RESOURCE),
                member("TypeName", "My::Absent::Resource"),
            ],
        ),
        (
            "half of the name form",
            vec![member("TypeName", type_name)],
        ),
        (
            "both identifying forms at once",
            vec![
                member("Arn", &extension_arn),
                member("Type", registry_type::RESOURCE),
                member("TypeName", type_name),
            ],
        ),
    ];
    for (case, members) in refusals {
        let (status, body, listed) = list_type_versions(&router, members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the listing to be refused, got {status} {body}"
        );
        assert!(
            listed.is_empty(),
            "case {case:?}: a refusal reports no summaries, got {listed:?}"
        );
        assert_eq!(
            refusal(&body).0,
            REGISTRY_REFUSAL_CODE,
            "case {case:?}: expected the registry's own refusal code, got {body}"
        );
    }
}

#[tokio::test]
async fn a_resubmission_under_one_client_request_token_registers_one_version() {
    let (router, _engine) = router_with_engine();
    create_package_bucket(&router).await;
    let type_name = "My::Idempotent::Resource";
    let schema = schema_of(type_name, EVERY_HANDLER);
    stage(&router, "idempotent.zip", &package_of(Some(&schema))).await;

    let idempotency_key = "registration-submitted-twice";
    let submitted = [member("ClientRequestToken", idempotency_key)];
    let first = register_ok(
        "the first submission",
        &router,
        Some(registry_type::RESOURCE),
        type_name,
        &Style::S3Uri.url("idempotent.zip"),
        &submitted,
    )
    .await;
    let again = register_ok(
        "the same request submitted again",
        &router,
        Some(registry_type::RESOURCE),
        type_name,
        &Style::S3Uri.url("idempotent.zip"),
        &submitted,
    )
    .await;
    assert_eq!(
        again, first,
        "case \"the same request submitted again\": expected the registration token \
         {first:?}, got {again:?}"
    );
    wait_for_the_conclusion().await;

    // One version, so the second version id was never handed out.
    let (status, body) = describe_type(
        &router,
        registry_type::RESOURCE,
        type_name,
        Some("00000002"),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"the same request submitted again\": expected no second version, got {status} \
         {body}"
    );
    let listed = registration_tokens(
        &router,
        registry_type::RESOURCE,
        type_name,
        registration_status::COMPLETE,
    )
    .await;
    assert_eq!(
        listed,
        vec![first.clone()],
        "case \"the same request submitted again\": expected the one request {first:?}, got \
         {listed:?}"
    );

    // A DIFFERENT token is a different registration request, which does add a version.
    let other = register_ok(
        "a second registration under another token",
        &router,
        Some(registry_type::RESOURCE),
        type_name,
        &Style::S3Uri.url("idempotent.zip"),
        &[member("ClientRequestToken", "registration-submitted-once")],
    )
    .await;
    assert_ne!(
        other, first,
        "case \"a second registration under another token\": expected a registration of its \
         own, got {other:?}"
    );
    wait_for_the_conclusion().await;
    let (status, body) = describe_type(
        &router,
        registry_type::RESOURCE,
        type_name,
        Some("00000002"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a second registration under another token\": expected the second version to \
         be readable, got {status} {body}"
    );
}

#[tokio::test]
async fn a_package_the_registration_cannot_use_registers_nothing() {
    let (router, _engine) = router_with_engine();
    create_package_bucket(&router).await;
    // Readable archives that carry no schema of the extension being registered: one with no
    // provider definition document at all, one whose document declares ANOTHER extension, and
    // one carrying no entries whatever — beside objects that are no archive at all and keys
    // Amazon S3 will not hand over.
    stage(&router, "schemaless.zip", &package_of(None)).await;
    stage(
        &router,
        "another.zip",
        &package_of(Some(&schema_of("Some::Other::Resource", EVERY_HANDLER))),
    )
    .await;
    stage(&router, "empty.zip", &package_of_nothing()).await;
    stage(&router, "plain.txt", b"this is not an archive").await;
    let truncated = package_of(Some(&schema_of("My::Sample::Resource", EVERY_HANDLER)));
    stage(&router, "truncated.zip", &truncated[..truncated.len() / 2]).await;

    let rows: &[(&str, &str, &str, &str)] = &[
        (
            "an archive carrying no provider definition document",
            "My::Failing::Schemaless",
            "s3://cfn-extension-packages/schemaless.zip",
            "no schema defining My::Failing::Schemaless",
        ),
        (
            "an archive whose schema declares another extension",
            "My::Failing::Mismatched",
            "s3://cfn-extension-packages/another.zip",
            "no schema defining My::Failing::Mismatched",
        ),
        (
            "an archive carrying no entries at all",
            "My::Failing::Empty",
            "s3://cfn-extension-packages/empty.zip",
            "no schema defining My::Failing::Empty",
        ),
        (
            "an object that is not a zip archive",
            "My::Failing::Plain",
            "s3://cfn-extension-packages/plain.txt",
            "not a valid ZIP file",
        ),
        (
            "an archive whose bytes stop part way through",
            "My::Failing::Truncated",
            "s3://cfn-extension-packages/truncated.zip",
            "not a valid ZIP file",
        ),
        (
            "a key the bucket does not hold",
            "My::Failing::Missing",
            "s3://cfn-extension-packages/nothing-staged-here.zip",
            "nothing-staged-here.zip",
        ),
        (
            // A bucket that is not there establishes nothing about the object, so what the
            // registration reports is what Amazon S3 answered the read with — the one
            // wording every read of a caller-addressed object carries
            // (`state::template_url`).
            "a bucket Amazon S3 does not hold",
            "My::Failing::Bucketless",
            "s3://no-such-package-bucket/package.zip",
            "The specified bucket does not exist",
        ),
    ];

    let mut tokens: Vec<String> = Vec::new();
    for (case, type_name, package, _) in rows {
        // The REQUEST is accepted — a registration is asynchronous, and validating the
        // package is part of registering rather than of accepting the registration.
        let token = register_ok(
            case,
            &router,
            Some(registry_type::RESOURCE),
            type_name,
            package,
            &[],
        )
        .await;
        let monitored = describe_registration(case, &router, &token).await;
        assert_eq!(
            monitored.progress,
            registration_status::IN_PROGRESS,
            "case {case:?}: a registration whose conclusion is not due is still running, got \
             {:?}",
            monitored.progress
        );
        tokens.push(token);
    }

    wait_for_the_conclusion().await;

    for ((case, type_name, _, names_the_cause), token) in rows.iter().zip(tokens.iter()) {
        let monitored = describe_registration(case, &router, token).await;
        assert_eq!(
            monitored.progress,
            registration_status::FAILED,
            "case {case:?}: expected the registration to have failed, got {:?}",
            monitored.progress
        );
        // "The description of the extension registration request": a request that failed is
        // described by WHY it could not register the version.
        let description = monitored.description.clone().unwrap_or_default();
        assert!(
            description.contains(names_the_cause),
            "case {case:?}: expected the description to name {names_the_cause:?}, got \
             {description:?}"
        );
        assert_eq!(
            (
                monitored.type_arn.as_deref(),
                monitored.type_version_arn.as_deref()
            ),
            (None, None),
            "case {case:?}: a registration other than COMPLETE reports neither ARN, got {:?} \
             and {:?}",
            monitored.type_arn,
            monitored.type_version_arn
        );
        // Nothing was registered: the extension is not in this Region's registry at all.
        let (status, body) = describe_type(&router, registry_type::RESOURCE, type_name, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected no record for a failed registration, got {status} {body}"
        );
        let (code, _) = refusal(&body);
        assert_eq!(
            code, NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {code:?} in {body}"
        );
        // The failed request is still a record of its own, under the status it concluded in.
        let listed = registration_tokens(
            &router,
            registry_type::RESOURCE,
            type_name,
            registration_status::FAILED,
        )
        .await;
        assert_eq!(
            listed,
            vec![token.clone()],
            "case {case:?}: expected the listing to report the failed request, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_reserved_organization_namespace_registers_nothing() {
    let (router, _engine) = router_with_engine();
    create_package_bucket(&router).await;
    stage(
        &router,
        "reserved.zip",
        &package_of(Some(&schema_of("Any::Sample::Resource", EVERY_HANDLER))),
    )
    .await;
    let package = Style::S3Uri.url("reserved.zip");

    // Every namespace AWS reserves: "the following organization namespaces are reserved and
    // can't be used in your extension names" (`RegisterTypeInput$TypeName`), read from the
    // catalogue the registry holds them in rather than spelled again here.
    for reserved in RESERVED_NAMESPACES {
        let type_name = format!("{reserved}::Sample::Resource");
        let (status, body) = register(
            &router,
            Some(registry_type::RESOURCE),
            &type_name,
            &package,
            &[],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {type_name:?}: expected the registration to be refused, got {status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code, REGISTRY_REFUSAL_CODE,
            "case {type_name:?}: expected {REGISTRY_REFUSAL_CODE}, got {code:?} in {body}"
        );
        let expected = format!(
            "Type name {type_name} is using a restricted namespace {}",
            reserved.to_lowercase()
        );
        assert_eq!(
            message, expected,
            "case {type_name:?}: expected the refusal {expected:?}, got {message:?}"
        );
        // Nothing was registered: the name resolves no extension at all.
        let (status, body) = describe_type(&router, registry_type::RESOURCE, &type_name, None).await;
        assert_eq!(
            status, 404,
            "case {type_name:?}: expected no record for a refused registration, got {status} \
             {body}"
        );
        let (code, _) = refusal(&body);
        assert_eq!(
            code, NOT_FOUND_CODE,
            "case {type_name:?}: expected {NOT_FOUND_CODE}, got {code:?} in {body}"
        );
    }

    // A namespace that merely RESEMBLES a reserved one is another organization's: only the
    // whole first segment is reserved.
    for type_name in ["AWSome::Sample::Resource", "Amazonia::Sample::Resource"] {
        let (status, body) = register(
            &router,
            Some(registry_type::RESOURCE),
            type_name,
            &package,
            &[],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {type_name:?}: expected the registration to succeed, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn text_that_addresses_no_package_is_refused() {
    let (router, _engine) = router_with_engine();
    // A `SchemaHandlerPackage` that addresses no Amazon S3 object at all names no package, so
    // the REQUEST is refused rather than a registration started for one. The rows differ along
    // what about the text addresses nothing: no scheme and no path, a scheme that names no
    // object, an http(s) URL carrying a bucket but no key, and an empty object name.
    let rows: &[(&str, &str, &str)] = &[
        (
            "text that is no URL at all",
            "My::Unaddressed::Text",
            "not-a-url-at-all",
        ),
        (
            "a scheme that addresses no Amazon S3 object",
            "My::Unaddressed::Scheme",
            "file:///tmp/package.zip",
        ),
        (
            "a URL carrying a bucket but no key",
            "My::Unaddressed::Keyless",
            "https://s3.us-east-1.amazonaws.com/cfn-extension-packages",
        ),
        (
            "an s3 URI carrying no key",
            "My::Unaddressed::Bare",
            "s3://cfn-extension-packages/",
        ),
    ];

    for (case, type_name, package) in rows {
        let (status, body) = register(
            &router,
            Some(registry_type::RESOURCE),
            type_name,
            package,
            &[],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the registration to be refused, got {status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code, REGISTRY_REFUSAL_CODE,
            "case {case:?}: expected {REGISTRY_REFUSAL_CODE}, got {code:?} in {body}"
        );
        assert!(
            message.contains("SchemaHandlerPackage"),
            "case {case:?}: expected the refusal to name the member, got {message:?}"
        );
        // Nothing was started: the request was refused, so there is no registration of this
        // extension at all.
        let listed = registration_tokens(
            &router,
            registry_type::RESOURCE,
            type_name,
            registration_status::IN_PROGRESS,
        )
        .await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected no registration request, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_registration_past_the_account_version_quota_is_refused() {
    // "You can have a maximum of 50 resource extension versions registered at a time. This
    // maximum is per account and per Region. Use DeregisterType to deregister specific
    // extension versions if necessary" (`RegisterType`): what the quota counts is the
    // versions still REGISTERED, so a deregistered version leaves room for another.
    //
    // The rows differ along how the Region's registry is filled: exactly the quota in live
    // versions, one short of it, and the quota's worth of live versions beside deprecated
    // ones that do not count.
    let rows: &[(&str, usize, usize, u16)] = &[
        ("one version short of the quota", MAX_REGISTERED_VERSIONS - 1, 0, 200),
        ("exactly the quota, in live versions", MAX_REGISTERED_VERSIONS, 0, 400),
        (
            "the quota's worth of live versions beside deprecated ones",
            MAX_REGISTERED_VERSIONS,
            5,
            400,
        ),
        (
            "deprecated versions alone, which are not registered any more",
            0,
            MAX_REGISTERED_VERSIONS,
            200,
        ),
    ];

    for (case, live, deprecated, expected) in rows {
        let (router, engine) = router_with_engine();
        create_package_bucket(&router).await;
        let type_name = "My::Quota::Resource";
        let schema = schema_of(type_name, EVERY_HANDLER);
        stage(&router, "quota.zip", &package_of(Some(&schema))).await;
        // More versions than any case could drive over the wire, so they are seeded straight
        // into the store — the same device `type_registrations.rs` seeds its requests with.
        {
            let state = store(&engine);
            let mut held = state.lock();
            let mut record =
                StoredExtension::new(REGION, registry_type::RESOURCE, "My::Quota::Filler");
            for index in 0..(live + deprecated) {
                let version_id = format!("{:08}", index + 1);
                let mut version =
                    StoredTypeVersion::new(&record.arn, &version_id, SystemTime::now());
                if index >= *live {
                    version.deprecate();
                }
                record.versions.push(version);
            }
            record.set_default_version(FIRST_VERSION);
            held.extensions.push(record);
        }

        let (status, body) = register(
            &router,
            Some(registry_type::RESOURCE),
            type_name,
            &Style::S3Uri.url("quota.zip"),
            &[],
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected != 400 {
            continue;
        }
        let (code, message) = refusal(&body);
        assert_eq!(
            code, REGISTRY_REFUSAL_CODE,
            "case {case:?}: expected {REGISTRY_REFUSAL_CODE}, got {code:?} in {body}"
        );
        assert!(
            message.contains(&MAX_REGISTERED_VERSIONS.to_string()),
            "case {case:?}: expected the refusal to state the quota \
             {MAX_REGISTERED_VERSIONS}, got {message:?}"
        );
        // A refused registration starts nothing: there is no request of the extension it
        // named.
        let listed = registration_tokens(
            &router,
            registry_type::RESOURCE,
            type_name,
            registration_status::IN_PROGRESS,
        )
        .await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected no registration request, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_registration_token_nothing_issued_resolves_no_request() {
    let (router, _engine) = router_with_engine();
    for (case, token) in [
        ("a token nothing issued", "not-a-token-this-service-issued"),
        ("a token of the form this service mints", "0123abcd-4567-89ef-0123-456789abcdef"),
    ] {
        let (status, body) = call(
            &router,
            "DescribeTypeRegistration",
            &[member("RegistrationToken", token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the read to be refused, got {status} {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code, REGISTRY_REFUSAL_CODE,
            "case {case:?}: expected {REGISTRY_REFUSAL_CODE}, got {code:?} in {body}"
        );
        // The registry's own sentence for a token it resolves to no request, written out here
        // rather than read off the implementation's constant. It quotes nothing from the
        // request, so the row's token is not part of it — `monitored_registrations.rs` is the
        // suite that holds that wording across the token forms the model admits.
        assert_eq!(
            message, "UUID is invalid",
            "case {case:?}: expected the registry's sentence for an unresolved token, got \
             {message:?}"
        );
    }
}
