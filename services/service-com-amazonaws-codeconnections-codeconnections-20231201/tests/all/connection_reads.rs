// SPDX-License-Identifier: CC0-1.0

//! What a connection READ reports about the connection, over the wire.
//!
//! THE RULE: every reader of a connection renders the whole stored row — including the
//! connection's ownership, which for a connection created in the calling account is
//! owned by that account — and the connection's provider type is the provider the
//! connection talks to: the one the create stated, or, when the create named only a
//! host, the provider that host is installed with (a host's provider type is modelled
//! required), persisted on the connection so every reader and every resource derived
//! from the connection reports the same provider. A connection whose create named
//! neither states no provider, so the member is absent. The tenant endpoint is part of
//! that same row and belongs to ONE provider: it is stored and reported for a connection
//! whose provider is `GitHubEnterpriseCloudDataResidency` and refused for every other.
//!
//! Each test asserts the rule over a TABLE whose rows differ along what the rule
//! generalises over: which provider the connection talks to (every value the modelled
//! `ProviderType` enum admits), and WHICH WAY the create names it — stated in the
//! request, taken from the host, stated by both, or named by neither. The recorded
//! units' cases (a connection created with a provider type, and one created with only a
//! GitHub Enterprise Server host) are two rows of that table. Each row is read back
//! through both readers — `GetConnection` and `ListConnections` — because each
//! serializes the projection itself.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers, so these tests exercise the same path a
//! signed SDK request takes.

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{PROVIDER_TYPES, call, router};
use super::git_sync::error_code;
use super::host_association::create_plain_host;

/// The ownership every connection this emulator mints reports: it is created in the
/// calling account, which is therefore the account that owns it.
const OWNED: &str = "Owned";

/// The one `ProviderType` a connection carries a tenant endpoint for, as
/// `CreateConnection$ProviderEndpoint` documents it: "the endpoint of the ghe.com tenant
/// where your organization resides. Provide this parameter only when `ProviderType` is
/// `GitHubEnterpriseCloudDataResidency`. Leave this parameter empty for all other provider
/// types." Written out as the literal rather than read off the implementation's own
/// constant.
const TENANT_ENDPOINT_PROVIDER: &str = "GitHubEnterpriseCloudDataResidency";

/// The published bounds of the modelled `Url` the endpoint member targets (`length
/// 1..512`, from `smithy_query(query="shape", shape="Url")`).
const ENDPOINT_MIN_LEN: usize = 1;
const ENDPOINT_MAX_LEN: usize = 512;

/// The sentence `CreateConnection` answers for an endpoint stated for a connection of any
/// other provider, written out as the LITERAL rather than read off the implementation's
/// own constant, so rewording that constant fails this suite.
const PROVIDER_ENDPOINT_NOT_SUPPORTED: &str =
    "The input is not valid. ProviderEndpoint is supported only for a connection with a \
     ProviderType of GitHubEnterpriseCloudDataResidency, and must be empty for all other \
     provider types.";

/// One row of the table: how a create names the provider the connection talks to.
struct ProviderCase {
    /// What a failing assertion names this row by.
    label: String,
    /// The `ProviderType` the request states, or `None` for a request that omits it.
    stated: Option<&'static str>,
    /// The provider the host named by the request is installed with, or `None` for a
    /// request that names no host.
    host_provider: Option<&'static str>,
    /// The `ProviderType` the connection must report, or `None` when the connection
    /// names no provider at all.
    expected: Option<&'static str>,
}

/// The table: for each value the modelled `ProviderType` admits, the three ways a
/// create can name it (stated, taken from the host, stated by both), plus the one row
/// in which nothing names a provider.
///
/// The provider catalog is `connection_lifecycle::PROVIDER_TYPES`, which is the
/// enum's own `values` list from
/// `smithy_query(query="shape", shape="ProviderType")` — so a value added to the model
/// joins this table rather than being left untested.
fn provider_cases() -> Vec<ProviderCase> {
    let mut cases = Vec::new();
    for provider in PROVIDER_TYPES {
        cases.push(ProviderCase {
            label: format!("the request states {provider}"),
            stated: Some(provider),
            host_provider: None,
            expected: Some(provider),
        });
        cases.push(ProviderCase {
            label: format!("the request names only a host installed with {provider}"),
            stated: None,
            host_provider: Some(provider),
            expected: Some(provider),
        });
        cases.push(ProviderCase {
            label: format!("the request states {provider} and names a host installed with it"),
            stated: Some(provider),
            host_provider: Some(provider),
            expected: Some(provider),
        });
    }
    // The one row nothing names a provider on: the modelled `CreateConnection` input makes both
    // `ProviderType` and `HostArn` optional (`smithy_query(query="operation",
    // operation="CreateConnection")` lists `ConnectionName` as its only required member), so a
    // create naming neither is accepted and the connection it mints names no provider — there is
    // no host to take one from. The row is here for the derivation rule's boundary, not as a claim
    // about that create's refusal.
    cases.push(ProviderCase {
        label: "the request names neither a provider type nor a host".to_string(),
        stated: None,
        host_provider: None,
        expected: None,
    });
    cases
}

