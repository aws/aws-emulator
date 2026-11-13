// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreatePublicDnsNamespace`.
//!
//! The rule under test: the call persists a `DNS_PUBLIC` namespace built from the
//! request — its `Name`, `Description`, `Tags` and `CreatorRequestId` — under a minted
//! namespace id, ARN and Route 53 hosted zone id, carrying the SOA TTL the request
//! stated, and answers with the `OperationId` of the `CREATE_NAMESPACE` operation it
//! filed; every later read of that namespace is computed from that record.
//!
//! Requests go over the wire through `MultiServiceRouter::dispatch`, with the harness
//! `cloud_map_core` already owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{call, discovered, emulator, failure, ok, REGION};

/// `CreatePublicDnsNamespace`, returning the `OperationId` it answers with.
async fn create(router: &MultiServiceRouter, body: Value) -> String {
    let created = ok(router, "CreatePublicDnsNamespace", body).await;
    created["OperationId"]
        .as_str()
        .expect("CreatePublicDnsNamespace returns an OperationId")
        .to_string()
}

/// The namespace id the `CREATE_NAMESPACE` operation `operation_id` targets.
async fn created_namespace_id(router: &MultiServiceRouter, operation_id: &str) -> String {
    let operation = ok(
        router,
        "GetOperation",
        json!({ "OperationId": operation_id }),
    )
    .await;
    assert_eq!(
        operation["Operation"]["Type"],
        json!("CREATE_NAMESPACE"),
        "the filed operation is a CREATE_NAMESPACE, got {operation}"
    );
    operation["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets the namespace it created")
        .to_string()
}

/// `GetNamespace`'s view of one namespace.
async fn read(router: &MultiServiceRouter, reference: &str) -> Value {
    ok(router, "GetNamespace", json!({ "Id": reference })).await["Namespace"].clone()
}

/// Whether `candidate` has the shape of a Route 53 hosted zone id: `Z` followed by
/// uppercase alphanumerics.
fn is_hosted_zone_id(candidate: &str) -> bool {
    candidate.len() > 1
        && candidate.starts_with('Z')
        && candidate[1..]
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

/// Create a public DNS namespace over a table of requests and read each one back: the
/// stored namespace reports what the request stated, plus the ids the service mints.
///
/// The SOA TTL rows span the published `RecordTTL` range (`smithy_query(query="shape",
/// shape="RecordTTL")`: 0..=2147483647), including both bounds, and the rows that state
/// no `Properties` at all — where the namespace holds no SOA TTL of its own and so
/// reports the zero of "no negative caching" on the `SOA` every namespace read carries.
#[tokio::test]
async fn a_created_public_dns_namespace_reports_back_what_was_stored() {
    let router = emulator();

    // (case, the request members beside Name, expected Description, expected SOA TTL)
    let cases: Vec<(&str, Value, Option<&str>, i64)> = vec![
        ("only the required name", json!({}), None, 0),
        (
            "a description and no properties",
            json!({ "Description": "the storefront namespace" }),
            Some("the storefront namespace"),
            0,
        ),
        (
            "the low bound of the published TTL range",
            json!({ "Properties": { "DnsProperties": { "SOA": { "TTL": 0 } } } }),
            None,
            0,
        ),
        (
            "a TTL inside the range",
            json!({
                "Description": "fifteen second negative caching",
                "Properties": { "DnsProperties": { "SOA": { "TTL": 15 } } },
            }),
            Some("fifteen second negative caching"),
            15,
        ),
        (
            "the high bound of the published TTL range",
            json!({ "Properties": { "DnsProperties": { "SOA": { "TTL": 2147483647_i64 } } } }),
            None,
            2147483647,
        ),
        (
            "tags and a creator request id",
            json!({
                "CreatorRequestId": "token-tagged",
                "Tags": [{ "Key": "team", "Value": "platform" }],
                "Properties": { "DnsProperties": { "SOA": { "TTL": 300 } } },
            }),
            None,
            300,
        ),
    ];

    let mut hosted_zone_ids = std::collections::BTreeSet::new();
    for (index, (case, extra, description, soa_ttl)) in cases.into_iter().enumerate() {
        let name = format!("row{index}.example.com");
        let mut body = json!({ "Name": name });
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key.as_str()] = value;
            }
        }

        let operation_id = create(&router, body.clone()).await;
        let namespace_id = created_namespace_id(&router, &operation_id).await;
        let namespace = read(&router, &namespace_id).await;

        assert_eq!(
            namespace["Name"],
            json!(name),
            "case {case:?}: the stored namespace keeps the requested name, got {namespace}"
        );
        assert_eq!(
            namespace["Type"],
            json!("DNS_PUBLIC"),
            "case {case:?}: a public DNS namespace is DNS_PUBLIC, got {namespace}"
        );
        assert_eq!(
            namespace["Description"],
            description.map_or(Value::Null, |text| json!(text)),
            "case {case:?}: expected Description {description:?}, got {namespace}"
        );
        assert_eq!(
            namespace["ServiceCount"],
            json!(0),
            "case {case:?}: a new namespace holds no services, got {namespace}"
        );
        assert_eq!(
            namespace["CreatorRequestId"],
            body.get("CreatorRequestId").cloned().unwrap_or(Value::Null),
            "case {case:?}: the stored CreatorRequestId is reported back, got {namespace}"
        );

        // The ARN is minted for the stored id and carries the signed region.
        let arn = namespace["Arn"].as_str().unwrap_or_default().to_string();
        assert!(
            arn.ends_with(&format!("namespace/{namespace_id}")) && arn.contains(REGION),
            "case {case:?}: expected an ARN for namespace/{namespace_id} in {REGION}, got {arn:?}"
        );

        // `Namespace$Type`: instances in a DNS_PUBLIC namespace are discoverable by
        // public DNS queries AND by DiscoverInstances, so the namespace carries both
        // the hosted zone Cloud Map created for it and its HTTP name.
        let hosted_zone_id = namespace["Properties"]["DnsProperties"]["HostedZoneId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_hosted_zone_id(&hosted_zone_id),
            "case {case:?}: expected a Route 53 hosted zone id, got {hosted_zone_id:?}"
        );
        assert!(
            hosted_zone_ids.insert(hosted_zone_id.clone()),
            "case {case:?}: hosted zone {hosted_zone_id:?} was already minted for another namespace"
        );
        assert_eq!(
            namespace["Properties"]["HttpProperties"]["HttpName"],
            json!(name),
            "case {case:?}: the namespace is discoverable under its name, got {namespace}"
        );
        assert_eq!(
            namespace["Properties"]["DnsProperties"]["SOA"]["TTL"],
            json!(soa_ttl),
            "case {case:?}: expected SOA TTL {soa_ttl:?}, got {namespace}"
        );

        // The same record answers a read that names the namespace by its ARN.
        let by_arn = read(&router, &arn).await;
        assert_eq!(
            by_arn, namespace,
            "case {case:?}: reading by ARN must reach the one stored record, got {by_arn}"
        );
    }
}

