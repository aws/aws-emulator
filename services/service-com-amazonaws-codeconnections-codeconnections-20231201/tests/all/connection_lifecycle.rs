// SPDX-License-Identifier: CC0-1.0

//! The connection lifecycle over the wire: create, get, list (paged and filtered),
//! list tags, delete — every answer computed from what `CreateConnection` persisted.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch (`POST /` with
//! `X-Amz-Target: CodeConnections_20231201.<Operation>`, awsJson1_0 bodies), so these
//! tests exercise the same path a signed SDK request takes.

use std::collections::BTreeSet;
use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codeconnections_codeconnections_20231201::state::{
    MAX_CONNECTIONS, MAX_PAGE_SIZE,
};

/// The region the test signs for, so a minted ARN must carry exactly this one.
pub(crate) const REGION: &str = "eu-west-1";

/// The `ProviderType` values the model accepts, from
/// `smithy_query(query="shape", shape="ProviderType")` — the enum's `values` list.
pub(crate) const PROVIDER_TYPES: &[&str] = &[
    "Bitbucket",
    "GitHub",
    "GitHubEnterpriseServer",
    "GitLab",
    "GitLabSelfManaged",
    "AzureDevOps",
    "GitHubEnterpriseCloudDataResidency",
];

/// A host ARN matching the modeled `HostArn` pattern. This suite mints no host, so
/// it names a resource this service does not hold (`host_association.rs` covers the
/// case where the host is really there).
fn some_host_arn() -> String {
    format!(
        "arn:aws:codeconnections:{REGION}:{}:host/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
        engine::emulator_account_id()
    )
}

/// One emulator holding only this service, driven in-process.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The same emulator, with the engine it was built over.
///
/// A suite that has to seed state the API cannot yet write (a resource's sync state,
/// say — `CreateSyncConfiguration` is still a scaffold) reaches the store through this
/// engine, which is the SAME one every handler of the returned router reads.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests assert handler behaviour, not signature verification.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codeconnections_codeconnections_20231201::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Call one operation and return its HTTP status and parsed JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call_signed_for(router, REGION, operation, body).await
}

/// Call one operation with the credential scope naming `scope_region`.
pub(crate) async fn call_signed_for(
    router: &MultiServiceRouter,
    scope_region: &str,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call_signed_with_target(
        router,
        scope_region,
        &format!("CodeConnections_20231201.{operation}"),
        body,
    )
    .await
}

/// Call with a literal `X-Amz-Target` header value, so a test can pin which target
/// forms this service routes. The one place this suite builds a request: every other
/// helper here delegates to it, so the SigV4 and header boilerplate exists once.
pub(crate) async fn call_with_target(
    router: &MultiServiceRouter,
    target: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call_signed_with_target(router, REGION, target, body).await
}

async fn call_signed_with_target(
    router: &MultiServiceRouter,
    scope_region: &str,
    target: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTCODECONN/20261021/{scope_region}/codeconnections/aws4_request, \
                 SignedHeaders=content-type;host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "target {target}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Create a connection and return its ARN, failing the test on any non-200.
pub(crate) async fn create(router: &MultiServiceRouter, name: &str, provider: &str) -> String {
    let (status, body) = call(
        router,
        "CreateConnection",
        serde_json::json!({ "ConnectionName": name, "ProviderType": provider }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}/{provider:?}: expected 200 from CreateConnection, got {status} {body}"
    );
    body["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateConnection returned no ConnectionArn: {body}"))
        .to_string()
}

/// Assert an ARN satisfies the modeled `ConnectionArn` pattern
/// `^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$` and names a connection in this region.
fn assert_connection_arn(case: &str, arn: &str) {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case}: expected a six-field ARN, got {arn:?}"
    );
    assert_eq!(fields[0], "arn", "case {case}: expected 'arn', got {arn:?}");
    assert_eq!(
        fields[1], "aws",
        "case {case}: expected partition 'aws', got {arn:?}"
    );
    assert_eq!(
        fields[2], "codeconnections",
        "case {case}: expected the codeconnections namespace, got {arn:?}"
    );
    assert_eq!(
        fields[3], REGION,
        "case {case}: expected the signed region, got {arn:?}"
    );
    assert_eq!(
        fields[4],
        engine::emulator_account_id(),
        "case {case}: expected the emulator account id, got {arn:?}"
    );
    assert!(
        fields[4].len() == 12 && fields[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case}: the account field must be 12 digits, got {arn:?}"
    );
    let resource = fields[5];
    assert!(
        resource.starts_with("connection/") && resource.len() > "connection/".len(),
        "case {case}: expected a 'connection/<id>' resource, got {arn:?}"
    );
}