/// Create the connection one row describes and return `(ConnectionArn, HostArn)`.
async fn create_connection(
    router: &MultiServiceRouter,
    case: &ProviderCase,
    index: usize,
) -> (String, Option<String>) {
    let host_arn = match case.host_provider {
        Some(provider) => {
            Some(create_plain_host(router, &format!("host-{index}"), provider).await)
        }
        None => None,
    };

    let mut body = serde_json::json!({ "ConnectionName": format!("conn-{index}") });
    if let Some(stated) = case.stated {
        body["ProviderType"] = serde_json::json!(stated);
    }
    if let Some(host_arn) = host_arn.as_deref() {
        body["HostArn"] = serde_json::json!(host_arn);
    }

    let (status, created) = call(router, "CreateConnection", body).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200 from CreateConnection, got {status} {created}",
        case.label
    );
    let connection_arn = created["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no ConnectionArn: {created}", case.label))
        .to_string();
    (connection_arn, host_arn)
}

/// The `Connection` structure every reader of one connection must report for a row.
fn expected_connection(
    case: &ProviderCase,
    index: usize,
    connection_arn: &str,
    host_arn: Option<&str>,
) -> serde_json::Value {
    let mut expected = serde_json::json!({
        "ConnectionName": format!("conn-{index}"),
        "ConnectionArn": connection_arn,
        "OwnerAccountId": engine::emulator_account_id(),
        "ConnectionStatus": "PENDING",
        "OwnershipType": OWNED,
    });
    if let Some(provider) = case.expected {
        expected["ProviderType"] = serde_json::json!(provider);
    }
    if let Some(host_arn) = host_arn {
        expected["HostArn"] = serde_json::json!(host_arn);
    }
    expected
}

/// `GetConnection` reports the ownership and the provider type of every connection,
/// however the create named that provider — and a second read reports the same.
#[tokio::test]
async fn get_connection_reports_the_ownership_and_the_resolved_provider_type() {
    let router = router();

    for (index, case) in provider_cases().iter().enumerate() {
        let (connection_arn, host_arn) = create_connection(&router, case, index).await;

        let (status, got) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": connection_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetConnection, got {status} {got}",
            case.label
        );
        assert_eq!(
            got["Connection"],
            expected_connection(case, index, &connection_arn, host_arn.as_deref()),
            "case {:?}: expected the whole stored connection, got {got}",
            case.label
        );

        // A read does not change what the next caller sees.
        let (_, again) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": connection_arn }),
        )
        .await;
        assert_eq!(
            again, got,
            "case {:?}: a second read must report the same connection, got {again}",
            case.label
        );
    }
}