/// A SOA TTL outside the published `RecordTTL` range (0..=2147483647) is refused before
/// the handler runs, so nothing is stored for it.
#[tokio::test]
async fn a_soa_ttl_outside_the_published_range_is_refused() {
    let router = emulator();

    for (case, ttl) in [
        ("one below the low bound", -1_i64),
        ("one above the high bound", 2147483648),
    ] {
        let name = "out-of-range.example.com";
        let (status, json) = call(
            &router,
            "CreatePublicDnsNamespace",
            json!({
                "Name": name,
                "Properties": { "DnsProperties": { "SOA": { "TTL": ttl } } },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} TTL={ttl}: a value outside the declared range is refused, got {json}"
        );
    }

    // Nothing was stored for either refusal: the name index is still free, so a valid
    // create of that same name succeeds and stores the TTL it states.
    let operation_id = create(
        &router,
        json!({
            "Name": "out-of-range.example.com",
            "Properties": { "DnsProperties": { "SOA": { "TTL": 30 } } },
        }),
    )
    .await;
    let namespace = read(&router, &created_namespace_id(&router, &operation_id).await).await;
    assert_eq!(
        namespace["Properties"]["DnsProperties"]["SOA"]["TTL"],
        json!(30),
        "the refused requests took neither the name nor a TTL, got {namespace}"
    );
}

/// The name index holds every namespace type, so a name already taken is refused
/// whichever create took it — and a name owned by a create that has COMPLETED is the
/// request conflict `DuplicateRequest` naming that create, not a validation
/// failure; a replayed `CreatorRequestId` replays the first operation instead of
/// creating a second namespace.
#[tokio::test]
async fn a_taken_name_is_refused_and_a_replayed_token_replays_its_operation() {
    let router = emulator();

    // (case, the operation that takes the name first, the name)
    for (case, first, name) in [
        (
            "taken by another public DNS namespace",
            "CreatePublicDnsNamespace",
            "taken-public.example.com",
        ),
        (
            "taken by an HTTP namespace",
            "CreateHttpNamespace",
            "taken-http.example.com",
        ),
    ] {
        let taken = ok(&router, first, json!({ "Name": name })).await;
        assert!(
            taken["OperationId"].is_string(),
            "case {case:?}: {first} must file an operation, got {taken}"
        );
        let (status, code) = failure(
            &router,
            "CreatePublicDnsNamespace",
            json!({ "Name": name }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "DuplicateRequest"),
            "case {case:?}: a second namespace named {name:?} must be refused"
        );
    }

    // A retry of the SAME request replays the first operation; the same token naming
    // another namespace is a different request, so it creates one.
    let replay_body = json!({ "Name": "replayed.example.com", "CreatorRequestId": "token-1" });
    let first = create(&router, replay_body.clone()).await;
    let again = create(&router, replay_body).await;
    assert_eq!(
        first, again,
        "a replayed CreatorRequestId must replay its operation, got {first} then {again}"
    );
    let other = create(
        &router,
        json!({ "Name": "other.example.com", "CreatorRequestId": "token-1" }),
    )
    .await;
    assert_ne!(
        first, other,
        "the same token naming another namespace is not a replay, got {other}"
    );

    let stored_replay = read(&router, &created_namespace_id(&router, &first).await).await;
    let stored_other = read(&router, &created_namespace_id(&router, &other).await).await;
    assert_eq!(
        stored_replay["Name"],
        json!("replayed.example.com"),
        "the replayed operation still targets the namespace the first call stored, got {stored_replay}"
    );
    assert_eq!(
        stored_other["Name"],
        json!("other.example.com"),
        "the token's second request stored its own namespace, got {stored_other}"
    );
    assert_ne!(
        stored_replay["Id"], stored_other["Id"],
        "two namespaces must not share an id: {stored_replay} and {stored_other}"
    );
}

/// Once the namespace a token created is deleted, the token is stale: replaying it
/// stores a NEW namespace instead of answering success for a create this call never
/// made.
#[tokio::test]
async fn a_token_is_not_replayed_once_its_namespace_is_deleted() {
    let router = emulator();
    let body = json!({ "Name": "recreated.example.com", "CreatorRequestId": "token-recreate" });

    let first = create(&router, body.clone()).await;
    let first_namespace = created_namespace_id(&router, &first).await;
    ok(
        &router,
        "DeleteNamespace",
        json!({ "Id": first_namespace.clone() }),
    )
    .await;
    let (status, code) = failure(&router, "GetNamespace", json!({ "Id": first_namespace.clone() })).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NamespaceNotFound"),
        "the deleted namespace must stop resolving"
    );

    let again = create(&router, body).await;
    assert_ne!(
        first, again,
        "a stale token must not replay its operation, got {first} twice"
    );
    let recreated = created_namespace_id(&router, &again).await;
    assert_ne!(
        first_namespace, recreated,
        "the replayed request must store a NEW namespace, got {recreated}"
    );
    let namespace = read(&router, &recreated).await;
    assert_eq!(
        namespace["Name"],
        json!("recreated.example.com"),
        "the re-created namespace reads back from the store, got {namespace}"
    );
    assert_eq!(
        namespace["Type"],
        json!("DNS_PUBLIC"),
        "the re-created namespace is a public DNS namespace, got {namespace}"
    );
}