/// Every connection's resource id is minted by the service: 32 lowercase hex
/// characters grouped 8-4-4-4-12, distinct for every connection, and never reused.
#[tokio::test]
async fn every_minted_resource_id_is_opaque_and_distinct() {
    let router = router();
    // More than one, more than two: enough connections that a repeated or malformed id
    // shows up. The names differ only in their index, so the id cannot be derived from
    // the name's shape.
    const CONNECTIONS: usize = 12;

    let mut ids = BTreeSet::new();
    for index in 0..CONNECTIONS {
        let case = format!("connection {index}");
        let arn = create(&router, &format!("conn-{index}"), "GitHub").await;
        assert_connection_arn(&case, &arn);
        let id = arn
            .rsplit_once("connection/")
            .map(|(_, id)| id.to_string())
            .unwrap_or_else(|| panic!("case {case}: expected a 'connection/<id>' ARN, got {arn:?}"));

        // The grouping of a connection id: 8-4-4-4-12 lowercase hex.
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "case {case}: expected an id grouped 8-4-4-4-12, got {id:?}"
        );
        assert!(
            groups
                .iter()
                .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
            "case {case}: expected lowercase hex in every group, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case}: id {id:?} was already minted for another connection"
        );
    }
    assert_eq!(
        ids.len(),
        CONNECTIONS,
        "expected one distinct id per connection, got {ids:?}"
    );
}

