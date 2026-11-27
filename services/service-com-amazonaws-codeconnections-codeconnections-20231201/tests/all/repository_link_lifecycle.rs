// SPDX-License-Identifier: CC0-1.0

//! Repository links over the wire: create, get, list tags, delete.
//!
//! The rule under test: a repository link is a PERSISTED resource addressed by the
//! `RepositoryLinkId` its create minted, so `DeleteRepositoryLink` removes exactly that
//! stored row — after it no reader resolves the id, and every other link is untouched —
//! and the delete is IDEMPOTENT: it states the post-state, so an id this account holds
//! no link for is answered 200 with an empty body and nothing in the store changes.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers (`router`, `call`, `create`), so these tests
//! exercise the same path a signed SDK request takes.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{PROVIDER_TYPES, REGION, call, call_with_target, create, router};

/// One `CreateRepositoryLink` request whose round trip this suite asserts.
struct LinkCase {
    /// What a failing assertion names this row by.
    label: &'static str,
    /// `OwnerId` is `length 1..255`, pattern `^.*$`
    /// (`smithy_query(query="operation", operation="CreateRepositoryLink")`).
    owner_id: String,
    /// `RepositoryName` is `length 1..128`, pattern `^.*$`.
    repository_name: String,
    /// `EncryptionKeyArn` (`KmsKeyArn`, `length 1..1024`) when the create supplies one.
    encryption_key_arn: Option<String>,
    /// Tags as `(Key, Value)`, in the order the create supplies them.
    tags: Vec<(&'static str, &'static str)>,
}

/// A KMS key ARN matching the modeled `KmsKeyArn` pattern
/// `^arn:aws(-[\w]+)*:kms:[a-z\-0-9]+:\d{12}:key/[a-zA-Z0-9\-]+$`.
fn kms_key_arn(key: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:key/{key}",
        engine::emulator_account_id()
    )
}

/// The table: links differing in owner id and repository name form and length (each
/// published bound), whether an encryption key is supplied, and how many tags they
/// carry.
fn link_cases() -> Vec<LinkCase> {
    vec![
        LinkCase {
            label: "no encryption key, no tags",
            owner_id: "octo-org".to_string(),
            repository_name: "service-emulator".to_string(),
            encryption_key_arn: None,
            tags: Vec::new(),
        },
        LinkCase {
            label: "an encryption key and one tag",
            owner_id: "another-org".to_string(),
            repository_name: "infra.config".to_string(),
            encryption_key_arn: Some(kms_key_arn("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0")),
            tags: vec![("team", "platform")],
        },
        LinkCase {
            label: "owner id and repository name at their published minimum length",
            owner_id: "o".to_string(),
            repository_name: "r".to_string(),
            encryption_key_arn: None,
            tags: vec![("only", "one")],
        },
        LinkCase {
            label: "owner id and repository name at their published maximum length",
            owner_id: "o".repeat(255),
            repository_name: "r".repeat(128),
            encryption_key_arn: Some(kms_key_arn("abcdef01-2345-6789-abcd-ef0123456789")),
            tags: vec![("a", "1"), ("b", "2"), ("c", "3")],
        },
        LinkCase {
            label: "a repository name of the nested form a provider admits",
            owner_id: "group/subgroup".to_string(),
            repository_name: "deep/nested/repo".to_string(),
            encryption_key_arn: None,
            tags: vec![("env", "")],
        },
    ]
}

/// Render a `(Key, Value)` table row as the wire `TagList`.
fn tag_list(tags: &[(&str, &str)]) -> serde_json::Value {
    serde_json::Value::Array(
        tags.iter()
            .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The `CreateRepositoryLink` request body for one row against `connection_arn`.
fn create_link_body(case: &LinkCase, connection_arn: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "ConnectionArn": connection_arn,
        "OwnerId": case.owner_id,
        "RepositoryName": case.repository_name,
    });
    if let Some(key) = &case.encryption_key_arn {
        body["EncryptionKeyArn"] = serde_json::Value::String(key.clone());
    }
    if !case.tags.is_empty() {
        body["Tags"] = tag_list(&case.tags);
    }
    body
}