/// A service created in a public DNS namespace registers DNS records (`Service$Type`
/// `DNS_HTTP`) and its instances are discoverable through `DiscoverInstances`, which
/// resolves the namespace by the HTTP name the create stored.
#[tokio::test]
async fn instances_in_a_public_dns_namespace_are_discoverable() {
    let router = emulator();
    let name = "discovery.example.com";
    let operation_id = create(
        &router,
        json!({
            "Name": name,
            "Properties": { "DnsProperties": { "SOA": { "TTL": 60 } } },
        }),
    )
    .await;
    let namespace_id = created_namespace_id(&router, &operation_id).await;

    let created = ok(
        &router,
        "CreateService",
        json!({
            "Name": "backend",
            "NamespaceId": namespace_id,
            "DnsConfig": { "DnsRecords": [{ "Type": "A", "TTL": 60 }] },
        }),
    )
    .await;
    assert_eq!(
        created["Service"]["Type"],
        json!("DNS_HTTP"),
        "a service with DNS records in a DNS namespace is DNS_HTTP, got {created}"
    );
    let service_id = created["Service"]["Id"]
        .as_str()
        .expect("CreateService returns the service it stored")
        .to_string();

    ok(
        &router,
        "RegisterInstance",
        json!({
            "ServiceId": service_id,
            "InstanceId": "i-public-1",
            "Attributes": { "AWS_INSTANCE_IPV4": "10.1.2.3" },
        }),
    )
    .await;

    let found = discovered(
        &router,
        json!({ "NamespaceName": name, "ServiceName": "backend" }),
    )
    .await;
    assert!(
        found.contains("i-public-1"),
        "the instance registered in {name:?} must be discoverable, got {found:?}"
    );

    // The namespace now reports the service the create added to it.
    let namespace = read(&router, &namespace_id).await;
    assert_eq!(
        namespace["ServiceCount"],
        json!(1),
        "ServiceCount is derived from the services stored in the namespace, got {namespace}"
    );
    assert_eq!(
        namespace["Properties"]["DnsProperties"]["SOA"]["TTL"],
        json!(60),
        "the stored SOA TTL survives later writes to the namespace, got {namespace}"
    );
}