/// `ListConnections` renders each element from the same projection, so every listed
/// element equals what `GetConnection` reports for that connection — ownership and
/// resolved provider type included.
#[tokio::test]
async fn listed_connections_carry_the_same_ownership_and_provider_type() {
    let router = router();

    let cases = provider_cases();
    let mut expected_elements = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let (connection_arn, host_arn) = create_connection(&router, case, index).await;
        expected_elements.push((
            case,
            expected_connection(case, index, &connection_arn, host_arn.as_deref()),
        ));
    }

    // One page per row, so the listing is walked through its own token rather than in a
    // single response: every element the report renders is checked.
    let mut listed: Vec<serde_json::Value> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = serde_json::json!({ "MaxResults": 1 });
        if let Some(token) = next_token.as_deref() {
            body["NextToken"] = serde_json::json!(token);
        }
        let (status, page) = call(&router, "ListConnections", body).await;
        assert_eq!(
            status, 200,
            "ListConnections: expected 200, got {status} {page}"
        );
        let entries = page["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("ListConnections reported no Connections: {page}"));
        listed.extend(entries.iter().cloned());
        match page["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }

    assert_eq!(
        listed.len(),
        expected_elements.len(),
        "expected every created connection to be listed, got {listed:?}"
    );
    for (case, expected) in &expected_elements {
        assert!(
            listed.contains(expected),
            "case {:?}: expected the listed element {expected}, got {listed:?}",
            case.label
        );
    }
}

/// The provider type a connection took from its host is PERSISTED on the connection, so
/// everything that keys on a connection's provider sees it: `ListConnections`'
/// `ProviderTypeFilter` matches the connection, and a repository link created against it
/// reports that provider as its own.
#[tokio::test]
async fn a_host_derived_provider_type_drives_the_filter_and_the_derived_link() {
    let router = router();

    for (index, provider) in PROVIDER_TYPES.iter().enumerate() {
        let case = ProviderCase {
            label: format!("a connection on a host installed with {provider}"),
            stated: None,
            host_provider: Some(provider),
            expected: Some(provider),
        };
        let (connection_arn, host_arn) = create_connection(&router, &case, index).await;

        let (status, filtered) = call(
            &router,
            "ListConnections",
            serde_json::json!({ "ProviderTypeFilter": provider }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListConnections, got {status} {filtered}",
            case.label
        );
        let arns: Vec<&str> = filtered["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no Connections: {filtered}", case.label))
            .iter()
            .filter_map(|entry| entry["ConnectionArn"].as_str())
            .collect();
        assert!(
            arns.contains(&connection_arn.as_str()),
            "case {:?}: expected the filter on the host's provider to match the connection, got {arns:?}",
            case.label
        );

        // `RepositoryLinkInfo.ProviderType` is the provider type of the connection the
        // link is associated with, so a link on a host-based connection reports the
        // host's provider too.
        let (status, link) = call(
            &router,
            "CreateRepositoryLink",
            serde_json::json!({
                "ConnectionArn": connection_arn,
                "OwnerId": format!("owner-{index}"),
                "RepositoryName": format!("repo-{index}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateRepositoryLink, got {status} {link}",
            case.label
        );
        assert_eq!(
            link["RepositoryLinkInfo"]["ProviderType"],
            serde_json::json!(provider),
            "case {:?}: expected the link to report the connection's provider, got {link}",
            case.label
        );

        // The host the provider came from is the one the connection reports.
        assert_eq!(
            host_arn.as_deref(),
            Some(
                call(
                    &router,
                    "GetConnection",
                    serde_json::json!({ "ConnectionArn": connection_arn }),
                )
                .await
                .1["Connection"]["HostArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {:?}: no HostArn reported", case.label))
            ),
            "case {:?}: expected the connection to report the host it was created on",
            case.label
        );
    }
}

/// One row of the tenant-endpoint table: what the create states, and what the connection
/// must report.
struct EndpointCase {
    /// What a failing assertion names this row by.
    label: String,
    /// The `ProviderType` the request states, or `None` for a request that omits it.
    provider: Option<&'static str>,
    /// The provider the host named by the request is installed with, or `None` for a
    /// request that names no host.
    host_provider: Option<&'static str>,
    /// The `ProviderEndpoint` the request states.
    endpoint: String,
    /// Whether the create is accepted: the endpoint belongs to one provider, so only a
    /// connection whose RESOLVED provider is that one carries it.
    accepted: bool,
}

/// The table: every value the modelled `ProviderType` admits, stated by the request and
/// taken from a host, plus the row that names no provider at all — each with an endpoint —
/// and the endpoint forms the modelled `Url` admits for the one provider that carries it
/// (its published bounds, and a host with a port and a path).
fn endpoint_cases() -> Vec<EndpointCase> {
    let mut cases = Vec::new();
    for provider in PROVIDER_TYPES {
        let accepted = *provider == TENANT_ENDPOINT_PROVIDER;
        cases.push(EndpointCase {
            label: format!("the request states {provider} and an endpoint"),
            provider: Some(provider),
            host_provider: None,
            endpoint: "https://tenant.ghe.com".to_string(),
            accepted,
        });
        cases.push(EndpointCase {
            label: format!(
                "the request names an endpoint and only a host installed with {provider}"
            ),
            provider: None,
            host_provider: Some(provider),
            endpoint: "https://tenant.ghe.com".to_string(),
            accepted,
        });
    }
    cases.push(EndpointCase {
        label: "the request names an endpoint and no provider at all".to_string(),
        provider: None,
        host_provider: None,
        endpoint: "https://tenant.ghe.com".to_string(),
        accepted: false,
    });
    // The endpoint forms, on the provider that carries one: the member's published
    // minimum and maximum length, and an endpoint with a port and a path.
    for endpoint in [
        "x".repeat(ENDPOINT_MIN_LEN),
        format!("https://{}", "e".repeat(ENDPOINT_MAX_LEN - "https://".len())),
        "https://tenant.ghe.com:8443/my-org".to_string(),
    ] {
        cases.push(EndpointCase {
            label: format!("an endpoint of {} characters", endpoint.len()),
            provider: Some(TENANT_ENDPOINT_PROVIDER),
            host_provider: None,
            endpoint,
            accepted: true,
        });
    }
    cases
}

/// The tenant endpoint is a fact about the connection's provider, not about the request:
/// a connection whose resolved provider is the one that endpoint belongs to stores it and
/// reports it through both readers, and a create that states one for any other provider —
/// or for no provider at all — is refused with the modelled `InvalidInputException` (400)
/// and persists nothing.
#[tokio::test]
async fn a_tenant_endpoint_is_stored_for_its_one_provider_and_refused_for_every_other() {
    let router = router();

    for (index, case) in endpoint_cases().iter().enumerate() {
        let name = format!("endpoint-{index}");
        let host_arn = match case.host_provider {
            Some(provider) => {
                Some(create_plain_host(&router, &format!("eh-{index}"), provider).await)
            }
            None => None,
        };

        let mut body = serde_json::json!({
            "ConnectionName": name,
            "ProviderEndpoint": case.endpoint,
        });
        if let Some(provider) = case.provider {
            body["ProviderType"] = serde_json::json!(provider);
        }
        if let Some(host_arn) = host_arn.as_deref() {
            body["HostArn"] = serde_json::json!(host_arn);
        }
        let (status, created) = call(&router, "CreateConnection", body).await;

        if !case.accepted {
            assert_eq!(
                status, 400,
                "case {:?}: expected 400 from CreateConnection, got {status} {created}",
                case.label
            );
            assert_eq!(
                error_code(&created),
                "InvalidInputException",
                "case {:?}: expected InvalidInputException, got {created}",
                case.label
            );
            let message = created["Message"]
                .as_str()
                .or_else(|| created["message"].as_str())
                .unwrap_or_else(|| panic!("case {:?}: no message: {created}", case.label))
                .to_string();
            assert_eq!(
                message, PROVIDER_ENDPOINT_NOT_SUPPORTED,
                "case {:?}: expected the fixed sentence, got {message:?}",
                case.label
            );
            assert!(
                !message.contains(&case.endpoint),
                "case {:?}: the refusal must not echo the endpoint, got {message:?}",
                case.label
            );
            // A refused create persists nothing: no connection of that name is listed.
            let (_, listed) = call(&router, "ListConnections", serde_json::json!({})).await;
            let listed_names: Vec<&str> = listed["Connections"]
                .as_array()
                .unwrap_or_else(|| panic!("case {:?}: no Connections: {listed}", case.label))
                .iter()
                .filter_map(|connection| connection["ConnectionName"].as_str())
                .collect();
            assert!(
                !listed_names.contains(&name.as_str()),
                "case {:?}: a refused create must persist nothing, got {listed_names:?}",
                case.label
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateConnection, got {status} {created}",
            case.label
        );
        let connection_arn = created["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no ConnectionArn: {created}", case.label))
            .to_string();

        let mut expected = serde_json::json!({
            "ConnectionName": name,
            "ConnectionArn": connection_arn,
            "OwnerAccountId": engine::emulator_account_id(),
            "ConnectionStatus": "PENDING",
            "ProviderType": TENANT_ENDPOINT_PROVIDER,
            "ProviderEndpoint": case.endpoint,
            "OwnershipType": OWNED,
        });
        if let Some(host_arn) = host_arn.as_deref() {
            expected["HostArn"] = serde_json::json!(host_arn);
        }

        let (status, got) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": connection_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetConnection, got {status} {got}",
            case.label
        );
        assert_eq!(
            got["Connection"], expected,
            "case {:?}: expected the whole stored connection, got {got}",
            case.label
        );

        // The second reader renders the same projection, so it reports the same element.
        let (status, listed) = call(
            &router,
            "ListConnections",
            serde_json::json!({ "ProviderTypeFilter": TENANT_ENDPOINT_PROVIDER }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListConnections, got {status} {listed}",
            case.label
        );
        assert!(
            listed["Connections"]
                .as_array()
                .unwrap_or_else(|| panic!("case {:?}: no Connections: {listed}", case.label))
                .contains(&expected),
            "case {:?}: expected the listed element {expected}, got {listed}",
            case.label
        );
    }
}

/// A connection of the provider that MAY carry a tenant endpoint, created without one,
/// reports none: the member is the endpoint the create stated, and nothing is invented for
/// a connection that stated none.
#[tokio::test]
async fn a_connection_created_without_a_tenant_endpoint_reports_none() {
    let router = router();

    let (status, created) = call(
        &router,
        "CreateConnection",
        serde_json::json!({
            "ConnectionName": "no-endpoint",
            "ProviderType": TENANT_ENDPOINT_PROVIDER,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from CreateConnection, got {status} {created}"
    );
    let connection_arn = created["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no ConnectionArn: {created}"))
        .to_string();

    let (status, got) = call(
        &router,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": connection_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetConnection, got {status} {got}"
    );
    assert!(
        got["Connection"]["ProviderEndpoint"].is_null(),
        "a connection created with no endpoint reports none, got {got}"
    );
    assert_eq!(
        got["Connection"]["ProviderType"],
        serde_json::json!(TENANT_ENDPOINT_PROVIDER),
        "expected the stated provider, got {got}"
    );
}

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declarations are checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-codeconnections-codeconnections-20231201.json"
));

/// The overlay model the local declarations are generated from, embedded for the same
/// reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shape the overlay restates with one member more than the snapshot declares.
const CONNECTION_SHAPE: &str = "com.amazonaws.codeconnections#Connection";

/// The member AWS answers every connection read with, which the snapshot does not declare.
const LOCAL_MEMBER: &str = "OwnershipType";

/// The shape that member targets, which the snapshot declares nowhere.
const LOCAL_SHAPE: &str = "com.amazonaws.codeconnections#OwnershipType";

/// The shape the overlay restates with one requirement fewer than the snapshot declares.
const SYNC_STATUS_OUTPUT_SHAPE: &str = "com.amazonaws.codeconnections#GetResourceSyncStatusOutput";

/// The member of it the snapshot marks `@required` and the overlay leaves optional, because
/// AWS answers a resource that has never synced with none of the sync-state members set.
const RELAXED_MEMBER: &str = "LatestSync";

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> serde_json::Value {
    let parsed: serde_json::Value =
        serde_json::from_str(model).expect("a model document must be JSON");
    serde_json::Value::Object(
        parsed["shapes"]
            .as_object()
            .expect("a model document must carry a shapes object")
            .clone(),
    )
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types, enum
/// values and constraint traits — so comparing declarations has to ignore the one trait
/// that carries no behaviour.
fn without_docs(declaration: &serde_json::Value) -> serde_json::Value {
    match declaration {
        serde_json::Value::Object(members) => serde_json::Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits"
                        || !value.as_object().is_some_and(serde_json::Map::is_empty)
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(without_docs).collect())
        }
        other => other.clone(),
    }
}

/// `declaration` with the `@required` trait dropped as well: how a member the overlay
/// leaves optional must otherwise match the snapshot's.
fn without_required(declaration: &serde_json::Value) -> serde_json::Value {
    let mut stripped = without_docs(declaration);
    if let Some(traits) = stripped
        .get_mut("traits")
        .and_then(serde_json::Value::as_object_mut)
    {
        traits.remove("smithy.api#required");
        if traits.is_empty() {
            stripped
                .as_object_mut()
                .expect("the declaration is an object")
                .remove("traits");
        }
    }
    stripped
}

/// Whether `declaration` is a bare structure: a target the overlay names only so the
/// member schema knows its shape kind, whose own members are declared by the generated
/// module the snapshot renders.
fn is_target_stub(declaration: &serde_json::Value) -> bool {
    declaration["type"] == serde_json::json!("structure")
        && declaration.get("members").is_none()
        && declaration.get("traits").is_none()
}

/// The two local declarations are generated from the overlay model beside the snapshot
/// (`smithy-model/LOCAL-ADDITIONS.md`), so anything the overlay restates of the snapshot —
/// a member's target, a shape's type, an enum's value set, a length or a pattern — must
/// still BE what the snapshot says. The only things it may differ in are the member AWS
/// answers with and the snapshot does not declare, and the one requirement AWS does not
/// hold a never-synced resource to.
///
/// A refreshed snapshot that declares that member, relaxes that requirement itself, adds a
/// member the overlay does not restate, or changes a target, value set or constraint fails
/// here rather than silently narrowing what this service accepts and reports.
#[test]
fn the_local_additions_overlay_restates_the_snapshot_but_for_what_aws_answers() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        if shape_id == LOCAL_SHAPE {
            assert!(
                snapshot[shape_id].is_null(),
                "case {shape_id:?}: the snapshot now declares it, so the local declaration \
                 should give way to the modelled shape"
            );
            continue;
        }
        let modelled = &snapshot[shape_id];
        assert!(
            !modelled.is_null(),
            "case {shape_id:?}: the overlay restates a shape the snapshot no longer declares"
        );

        if shape_id == CONNECTION_SHAPE {
            // The snapshot's members, restated as the snapshot declares them, plus the one
            // AWS answers with and it does not.
            let declared_members = declared["members"]
                .as_object()
                .expect("the overlay declares the Connection members");
            let modelled_members = modelled["members"]
                .as_object()
                .expect("the snapshot declares the Connection members");
            for (member, modelled_member) in modelled_members {
                assert_eq!(
                    without_docs(&declared_members[member]),
                    without_docs(modelled_member),
                    "case {shape_id:?}: member {member} must be restated as the snapshot \
                     declares it, got {:?}",
                    declared_members.get(member)
                );
            }
            let added: Vec<&String> = declared_members
                .keys()
                .filter(|member| !modelled_members.contains_key(*member))
                .collect();
            assert_eq!(
                added,
                vec![LOCAL_MEMBER],
                "case {shape_id:?}: it may add {LOCAL_MEMBER} and nothing else, added {added:?}"
            );
            continue;
        }

        if shape_id == SYNC_STATUS_OUTPUT_SHAPE {
            // The same members, each as the snapshot declares it — but for the one the
            // overlay leaves optional, which must otherwise match and which the snapshot
            // must still require, or the overlay has nothing left to do.
            let declared_members = declared["members"]
                .as_object()
                .expect("the overlay declares the output's members");
            let modelled_members = modelled["members"]
                .as_object()
                .expect("the snapshot declares the output's members");
            assert_eq!(
                declared_members.keys().collect::<Vec<_>>(),
                modelled_members.keys().collect::<Vec<_>>(),
                "case {shape_id:?}: it must restate exactly the snapshot's members"
            );
            for (member, modelled_member) in modelled_members {
                if member == RELAXED_MEMBER {
                    assert_eq!(
                        modelled_member["traits"]["smithy.api#required"],
                        serde_json::json!({}),
                        "case {shape_id:?}: the snapshot no longer requires {member}, so the \
                         local declaration should give way to the modelled shape"
                    );
                    assert_eq!(
                        without_docs(&declared_members[member]),
                        without_required(modelled_member),
                        "case {shape_id:?}: {member} must be restated as the snapshot declares \
                         it but for its requirement, got {:?}",
                        declared_members.get(member)
                    );
                    continue;
                }
                assert_eq!(
                    without_docs(&declared_members[member]),
                    without_docs(modelled_member),
                    "case {shape_id:?}: member {member} must be restated as the snapshot \
                     declares it, got {:?}",
                    declared_members.get(member)
                );
            }
            continue;
        }

        if is_target_stub(declared) {
            // A target named only for its shape kind: the kind is all this claims of it.
            assert_eq!(
                declared["type"], modelled["type"],
                "case {shape_id:?}: the snapshot declares another kind of shape"
            );
            continue;
        }

        assert_eq!(
            without_docs(declared),
            without_docs(modelled),
            "case {shape_id:?}: it must be restated exactly as the snapshot declares it"
        );
    }

    // And every shape the restated members target is itself restated, so nothing the local
    // declarations read comes from outside the overlay.
    for shape_id in [CONNECTION_SHAPE, SYNC_STATUS_OUTPUT_SHAPE] {
        for (member, declared) in restated[shape_id]["members"]
            .as_object()
            .unwrap_or_else(|| panic!("the overlay declares the members of {shape_id}"))
        {
            let target = declared["target"].as_str().unwrap_or_default();
            assert!(
                restated.contains_key(target),
                "case {shape_id:?}/{member:?}: its target {target:?} must be restated in the \
                 overlay too"
            );
        }
    }
}