/// The whole script the bootstrap asks for: create with tags, read it back, page it,
/// read its tags, delete it, then observe it is gone from both the get and the list.
#[tokio::test]
async fn a_connection_round_trips_from_create_to_delete() {
    let router = router();

    let (status, created) = call(
        &router,
        "CreateConnection",
        serde_json::json!({
            "ConnectionName": "my-conn",
            "ProviderType": "GitHub",
            "Tags": [{ "Key": "env", "Value": "test" }],
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateConnection: expected 200, got {status} {created}");
    let arn = created["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateConnection returned no ConnectionArn: {created}"))
        .to_string();
    assert_connection_arn("create my-conn", &arn);
    assert_eq!(
        created["Tags"],
        serde_json::json!([{ "Key": "env", "Value": "test" }]),
        "CreateConnection: expected the stored tags back, got {created}"
    );

    // GetConnection renders the stored row.
    let (status, got) = call(
        &router,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": arn }),
    )
    .await;
    assert_eq!(status, 200, "GetConnection: expected 200, got {status} {got}");
    assert_eq!(
        got["Connection"],
        serde_json::json!({
            "ConnectionName": "my-conn",
            "ConnectionArn": arn,
            "ProviderType": "GitHub",
            "OwnerAccountId": engine::emulator_account_id(),
            "ConnectionStatus": "PENDING",
            "OwnershipType": "Owned",
        }),
        "GetConnection: expected the created connection, got {got}"
    );

    // A read does not change what the next caller sees.
    let (_, again) = call(
        &router,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": arn }),
    )
    .await;
    assert_eq!(
        again, got,
        "GetConnection: a second read must report the same connection, got {again}"
    );

    // ListConnections with MaxResults=1 returns it and no token (nothing remains).
    let (status, listed) = call(
        &router,
        "ListConnections",
        serde_json::json!({ "MaxResults": 1 }),
    )
    .await;
    assert_eq!(status, 200, "ListConnections: expected 200, got {status} {listed}");
    assert_eq!(
        listed["Connections"],
        serde_json::json!([got["Connection"]]),
        "ListConnections: expected the one created connection, got {listed}"
    );
    assert!(
        listed.get("NextToken").is_none_or(serde_json::Value::is_null),
        "ListConnections: expected no NextToken when nothing remains, got {listed}"
    );

    // ListTagsForResource reports what CreateConnection stored.
    let (status, tags) = call(
        &router,
        "ListTagsForResource",
        serde_json::json!({ "ResourceArn": arn }),
    )
    .await;
    assert_eq!(status, 200, "ListTagsForResource: expected 200, got {status} {tags}");
    assert_eq!(
        tags["Tags"],
        serde_json::json!([{ "Key": "env", "Value": "test" }]),
        "ListTagsForResource: expected the tags supplied at create, got {tags}"
    );

    // DeleteConnection succeeds with an empty body.
    let (status, deleted) = call(
        &router,
        "DeleteConnection",
        serde_json::json!({ "ConnectionArn": arn }),
    )
    .await;
    assert_eq!(status, 200, "DeleteConnection: expected 200, got {status} {deleted}");
    assert!(
        deleted.as_object().is_some_and(serde_json::Map::is_empty) || deleted.is_null(),
        "DeleteConnection: expected an empty success body, got {deleted}"
    );

    // And then it is gone, everywhere.
    let (status, gone) = call(
        &router,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "GetConnection after delete: expected 400, got {status} {gone}"
    );
    assert!(
        gone["__type"]
            .as_str()
            .is_some_and(|t| t.ends_with("ResourceNotFoundException")),
        "GetConnection after delete: expected ResourceNotFoundException, got {gone}"
    );

    let (status, empty) = call(&router, "ListConnections", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListConnections after delete: expected 200, got {status} {empty}");
    assert_eq!(
        empty["Connections"],
        serde_json::json!([]),
        "ListConnections after delete: expected no connections, got {empty}"
    );
}

/// Every modeled `ProviderType` stores and reports as itself — one row per value the
/// enum declares, not just the one the issue cited.
#[tokio::test]
async fn every_modeled_provider_type_round_trips() {
    let router = router();

    for (index, provider) in PROVIDER_TYPES.iter().enumerate() {
        let name = format!("conn-{index}");
        let arn = create(&router, &name, provider).await;
        assert_connection_arn(provider, &arn);

        let (status, got) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {provider:?}: expected 200 from GetConnection, got {status} {got}"
        );
        assert_eq!(
            got["Connection"]["ProviderType"],
            serde_json::json!(provider),
            "case {provider:?}: expected the stored provider type, got {got}"
        );
        assert_eq!(
            got["Connection"]["ConnectionName"],
            serde_json::json!(name),
            "case {provider:?}: expected the stored name, got {got}"
        );
        // The shape documents that a connection created through the API/CLI/SDK is
        // PENDING by default; the console handshake is what moves it.
        assert_eq!(
            got["Connection"]["ConnectionStatus"],
            serde_json::json!("PENDING"),
            "case {provider:?}: expected ConnectionStatus PENDING, got {got}"
        );
    }

    // And a tagless create reports no tags against the resource.
    let arn = create(&router, "untagged", "GitHub").await;
    let (status, tags) = call(
        &router,
        "ListTagsForResource",
        serde_json::json!({ "ResourceArn": arn }),
    )
    .await;
    assert_eq!(status, 200, "untagged: expected 200, got {status} {tags}");
    assert_eq!(
        tags["Tags"],
        serde_json::json!([]),
        "untagged: expected an empty tag list, got {tags}"
    );
}

/// Paging covers every connection exactly once, for every page size: the invariant
/// `@paginated(inputToken=NextToken, outputToken=NextToken, pageSize=MaxResults)`
/// states. Rows differ in page size, including each published `MaxResults` bound
/// (0 and 100) and sizes above, below and equal to the collection size.
#[tokio::test]
async fn paging_covers_every_connection_exactly_once() {
    const CONNECTIONS: usize = 5;

    // `MaxResults` has range 0..100; 0 means "no caller-imposed page size".
    for max_results in [0_i64, 1, 2, 3, 5, 6, 100] {
        let router = router();
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for index in 0..CONNECTIONS {
            expected.insert(create(&router, &format!("conn-{index}"), "GitHub").await);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = serde_json::json!({ "MaxResults": max_results });
            if let Some(token) = &next_token {
                request["NextToken"] = serde_json::json!(token);
            }
            let (status, page) = call(&router, "ListConnections", request).await;
            assert_eq!(
                status, 200,
                "case MaxResults={max_results}: expected 200, got {status} {page}"
            );
            let items = page["Connections"]
                .as_array()
                .unwrap_or_else(|| panic!("case MaxResults={max_results}: no Connections: {page}"));
            if max_results > 0 {
                assert!(
                    items.len() as i64 <= max_results,
                    "case MaxResults={max_results}: page carried {} items, got {page}",
                    items.len()
                );
            }
            for item in items {
                seen.push(
                    item["ConnectionArn"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case MaxResults={max_results}: item has no ARN: {page}")
                        })
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= CONNECTIONS + 2,
                "case MaxResults={max_results}: paging did not terminate after {pages} pages"
            );
            match page.get("NextToken").and_then(serde_json::Value::as_str) {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case MaxResults={max_results}: paging returned a duplicate, got {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case MaxResults={max_results}: paging must cover every connection exactly once, got {seen:?}"
        );
        let expected_pages = if max_results > 0 {
            CONNECTIONS.div_ceil(max_results as usize)
        } else {
            1
        };
        assert_eq!(
            pages, expected_pages,
            "case MaxResults={max_results}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// `ProviderTypeFilter` and `HostArnFilter` select from the stored set, and a filter
/// composes with paging rather than replacing it.
#[tokio::test]
async fn list_filters_select_from_the_stored_set() {
    let router = router();

    // Two connections per provider type, so every filter row selects a known subset. The
    // name is built from the row's INDEX, not from the provider: the modelled
    // `ConnectionName` is `length 1..32`, which the longest provider value overruns.
    let mut by_provider: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for (index, provider) in PROVIDER_TYPES.iter().enumerate() {
        let mut arns = BTreeSet::new();
        for copy in 0..2 {
            arns.insert(create(&router, &format!("conn-{index}-{copy}"), provider).await);
        }
        by_provider.push((provider, arns));
    }

    for (provider, expected) in &by_provider {
        // Page size 1 as well, so the filter has to survive a resume.
        for max_results in [0_i64, 1] {
            let mut seen = BTreeSet::new();
            let mut next_token: Option<String> = None;
            loop {
                let mut request = serde_json::json!({
                    "MaxResults": max_results,
                    "ProviderTypeFilter": provider,
                });
                if let Some(token) = &next_token {
                    request["NextToken"] = serde_json::json!(token);
                }
                let (status, page) = call(&router, "ListConnections", request).await;
                assert_eq!(
                    status, 200,
                    "case {provider:?}/MaxResults={max_results}: expected 200, got {status} {page}"
                );
                for item in page["Connections"].as_array().unwrap_or_else(|| {
                    panic!("case {provider:?}: no Connections in {page}")
                }) {
                    assert_eq!(
                        item["ProviderType"],
                        serde_json::json!(provider),
                        "case {provider:?}: filter leaked another provider, got {page}"
                    );
                    seen.insert(item["ConnectionArn"].as_str().unwrap_or_default().to_string());
                }
                match page.get("NextToken").and_then(serde_json::Value::as_str) {
                    Some(token) => next_token = Some(token.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                &seen, expected,
                "case {provider:?}/MaxResults={max_results}: expected exactly that provider's connections, got {seen:?}"
            );
        }
    }

    // A host filter is RESOLVED against the host store: a host that IS there but has
    // no connections selects none …
    let (status, host) = call(
        &router,
        "CreateHost",
        serde_json::json!({
            "Name": "filter-host",
            "ProviderType": "GitHubEnterpriseServer",
            "ProviderEndpoint": "https://ghe.example.internal",
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateHost: expected 200, got {status} {host}");
    let host_arn = host["HostArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateHost returned no HostArn: {host}"))
        .to_string();
    let (status, filtered) = call(
        &router,
        "ListConnections",
        serde_json::json!({ "HostArnFilter": host_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "HostArnFilter: expected 200, got {status} {filtered}"
    );
    assert_eq!(
        filtered["Connections"],
        serde_json::json!([]),
        "HostArnFilter: no stored connection has a host, got {filtered}"
    );

    // … and a host filter naming a host the store does not hold is refused with the
    // modeled `ResourceNotFoundException`, not reported as an empty page.
    let (status, body) = call(
        &router,
        "ListConnections",
        serde_json::json!({ "HostArnFilter": some_host_arn() }),
    )
    .await;
    assert_eq!(
        status, 400,
        "HostArnFilter naming no host: expected 400, got {status} {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|kind| kind.ends_with("ResourceNotFoundException")),
        "HostArnFilter naming no host: expected ResourceNotFoundException, got {body}"
    );
}

/// An ARN no connection has is `ResourceNotFoundException` from every operation
/// that resolves one — one row per such operation and its ARN member.
#[tokio::test]
async fn an_unknown_arn_is_not_found_everywhere() {
    let router = router();
    // Created only so the store is non-empty: "not found" must not mean "store empty".
    let live = create(&router, "live", "GitLab").await;
    let missing = format!(
        "arn:aws:codeconnections:{REGION}:{}:connection/11111111-2222-3333-4444-555555555555",
        engine::emulator_account_id()
    );

    for (operation, member) in [
        ("GetConnection", "ConnectionArn"),
        ("ListTagsForResource", "ResourceArn"),
        ("DeleteConnection", "ConnectionArn"),
    ] {
        let (status, body) = call(
            &router,
            operation,
            serde_json::json!({ member: missing.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation}: expected 400 for an unknown ARN, got {status} {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|t| t.ends_with("ResourceNotFoundException")),
            "case {operation}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The live connection is untouched by the failed calls.
    let (status, got) = call(
        &router,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": live }),
    )
    .await;
    assert_eq!(status, 200, "the live connection must survive: {status} {got}");
}

/// Connection names are unique within the account, and a deleted name frees up again.
#[tokio::test]
async fn a_connection_name_is_unique_within_the_account() {
    let router = router();
    let first = create(&router, "shared-name", "GitHub").await;

    // Same name, any provider: still a name that is taken.
    for provider in PROVIDER_TYPES {
        let (status, body) = call(
            &router,
            "CreateConnection",
            serde_json::json!({ "ConnectionName": "shared-name", "ProviderType": provider }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {provider:?}: expected 400 for a duplicate name, got {status} {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|t| t.ends_with("ResourceAlreadyExistsException")),
            "case {provider:?}: expected ResourceAlreadyExistsException, got {body}"
        );
    }

    // A different name is accepted, so the refusal is about the name and nothing else.
    let other = create(&router, "other-name", "GitHub").await;
    assert_ne!(other, first, "distinct connections must have distinct ARNs");

    // Once deleted, the name is available — and the new connection gets a NEW ARN,
    // because a connection ARN is never reused.
    let (status, _) = call(
        &router,
        "DeleteConnection",
        serde_json::json!({ "ConnectionArn": first }),
    )
    .await;
    assert_eq!(status, 200, "DeleteConnection: expected 200, got {status}");
    let reused_name = create(&router, "shared-name", "Bitbucket").await;
    assert_ne!(
        reused_name, first,
        "case reuse: a recreated connection must not reuse the deleted ARN"
    );
}

/// A `HostArn` names a host resource. This suite mints no host, so the create is
/// refused with the modeled `ResourceNotFoundException` rather than storing
/// a connection that points at a resource that is not there.
#[tokio::test]
async fn a_host_arn_that_names_no_host_is_refused() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateConnection",
        serde_json::json!({
            "ConnectionName": "hosted",
            "ProviderType": "GitHubEnterpriseServer",
            "HostArn": some_host_arn(),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "HostArn: expected 400 for a host that does not exist, got {status} {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|t| t.ends_with("ResourceNotFoundException")),
        "HostArn: expected ResourceNotFoundException, got {body}"
    );

    // Nothing was persisted by the refused create.
    let (status, listed) = call(&router, "ListConnections", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListConnections: expected 200, got {status} {listed}");
    assert_eq!(
        listed["Connections"],
        serde_json::json!([]),
        "HostArn: a refused create must persist nothing, got {listed}"
    );
}

/// A `NextToken` this service did not issue names no position, so it is refused
/// instead of silently answered from the top of the list. A token is only accepted
/// when it decodes to a connection ARN of the kind this service mints, so a hex blob
/// that merely decodes to text is refused too.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = router();
    create(&router, "only", "GitLab").await;

    let hex_but_not_an_arn = hex::encode(b"arn:aws:s3:::some-bucket");
    let hex_but_plain_text = hex::encode(b"page-2");
    // Near-miss connection ARNs: an unpublished partition, and an empty region.
    let hex_bad_partition = hex::encode(
        format!(
            "arn:awsfoo:codeconnections:{REGION}:{}:connection/11111111-2222-3333-4444-555555555555",
            engine::emulator_account_id()
        )
        .as_bytes(),
    );
    let hex_empty_region = hex::encode(
        format!(
            "arn:aws:codeconnections::{}:connection/11111111-2222-3333-4444-555555555555",
            engine::emulator_account_id()
        )
        .as_bytes(),
    );
    for token in [
        "not-a-token",
        "zz",
        "0",
        "++++",
        hex_but_not_an_arn.as_str(),
        hex_but_plain_text.as_str(),
        hex_bad_partition.as_str(),
        hex_empty_region.as_str(),
    ] {
        let (status, body) = call(
            &router,
            "ListConnections",
            serde_json::json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400 for a token this service did not issue, got {status} {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|t| t.ends_with("ValidationException")),
            "case {token:?}: expected ValidationException, got {body}"
        );
    }
}

/// No single response renders the whole store: `ListConnections` clamps the page to
/// the modeled `MaxResults` maximum even when the caller asks for more or asks for
/// nothing, and reports the remainder through `NextToken`.
#[tokio::test]
async fn a_page_never_exceeds_the_server_side_maximum() {
    // Half a page more than the bound, so a clamped page must leave a remainder.
    let total = MAX_PAGE_SIZE + MAX_PAGE_SIZE / 2;
    let router = router();
    for index in 0..total {
        create(&router, &format!("conn-{index}"), "GitHub").await;
    }

    // Rows: no MaxResults at all (the modeled default of 0), and the published
    // maximum. Both must come back clamped, with the rest behind a token.
    for request in [
        serde_json::json!({}),
        serde_json::json!({ "MaxResults": MAX_PAGE_SIZE }),
    ] {
        let case = request.to_string();
        let (status, page) = call(&router, "ListConnections", request).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {page}");
        let items = page["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no Connections: {page}"));
        assert_eq!(
            items.len(),
            MAX_PAGE_SIZE,
            "case {case}: expected a page clamped to {MAX_PAGE_SIZE}, got {} items",
            items.len()
        );
        assert!(
            page.get("NextToken")
                .and_then(serde_json::Value::as_str)
                .is_some(),
            "case {case}: expected a NextToken for the remainder, got {page}"
        );
    }

    // And following the token still covers everything exactly once.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = serde_json::json!({});
        if let Some(token) = &next_token {
            request["NextToken"] = serde_json::json!(token);
        }
        let (status, page) = call(&router, "ListConnections", request).await;
        assert_eq!(status, 200, "clamped walk: expected 200, got {status} {page}");
        for item in page["Connections"].as_array().unwrap_or(&Vec::new()) {
            seen.insert(item["ConnectionArn"].as_str().unwrap_or_default().to_string());
        }
        match page.get("NextToken").and_then(serde_json::Value::as_str) {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen.len(),
        total,
        "clamped walk: expected all {total} connections across pages, got {}",
        seen.len()
    );
}

/// The store is bounded: past `MAX_CONNECTIONS` a create is refused with the modeled
/// `LimitExceededException`, and deleting one frees the slot again.
#[tokio::test]
async fn creating_past_the_store_bound_is_refused() {
    let router = router();
    let mut first_arn = String::new();
    for index in 0..MAX_CONNECTIONS {
        let arn = create(&router, &format!("conn-{index}"), "GitHub").await;
        if index == 0 {
            first_arn = arn;
        }
    }

    let (status, body) = call(
        &router,
        "CreateConnection",
        serde_json::json!({ "ConnectionName": "one-too-many", "ProviderType": "GitHub" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case at-bound: expected 400 past {MAX_CONNECTIONS} connections, got {status} {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|t| t.ends_with("LimitExceededException")),
        "case at-bound: expected LimitExceededException, got {body}"
    );

    // At the bound, a name that is already taken is still reported as the specific
    // client-input failure: the request is invalid for its own reason, not merely
    // because the store is full.
    let (status, body) = call(
        &router,
        "CreateConnection",
        serde_json::json!({ "ConnectionName": "conn-0", "ProviderType": "GitLab" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case at-bound duplicate name: expected 400, got {status} {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|t| t.ends_with("ResourceAlreadyExistsException")),
        "case at-bound duplicate name: expected ResourceAlreadyExistsException, got {body}"
    );

    // Freeing a slot makes room again, so the bound is on what is stored and not a
    // one-way latch.
    let (status, _) = call(
        &router,
        "DeleteConnection",
        serde_json::json!({ "ConnectionArn": first_arn }),
    )
    .await;
    assert_eq!(status, 200, "DeleteConnection: expected 200, got {status}");
    create(&router, "one-too-many", "GitHub").await;
}

/// State is shared: a connection created by one request is visible to the next, and
/// two routers over two engines do not see each other's connections.
#[tokio::test]
async fn connections_live_in_the_shared_engine() {
    let first = router();
    let second = router();
    let arn = create(&first, "in-first", "GitHub").await;

    let (status, got) = call(
        &first,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": arn }),
    )
    .await;
    assert_eq!(status, 200, "the first emulator must hold it: {status} {got}");

    let (status, listed) = call(&second, "ListConnections", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListConnections: expected 200, got {status} {listed}");
    assert_eq!(
        listed["Connections"],
        serde_json::json!([]),
        "a separate emulator must not see another's connections, got {listed}"
    );
}

/// A minted ARN is regional in the region the caller signed for; a credential scope
/// whose region field is not shaped like a region code falls back to the default
/// instead of being spliced into the ARN. The minter and the page-token reader apply
/// one rule, so the service never refuses a token it issued itself.
#[tokio::test]
async fn the_arn_region_comes_from_a_region_shaped_credential_scope() {
    // Rows: the region the ARN must carry, and the scope that should produce it.
    for (scope_region, expected_region) in [
        ("us-east-1", "us-east-1"),
        ("eu-west-1", "eu-west-1"),
        ("cn-north-1", "cn-north-1"),
        ("ap-southeast-4", "ap-southeast-4"),
        // Not region codes: uppercase, underscores, and a value carrying the ARN
        // field separator. Each falls back to the emulator default.
        ("EU-WEST-1", "us-east-1"),
        ("eu_west_1", "us-east-1"),
        ("eu:west:1", "us-east-1"),
    ] {
        let router = router();
        let (status, created) = call_signed_for(
            &router,
            scope_region,
            "CreateConnection",
            serde_json::json!({ "ConnectionName": "scoped", "ProviderType": "GitHub" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {scope_region:?}: expected 200, got {status} {created}"
        );
        let arn = created["ConnectionArn"].as_str().unwrap_or_default();
        let region_field = arn.split(':').nth(3).unwrap_or_default();
        assert_eq!(
            region_field, expected_region,
            "case {scope_region:?}: expected the ARN region {expected_region:?}, got {arn:?}"
        );

        // And the connection still pages: the token this service issues for an ARN
        // it minted is one it accepts back.
        let (status, page) = call_signed_for(
            &router,
            scope_region,
            "ListConnections",
            serde_json::json!({ "MaxResults": 1 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {scope_region:?}: expected 200 from ListConnections, got {status} {page}"
        );
        assert_eq!(
            page["Connections"]
                .as_array()
                .map(std::vec::Vec::len)
                .unwrap_or_default(),
            1,
            "case {scope_region:?}: expected the one connection, got {page}"
        );
    }
}