/// Create a repository link and return its reported `RepositoryLinkInfo`, failing the
/// test on any non-200.
async fn create_link(
    router: &MultiServiceRouter,
    case: &LinkCase,
    connection_arn: &str,
) -> serde_json::Value {
    let (status, body) = call(
        router,
        "CreateRepositoryLink",
        create_link_body(case, connection_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200 from CreateRepositoryLink, got {status} {body}",
        case.label
    );
    body["RepositoryLinkInfo"].clone()
}

/// Create a host and return its ARN, failing the test on any non-200.
async fn create_host(router: &MultiServiceRouter, name: &str, provider: &str) -> String {
    let (status, body) = call(
        router,
        "CreateHost",
        serde_json::json!({
            "Name": name,
            "ProviderType": provider,
            "ProviderEndpoint": format!("https://{name}.example.internal"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected 200 from CreateHost, got {status} {body}"
    );
    body["HostArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateHost returned no HostArn: {body}"))
        .to_string()
}

/// Assert an id satisfies the modeled `RepositoryLinkId` pattern
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
fn assert_link_id(case: &str, id: &str) {
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected an 8-4-4-4-12 RepositoryLinkId, got {id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "case {case:?}: expected hex groups in the RepositoryLinkId, got {id:?}"
    );
}

/// Assert an ARN satisfies the modeled `RepositoryLinkArn` pattern
/// `^arn:aws(?:-[a-z]+)*:(codestar-connections|codeconnections):[a-z\-0-9]+:\d{12}:repository-link\/[a-zA-Z0-9\-:/]+$`
/// and names `id` in the region the caller signed for.
fn assert_link_arn(case: &str, arn: &str, id: &str) {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected a six-field ARN, got {arn:?}"
    );
    assert_eq!(fields[0], "arn", "case {case:?}: expected 'arn', got {arn:?}");
    assert_eq!(
        fields[1], "aws",
        "case {case:?}: expected partition 'aws', got {arn:?}"
    );
    assert_eq!(
        fields[2], "codeconnections",
        "case {case:?}: expected the codeconnections namespace, got {arn:?}"
    );
    assert_eq!(
        fields[3], REGION,
        "case {case:?}: expected the signed region, got {arn:?}"
    );
    assert_eq!(
        fields[4],
        engine::emulator_account_id(),
        "case {case:?}: expected the emulator account id, got {arn:?}"
    );
    assert_eq!(
        fields[5],
        format!("repository-link/{id}"),
        "case {case:?}: expected the ARN to name the reported id, got {arn:?}"
    );
}

/// Assert a response is the modeled error `expected_type` at `expected_status` — which
/// for a modeled CLIENT error of this service is the client status of its RPC protocol,
/// 400, whatever `@httpError` the shape records.
fn assert_error(case: &str, expected_status: u16, expected_type: &str, got: (u16, serde_json::Value)) {
    let (status, body) = got;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|kind| kind.ends_with(expected_type)),
        "case {case:?}: expected {expected_type}, got {body}"
    );
}

/// Every `X-Amz-Target` prefix this service answers to names the same operation: the
/// operation is the segment after the LAST dot, so the short service-shape-name form and
/// the fully qualified shape-id form (which an AWS SDK for this service sends, and whose
/// prefix itself carries dots) both route — while a target naming no modeled operation
/// is still the framework's routing refusal.
#[tokio::test]
async fn both_target_prefix_forms_route_to_the_same_operation() {
    // The service shape is `com.amazonaws.codeconnections#CodeConnections_20231201`
    // (`smithy_query(query="service_info")`), so the fully qualified target prefix is
    // that id with its namespace separator written as a dot.
    const PREFIXES: &[&str] = &[
        "CodeConnections_20231201",
        "com.amazonaws.codeconnections.CodeConnections_20231201",
    ];

    for (index, prefix) in PREFIXES.iter().enumerate() {
        let router = router();
        // The whole repository-link lifecycle, driven only through this target prefix.
        let (status, created) = call_with_target(
            &router,
            &format!("{prefix}.CreateConnection"),
            serde_json::json!({ "ConnectionName": "targets", "ProviderType": "GitHub" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected 200 from CreateConnection, got {status} {created}"
        );
        let connection = created["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {prefix:?}: no ConnectionArn: {created}"))
            .to_string();

        let (status, link) = call_with_target(
            &router,
            &format!("{prefix}.CreateRepositoryLink"),
            serde_json::json!({
                "ConnectionArn": connection,
                "OwnerId": format!("owner-{index}"),
                "RepositoryName": format!("repo-{index}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected 200 from CreateRepositoryLink, got {status} {link}"
        );
        let id = link["RepositoryLinkInfo"]["RepositoryLinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {prefix:?}: no RepositoryLinkId: {link}"))
            .to_string();

        let (status, deleted) = call_with_target(
            &router,
            &format!("{prefix}.DeleteRepositoryLink"),
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected 200 from DeleteRepositoryLink, got {status} {deleted}"
        );

        // The row is gone under this prefix too, and a repeat delete is the same
        // idempotent 200 — not the framework's 501 for a target it could not resolve.
        let (status, again) = call_with_target(
            &router,
            &format!("{prefix}.DeleteRepositoryLink"),
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected an idempotent 200 from the repeat delete, got {status} {again}"
        );
        assert_error(
            prefix,
            400,
            "ResourceNotFoundException",
            call_with_target(
                &router,
                &format!("{prefix}.GetRepositoryLink"),
                serde_json::json!({ "RepositoryLinkId": id }),
            )
            .await,
        );

        // A target of the same form naming no modeled operation stays unroutable.
        let (status, body) = call_with_target(
            &router,
            &format!("{prefix}.NotAModeledOperation"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 501,
            "case {prefix:?}: an unmodeled operation name must not route, got {status} {body}"
        );
    }
}

/// A repository link round trips from create to get, and `DeleteRepositoryLink` removes
/// exactly that stored row: after it the id resolves nowhere, and deleting it again is
/// the same empty 200 — the post-state the delete states already holds.
#[tokio::test]
async fn a_repository_link_round_trips_and_its_delete_removes_it() {
    let router = router();
    let connection = create(&router, "links", "GitHub").await;
    let mut ids = BTreeSet::new();

    for case in link_cases() {
        let info = create_link(&router, &case, &connection).await;
        let id = info["RepositoryLinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no RepositoryLinkId: {info}", case.label))
            .to_string();
        assert_link_id(case.label, &id);
        assert_link_arn(
            case.label,
            info["RepositoryLinkArn"].as_str().unwrap_or_default(),
            &id,
        );
        assert!(
            ids.insert(id.clone()),
            "case {:?}: every link gets a distinct id, saw {id:?} twice",
            case.label
        );
        // The provider comes off the connection the link was created against, not off
        // the request: the request names no provider at all.
        assert_eq!(
            info["ProviderType"],
            serde_json::json!("GitHub"),
            "case {:?}: expected the connection's provider type, got {info}",
            case.label
        );

        // The round trip: every member the create persisted is what the reader reports.
        let (status, got) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRepositoryLink, got {status} {got}",
            case.label
        );
        assert_eq!(
            got["RepositoryLinkInfo"], info,
            "case {:?}: expected GetRepositoryLink to report the created link, got {got}",
            case.label
        );
        assert_eq!(
            info["ConnectionArn"],
            serde_json::json!(connection),
            "case {:?}: expected the connection the create named, got {info}",
            case.label
        );
        assert_eq!(
            info["OwnerId"],
            serde_json::json!(case.owner_id),
            "case {:?}: expected the stored OwnerId, got {info}",
            case.label
        );
        assert_eq!(
            info["RepositoryName"],
            serde_json::json!(case.repository_name),
            "case {:?}: expected the stored RepositoryName, got {info}",
            case.label
        );
        match &case.encryption_key_arn {
            Some(key) => assert_eq!(
                info["EncryptionKeyArn"],
                serde_json::json!(key),
                "case {:?}: expected the stored EncryptionKeyArn, got {info}",
                case.label
            ),
            None => assert!(
                info["EncryptionKeyArn"].is_null(),
                "case {:?}: a link created with no EncryptionKeyArn reports none, got {info}",
                case.label
            ),
        }

        // A repository link is taggable, so its ARN resolves in the tag reader too.
        let (status, tags) = call(
            &router,
            "ListTagsForResource",
            serde_json::json!({ "ResourceArn": info["RepositoryLinkArn"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListTagsForResource, got {status} {tags}",
            case.label
        );
        assert_eq!(
            tags["Tags"],
            tag_list(&case.tags),
            "case {:?}: expected the tags the create supplied, got {tags}",
            case.label
        );

        // The delete itself: 200 with an empty body, and a repeat delete answers the
        // same — the post-state it states already holds the second time.
        for attempt in 1..=2 {
            let (status, deleted) = call(
                &router,
                "DeleteRepositoryLink",
                serde_json::json!({ "RepositoryLinkId": id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}, delete attempt {attempt}: expected 200 from DeleteRepositoryLink, got {status} {deleted}",
                case.label
            );
            assert_eq!(
                deleted,
                serde_json::json!({}),
                "case {:?}, delete attempt {attempt}: expected an empty body, got {deleted}",
                case.label
            );
        }

        // And the row is gone: no reader resolves the id or its ARN any more.
        assert_error(
            case.label,
            400,
            "ResourceNotFoundException",
            call(
                &router,
                "GetRepositoryLink",
                serde_json::json!({ "RepositoryLinkId": id }),
            )
            .await,
        );
        assert_error(
            case.label,
            400,
            "ResourceNotFoundException",
            call(
                &router,
                "ListTagsForResource",
                serde_json::json!({ "ResourceArn": info["RepositoryLinkArn"] }),
            )
            .await,
        );
    }
}

/// An id that names no repository link this account holds is an idempotent no-op: the
/// delete answers 200 with an empty body and the store is unchanged — every row is a
/// value the modeled `RepositoryLinkId` pattern admits, so the answer comes from the
/// handler and not from input validation, and the reader still reports the id as not
/// found.
#[tokio::test]
async fn deleting_an_id_that_names_no_link_is_an_idempotent_no_op() {
    let router = router();
    let connection = create(&router, "survivors", "Bitbucket").await;

    // Three live links, so "not found" can never mean "the store is empty".
    let mut live = Vec::new();
    for index in 0..3u32 {
        let case = LinkCase {
            label: "live link",
            owner_id: format!("owner-{index}"),
            repository_name: format!("repo-{index}"),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let info = create_link(&router, &case, &connection).await;
        live.push(
            info["RepositoryLinkId"]
                .as_str()
                .expect("a created link reports its id")
                .to_string(),
        );
    }

    // One live id, deleted, so a deleted id is one of the rows too.
    let gone = {
        let case = LinkCase {
            label: "deleted link",
            owner_id: "owner-gone".to_string(),
            repository_name: "repo-gone".to_string(),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let info = create_link(&router, &case, &connection).await;
        let id = info["RepositoryLinkId"]
            .as_str()
            .expect("a created link reports its id")
            .to_string();
        let (status, body) = call(
            &router,
            "DeleteRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "the first delete of a live link must succeed, got {status} {body}"
        );
        id
    };

    let unknown = [
        ("an id this store never minted", "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0".to_string()),
        ("the all-zero id", "00000000-0000-0000-0000-000000000000".to_string()),
        ("the all-f id", "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string()),
        // The pattern admits `[0-9a-fA-F]`, so an uppercase id is well-formed input.
        ("an uppercase id", "ABCDEF01-2345-6789-ABCD-EF0123456789".to_string()),
        ("the uppercasing of a live id", live[0].to_uppercase()),
        ("an id that was deleted", gone),
    ];

    for (label, id) in &unknown {
        let (status, body) = call(
            &router,
            "DeleteRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting an id that names no link is a no-op success, got {status} {body}"
        );
        assert_eq!(
            body,
            serde_json::json!({}),
            "case {label:?}: expected an empty body, got {body}"
        );
        // The id still names nothing: the no-op created no resource either.
        assert_error(
            label,
            400,
            "ResourceNotFoundException",
            call(
                &router,
                "GetRepositoryLink",
                serde_json::json!({ "RepositoryLinkId": id }),
            )
            .await,
        );
    }

    // Every no-op delete removed nothing: the live links still resolve.
    for id in &live {
        let (status, got) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {id:?}: a no-op delete must remove nothing, got {status} {got}"
        );
        assert_eq!(
            got["RepositoryLinkInfo"]["RepositoryLinkId"],
            serde_json::json!(id),
            "case {id:?}: expected the link to report its own id, got {got}"
        );
    }
}

/// Deleting one repository link removes exactly that one, whichever position it holds
/// among the links the account holds.
#[tokio::test]
async fn deleting_one_link_leaves_every_other_link_resolvable() {
    const LINKS: usize = 6;

    for target in 0..LINKS {
        let router = router();
        let connection = create(&router, "many", "GitLab").await;
        let mut ids = Vec::new();
        for index in 0..LINKS {
            let case = LinkCase {
                label: "one of many",
                owner_id: format!("owner-{index}"),
                repository_name: format!("repo-{index}"),
                encryption_key_arn: None,
                tags: Vec::new(),
            };
            let info = create_link(&router, &case, &connection).await;
            ids.push(
                info["RepositoryLinkId"]
                    .as_str()
                    .expect("a created link reports its id")
                    .to_string(),
            );
        }

        let (status, body) = call(
            &router,
            "DeleteRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": ids[target] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case target={target}: expected 200 from DeleteRepositoryLink, got {status} {body}"
        );

        for (index, id) in ids.iter().enumerate() {
            let (status, got) = call(
                &router,
                "GetRepositoryLink",
                serde_json::json!({ "RepositoryLinkId": id }),
            )
            .await;
            let expected = if index == target { 400 } else { 200 };
            assert_eq!(
                status, expected,
                "case target={target}, link {index}: expected HTTP {expected} from GetRepositoryLink, got {status} {got}"
            );
        }
    }
}

/// A repository link reports the provider type OF THE CONNECTION it was created
/// against, host-backed or not — and a `ConnectionArn` the store cannot resolve to a
/// connection with a provider type is refused with the modeled `InvalidInputException`
/// (400), the create's refusal for an input it cannot resolve.
#[tokio::test]
async fn a_link_reports_its_connections_provider_and_refuses_an_unresolvable_connection() {
    let router = router();

    for (index, provider) in PROVIDER_TYPES.iter().enumerate() {
        // A connection that states the provider and nothing else. The names are built
        // from the row's INDEX, not from the provider: the modelled `ConnectionName` is
        // `length 1..32`, which the longest provider value overruns.
        let direct = create(&router, &format!("direct-{index}"), provider).await;
        // And one of the same provider that is also associated with a host.
        let host = create_host(&router, &format!("host-{index}"), provider).await;
        let (status, created) = call(
            &router,
            "CreateConnection",
            serde_json::json!({
                "ConnectionName": format!("hosted-{index}"),
                "ProviderType": provider,
                "HostArn": host,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {provider:?}: expected 200 creating a host-backed connection, got {status} {created}"
        );
        let hosted = created["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {provider:?}: no ConnectionArn: {created}"))
            .to_string();

        for (label, connection_arn) in [("direct", &direct), ("host-backed", &hosted)] {
            let case = LinkCase {
                label: "provider resolution",
                owner_id: format!("{label}-owner"),
                repository_name: format!("{label}-{provider}"),
                encryption_key_arn: None,
                tags: Vec::new(),
            };
            let info = create_link(&router, &case, connection_arn).await;
            assert_eq!(
                info["ProviderType"],
                serde_json::json!(provider),
                "case {label}/{provider:?}: expected the connection's provider type, got {info}"
            );
        }
    }

    // A `ConnectionArn` this create cannot resolve to a connection with a provider
    // type. The modeled `ConnectionArn` pattern `^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$`
    // admits each of these, so the refusal comes from the handler resolving the
    // reference, not from input validation.
    let account = engine::emulator_account_id();
    // A connection created with NEITHER a `ProviderType` nor a `HostArn` names no
    // provider — nothing states one and there is no host to take one from — so it cannot
    // back a link: `RepositoryLinkInfo.ProviderType` is required and is the connection's.
    // (A connection created with a host and no `ProviderType` DOES name a provider: the
    // host's. That derivation is covered in `connection_reads.rs`.)
    let (status, created) = call(
        &router,
        "CreateConnection",
        serde_json::json!({
            "ConnectionName": "providerless",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a connection with neither ProviderType nor HostArn must still be creatable, got {status} {created}"
    );
    let providerless = created["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no ConnectionArn: {created}"))
        .to_string();

    let unresolvable = [
        (
            "a connection id this store never minted",
            format!("arn:aws:codeconnections:{REGION}:{account}:connection/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"),
        ),
        (
            "another account's connection",
            format!("arn:aws:codeconnections:{REGION}:210987654321:connection/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"),
        ),
        ("a connection that states no provider type", providerless),
    ];
    for (label, connection_arn) in &unresolvable {
        let case = LinkCase {
            label: "unresolvable connection",
            owner_id: "owner".to_string(),
            repository_name: format!("repo-{label}"),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        assert_error(
            label,
            400,
            "InvalidInputException",
            call(
                &router,
                "CreateRepositoryLink",
                create_link_body(&case, connection_arn),
            )
            .await,
        );
    }

    // An ARN of ANOTHER resource kind is not a connection ARN at all, so it is refused as
    // an INPUT-CONSTRAINT violation before anything is resolved: the repository-link
    // surface holds `ConnectionArn` to the connection-scoped expression rather than to
    // the generic ARN pattern the public model carries, which the loose pattern admits.
    // The expression and the message are pinned in `validation_message_rules.rs`.
    assert_error(
        "a host ARN, which is not a connection ARN",
        400,
        "ValidationException",
        call(
            &router,
            "CreateRepositoryLink",
            create_link_body(
                &LinkCase {
                    label: "not a connection ARN",
                    owner_id: "owner".to_string(),
                    repository_name: "repo-host-arn".to_string(),
                    encryption_key_arn: None,
                    tags: Vec::new(),
                },
                &create_host(&router, "not-a-connection", "GitHub").await,
            ),
        )
        .await,
    );
}

/// A link a sync configuration still references cannot be deleted: the delete is
/// refused with the modeled `SyncConfigurationStillExistsException` (HTTP 400, the shape
/// named in the body) and removes
/// nothing, and it succeeds once the configuration that referenced it is gone. Rows
/// differ in how many configurations reference the link and in which of them is removed
/// first, because the refusal holds while ANY reference is left.
#[tokio::test]
async fn a_link_a_sync_configuration_references_cannot_be_deleted() {
    // `ResourceName` is `length 1..100`, pattern `^[0-9A-Za-z]+[0-9A-Za-z_\\-]*$`, and
    // `SyncType` is the modeled `SyncConfigurationType` enum, whose only value is
    // `CFN_STACK_SYNC` (`smithy_query(query="operation",
    // operation="CreateSyncConfiguration")`).
    const SYNC_TYPE: &str = "CFN_STACK_SYNC";

    for references in 1..=3usize {
        let router = router();
        let connection = create(&router, "sync", "GitHub").await;
        let case = LinkCase {
            label: "referenced link",
            owner_id: "octo-org".to_string(),
            repository_name: "synced-repo".to_string(),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let info = create_link(&router, &case, &connection).await;
        let id = info["RepositoryLinkId"]
            .as_str()
            .expect("a created link reports its id")
            .to_string();

        // One `ResourceName` per reference; every configuration names this link.
        let resources: Vec<String> = (0..references)
            .map(|index| format!("Stack{index}"))
            .collect();
        for resource_name in &resources {
            let (status, body) = call(
                &router,
                "CreateSyncConfiguration",
                serde_json::json!({
                    "Branch": "main",
                    "ConfigFile": "deploy.yaml",
                    "RepositoryLinkId": id,
                    "ResourceName": resource_name,
                    "RoleArn": format!(
                        "arn:aws:iam::{}:role/sync-role",
                        engine::emulator_account_id()
                    ),
                    "SyncType": SYNC_TYPE,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case references={references}, {resource_name}: expected 200 from CreateSyncConfiguration, got {status} {body}"
            );
        }

        // Every reference but the last one is removed in turn: the refusal holds until
        // none is left.
        for (removed, resource_name) in resources.iter().enumerate() {
            let left = references - removed;
            assert_error(
                &format!("references={references}, {left} left"),
                400,
                "SyncConfigurationStillExistsException",
                call(
                    &router,
                    "DeleteRepositoryLink",
                    serde_json::json!({ "RepositoryLinkId": id }),
                )
                .await,
            );
            // The refused delete removed nothing: the link still resolves.
            let (status, got) = call(
                &router,
                "GetRepositoryLink",
                serde_json::json!({ "RepositoryLinkId": id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case references={references}, {left} left: a refused delete must remove nothing, got {status} {got}"
            );

            let (status, body) = call(
                &router,
                "DeleteSyncConfiguration",
                serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case references={references}, {resource_name}: expected 200 from DeleteSyncConfiguration, got {status} {body}"
            );
        }

        // With the last reference gone the delete goes through.
        let (status, body) = call(
            &router,
            "DeleteRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case references={references}: expected 200 once no configuration references the link, got {status} {body}"
        );
        assert_error(
            &format!("references={references}, after delete"),
            400,
            "ResourceNotFoundException",
            call(
                &router,
                "GetRepositoryLink",
                serde_json::json!({ "RepositoryLinkId": id }),
            )
            .await,
        );
    }
}

/// A repository link IS the association of one connection with one repository, so a
/// second create for the same connection, owner and repository is refused with the
/// modeled `ResourceAlreadyExistsException` (HTTP 400, the shape named in the body) —
/// and the association is free again
/// once the link is deleted.
#[tokio::test]
async fn one_link_per_connection_and_repository() {
    let router = router();
    let first = create(&router, "first", "GitHub").await;
    let second = create(&router, "second", "GitHub").await;

    for case in link_cases() {
        let info = create_link(&router, &case, &first).await;
        let id = info["RepositoryLinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no RepositoryLinkId: {info}", case.label))
            .to_string();

        assert_error(
            case.label,
            400,
            "ResourceAlreadyExistsException",
            call(
                &router,
                "CreateRepositoryLink",
                create_link_body(&case, &first),
            )
            .await,
        );

        // Another connection to the same repository is a different association.
        let other = create_link(&router, &case, &second).await;
        assert_ne!(
            other["RepositoryLinkId"], info["RepositoryLinkId"],
            "case {:?}: a link on another connection is a distinct resource, got {other}",
            case.label
        );

        // Deleting the link frees the association: the same create succeeds again, and
        // mints an id the store never handed out before.
        let (status, body) = call(
            &router,
            "DeleteRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from DeleteRepositoryLink, got {status} {body}",
            case.label
        );
        let again = create_link(&router, &case, &first).await;
        assert_ne!(
            again["RepositoryLinkId"],
            serde_json::json!(id),
            "case {:?}: a deleted link's id must never be minted again, got {again}",
            case.label
        );
    }
}
