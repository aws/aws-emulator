// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateCustomKeyStore`, driven over the wire through the
//! shared [`super::harness`].
//!
//! The rule under test: `CreateCustomKeyStore` persists ONE custom key store built
//! from the request — the CloudHSM cluster, its trust anchor certificate and its
//! kmsuser password for a key store whose `CustomKeyStoreType` is `AWS_CLOUDHSM`
//! (the default) — mints its id, leaves it `DISCONNECTED` because it "has never been
//! connected", and answers that id, so every later reader of the family serves the
//! record this call left behind. A request that cannot produce a key store persists
//! NOTHING and is refused: a property the asked-for type does not carry, or one that
//! belongs to the other type, is an invalid request; a CloudHSM cluster this account
//! does not hold, one that is not active, one whose subnets do not span two
//! Availability Zones and one that already backs a key store of the account are each
//! that cluster's own modelled refusal; a friendly name another key store of the
//! account holds, and a proxy endpoint, proxy URI or Amazon VPC endpoint service
//! another external key store of the Region holds, are each that collision's own
//! modelled refusal; and an `EXTERNAL_KEY_STORE` request whose proxy cannot be reached
//! is `XksProxyUriUnreachableException` — which is every such request here, since no
//! external key store proxy runs in this process (the rule
//! [`super::external_key_store_proxy`] is about).
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in whether the
//! type is named or left to its default, in the friendly name (the shortest and the
//! longest the modelled length admits), in the Region the key store belongs to, in
//! which required property is withheld, and in which already-taken value the request
//! reuses. The external key stores a collision row needs are SEEDED into the store,
//! the way [`super::custom_key_stores`] seeds the records it reads, because no create
//! can make one.
//!
//! A CloudHSM cluster is NOT a KMS resource: the clusters an account holds are the
//! CloudHSM service's own records, so the rows that need one create it through
//! CloudHSM's own `CreateCluster` with that service mounted beside this one over the
//! same engine. `InitializeCluster` — the operation that takes a cluster to `ACTIVE`
//! — is still a scaffold stub with its own issue, so a row that needs an ACTIVE
//! cluster activates it through the CloudHSM store's own accessor, the way
//! initializing it would.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kms_trentservice::state::{
    mint_custom_key_store_id, service_state, CustomKeyStore, CustomKeyStoreBacking, XksProxy,
    CUSTOM_KEY_STORE_CONNECTION_SPAN,
};

use super::harness::{call, error_code, error_message, is_lower_hex, ok, ok_in, REGION};

/// The Region a row belongs to when the row is about regional scoping.
const OTHER_REGION: &str = "eu-west-2";

/// The shortest and longest friendly name `CustomKeyStoreNameType` admits
/// (`smithy_query(query="shape",shape="CustomKeyStoreNameType")` — `@length(1..256)`).
const MIN_NAME_LEN: usize = 1;
/// See [`MIN_NAME_LEN`].
const MAX_NAME_LEN: usize = 256;

/// The `ClusterState` of a CloudHSM cluster that has finished initializing and is
/// serving, from `smithy_query(query="shape",shape="ClusterState",
/// model_path="services/service-com-amazonaws-cloudhsmv2-baldrapiservice/smithy-model/com-amazonaws-cloudhsmv2-baldrapiservice.json")`.
const CLUSTER_STATE_ACTIVE: &str = "ACTIVE";

/// Every `ClusterState` other than [`CLUSTER_STATE_ACTIVE`] that a cluster of the
/// account can be in while still being a cluster an operation acts on, from the same
/// enum as [`CLUSTER_STATE_ACTIVE`]. `DELETE_IN_PROGRESS` and `DELETED` are left out:
/// a cluster being deleted or deleted is gone, so its id names no cluster at all.
const INACTIVE_CLUSTER_STATES: [&str; 8] = [
    "CREATE_IN_PROGRESS",
    "UNINITIALIZED",
    "INITIALIZE_IN_PROGRESS",
    "INITIALIZED",
    "UPDATE_IN_PROGRESS",
    "MODIFY_IN_PROGRESS",
    "ROLLBACK_IN_PROGRESS",
    "DEGRADED",
];

/// The whole `message` KMS answers with `CloudHsmClusterNotActiveException`: fixed
/// text for the class, into which no cluster id, key store name or limit is
/// interpolated, so it is the same string for every cluster that is not serving.
const CLUSTER_NOT_ACTIVE_MESSAGE: &str = "Cluster is not active.";

/// A trust anchor certificate, as a caller sends the content of its `customerCA.crt`
/// file. `TrustAnchorCertificateType` is modelled `@length(1..5000)` with no pattern.
const TRUST_ANCHOR: &str = "-----BEGIN CERTIFICATE-----\nY3JlYXRl\n-----END CERTIFICATE-----";

/// The kmsuser password, inside `KeyStorePasswordType`'s modelled `@length(7..32)`.
const KEY_STORE_PASSWORD: &str = "kmsuser-password";

/// An `XksProxyAuthenticationCredential` access key id: 20..30 characters of
/// `^[A-Z2-7]+$` (`smithy_query(query="shape",shape="XksProxyAuthenticationCredentialType")`).
const PROXY_ACCESS_KEY_ID: &str = "ABCDEFGHIJKLMNOPQRST";

/// Its raw secret access key: 43..64 characters of `^[a-zA-Z0-9\/+=]+$` (the same
/// shape). A credential this emulator never reports back.
const PROXY_SECRET: &str = "c2VjcmV0L3Byb3h5K2NyZWRlbnRpYWw9PTAxMjM0NTY3ODk=";

/// Long enough past [`CUSTOM_KEY_STORE_CONNECTION_SPAN`] that a connection the call
/// started has certainly fallen due.
fn past_the_span() -> Duration {
    CUSTOM_KEY_STORE_CONNECTION_SPAN + Duration::from_millis(250)
}

/// A router carrying this service, the CloudHSM service and Amazon EC2 over ONE shared
/// engine — the assembly a row that names a CloudHSM cluster needs, since the clusters of
/// the account are CloudHSM's own state and the subnets such a cluster is placed in are
/// EC2's.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = super::harness::router_with_engine();
    router.register_service(
        service_com_amazonaws_cloudhsmv2_baldrapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// The EC2 API version an ec2Query body names.
const EC2_API_VERSION: &str = "2016-11-15";

/// The Availability Zones of [`REGION`] a cluster's subnets are placed in, one per subnet:
/// CloudHSM "can specify only one subnet per Availability Zone"
/// (`CreateCluster$SubnetIds`).
const ZONE_LETTERS: [char; 3] = ['a', 'b', 'c'];

/// One ec2Query action that must succeed, answered with its XML body.
async fn ec2(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={value}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEMULATOR/20260101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case}: ec2:{action} must answer HTTP 200, got {status} with {text}"
    );
    text
}

/// The text of the first `<name>` element of `xml`.
fn element(xml: &str, name: &str, case: &str) -> String {
    let opened = xml
        .split_once(&format!("<{name}>"))
        .unwrap_or_else(|| panic!("case {case}: no <{name}> in {xml}"))
        .1;
    opened
        .split_once(&format!("</{name}>"))
        .unwrap_or_else(|| panic!("case {case}: no </{name}> in {xml}"))
        .0
        .to_string()
}

/// `subnets` subnets of this account, each in its own Availability Zone of [`REGION`],
/// minted by Amazon EC2 itself through its own operations — the only subnet ids a CloudHSM
/// cluster of this account can be placed in.
async fn ec2_subnets(router: &MultiServiceRouter, case: &str, subnets: usize) -> Vec<String> {
    assert!(
        subnets <= ZONE_LETTERS.len(),
        "case {case}: a cluster takes one subnet per Availability Zone, so at most {}",
        ZONE_LETTERS.len()
    );
    let vpc = element(
        &ec2(router, case, "CreateVpc", &[("CidrBlock", "10.70.0.0%2F16")]).await,
        "vpcId",
        case,
    );
    let mut minted = Vec::with_capacity(subnets);
    for (index, letter) in ZONE_LETTERS.iter().take(subnets).enumerate() {
        let cidr = format!("10.70.{index}.0%2F24");
        let zone = format!("{REGION}{letter}");
        minted.push(element(
            &ec2(
                router,
                case,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", cidr.as_str()),
                    ("AvailabilityZone", zone.as_str()),
                ],
            )
            .await,
            "subnetId",
            case,
        ));
    }
    minted
}

/// Create a CloudHSM cluster of this account over `subnets` private subnets —
/// CloudHSM places one Availability Zone per subnet — through CloudHSM's OWN
/// `CreateCluster`, and answer its cluster id.
async fn cloud_hsm_cluster(router: &MultiServiceRouter, case: &str, subnets: usize) -> String {
    let subnet_ids = ec2_subnets(router, case, subnets).await;
    let body = json!({"HsmType": "hsm2m.medium", "SubnetIds": subnet_ids});
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cloudhsmv2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "BaldrApiService.CreateCluster")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEMULATOR/20260101/{REGION}/cloudhsm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case}: cloudhsm:CreateCluster must answer HTTP 200, got {status} with {payload}"
    );
    payload["Cluster"]["ClusterId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a ClusterId in {payload}"))
        .to_string()
}

/// Put the cluster `cluster_id` into `cluster_state` in the CloudHSM store, settled,
/// so the cluster is one a key store request is judged against in that state.
fn put_cluster_state(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    cluster_id: &str,
    cluster_state: &str,
) {
    let state = service_com_amazonaws_cloudhsmv2_baldrapiservice::state::service_state(&**engine)
        .expect("the CloudHSM state store is reachable");
    let mut state = state.lock();
    let cluster = state
        .clusters
        .get_mut(cluster_id)
        .unwrap_or_else(|| panic!("case {case}: the CloudHSM store holds {cluster_id}"));
    cluster.state = cluster_state.to_string();
    cluster.state_message = None;
    cluster.pending = None;
}

/// Take the cluster `cluster_id` to [`CLUSTER_STATE_ACTIVE`] in the CloudHSM store —
/// what `InitializeCluster`, still a scaffold stub with its own issue, would do — so
/// the cluster is one that can back a CloudHSM key store.
fn activate(engine: &Arc<engine::InMemoryEngine>, case: &str, cluster_id: &str) {
    put_cluster_state(engine, case, cluster_id, CLUSTER_STATE_ACTIVE);
}

/// A CloudHSM cluster of the account that is active and spans two Availability
/// Zones: a cluster that can back a CloudHSM key store.
async fn active_cluster(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
) -> String {
    let cluster_id = cloud_hsm_cluster(router, case, 2).await;
    activate(engine, case, &cluster_id);
    cluster_id
}

/// Seed an EXTERNAL key store over `uri_endpoint` and `uri_path` directly into the
/// store, which is how a case gets one to collide with: no external key store proxy is
/// reachable from this emulator, so `CreateCustomKeyStore` creates no external key store
/// (the rule the unreachable-proxy test is about) and a record that has to be THERE for
/// a uniqueness refusal is seeded the way [`super::custom_key_stores`] seeds the records
/// it reads.
fn seed_external_key_store(
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    name: &str,
    connectivity: &str,
    uri_endpoint: &str,
    uri_path: &str,
    vpc_endpoint_service_name: Option<&str>,
) {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    let store = CustomKeyStore::new(
        mint_custom_key_store_id().expect("entropy for a custom key store id"),
        region.to_string(),
        name.to_string(),
        CustomKeyStoreBacking::External(XksProxy {
            connectivity: connectivity.to_string(),
            access_key_id: PROXY_ACCESS_KEY_ID.to_string(),
            uri_endpoint: uri_endpoint.to_string(),
            uri_path: uri_path.to_string(),
            vpc_endpoint_service_name: vpc_endpoint_service_name.map(str::to_string),
            vpc_endpoint_service_owner: vpc_endpoint_service_name
                .map(|_| engine::emulator_account_id().to_string()),
        }),
    );
    assert!(
        state.insert_custom_key_store(store),
        "case {name}: the seeded external key store must be persisted"
    );
}

/// A `CreateCustomKeyStore` request for a CloudHSM key store over `cluster_id`.
fn cloud_hsm_request(name: &str, cluster_id: &str) -> Value {
    json!({
        "CustomKeyStoreName": name,
        "CustomKeyStoreType": "AWS_CLOUDHSM",
        "CloudHsmClusterId": cluster_id,
        "KeyStorePassword": KEY_STORE_PASSWORD,
        "TrustAnchorCertificate": TRUST_ANCHOR,
    })
}

/// A `CreateCustomKeyStore` request for an external key store on a public endpoint.
fn public_endpoint_request(name: &str, uri_endpoint: &str, uri_path: &str) -> Value {
    json!({
        "CustomKeyStoreName": name,
        "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
        "XksProxyConnectivity": "PUBLIC_ENDPOINT",
        "XksProxyUriEndpoint": uri_endpoint,
        "XksProxyUriPath": uri_path,
        "XksProxyAuthenticationCredential": {
            "AccessKeyId": PROXY_ACCESS_KEY_ID,
            "RawSecretAccessKey": PROXY_SECRET,
        },
    })
}

/// A `CreateCustomKeyStore` request for an external key store behind a VPC endpoint
/// service.
fn vpc_endpoint_request(
    name: &str,
    uri_endpoint: &str,
    uri_path: &str,
    service_name: &str,
) -> Value {
    json!({
        "CustomKeyStoreName": name,
        "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
        "XksProxyConnectivity": "VPC_ENDPOINT_SERVICE",
        "XksProxyUriEndpoint": uri_endpoint,
        "XksProxyUriPath": uri_path,
        "XksProxyVpcEndpointServiceName": service_name,
        "XksProxyAuthenticationCredential": {
            "AccessKeyId": PROXY_ACCESS_KEY_ID,
            "RawSecretAccessKey": PROXY_SECRET,
        },
    })
}

/// Create a custom key store in `region`, requiring success, and answer the id it
/// minted — checked to have the form this service mints: the `cks-` prefix and 16
/// lowercase hexadecimal characters, inside `CustomKeyStoreIdType`'s `@length(1..64)`.
async fn created_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    request: &Value,
) -> String {
    let answer = ok_in(router, region, "CreateCustomKeyStore", request).await;
    let id = answer["CustomKeyStoreId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a CustomKeyStoreId in {answer}"))
        .to_string();
    assert_eq!(
        answer.as_object().map(serde_json::Map::len),
        Some(1),
        "case {case}: the answer carries the new key store's id alone, got {answer}"
    );
    let body = id.strip_prefix("cks-").unwrap_or_default();
    assert!(
        body.len() == 16 && is_lower_hex(body) && id.len() <= 64,
        "case {case}: expected a minted custom key store id, got {id:?}"
    );
    id
}

/// [`created_in`], in [`REGION`].
async fn created(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    created_in(router, REGION, case, request).await
}

/// The one entry `DescribeCustomKeyStores` reports for `custom_key_store_id` in
/// `region`, without its `CreationDate` (which the clock, not the row, decides) —
/// having checked the entry carries one.
async fn entry_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    custom_key_store_id: &str,
) -> Value {
    let answer = ok_in(
        router,
        region,
        "DescribeCustomKeyStores",
        &json!({"CustomKeyStoreId": custom_key_store_id}),
    )
    .await;
    let entries = answer["CustomKeyStores"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a CustomKeyStores list in {answer}"))
        .clone();
    assert_eq!(
        entries.len(),
        1,
        "case {case}: the created key store is reported exactly once, got {entries:?}"
    );
    let mut members = entries[0]
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a key store entry, got {}", entries[0]))
        .clone();
    let creation_date = members.remove("CreationDate").unwrap_or(Value::Null);
    assert!(
        creation_date.as_f64().is_some_and(|seconds| seconds > 0.0),
        "case {case}: expected a CreationDate timestamp, got {}",
        entries[0]
    );
    Value::Object(members)
}

/// [`entry_in`], in [`REGION`].
async fn entry(router: &MultiServiceRouter, case: &str, custom_key_store_id: &str) -> Value {
    entry_in(router, REGION, case, custom_key_store_id).await
}

/// How many custom key stores the store holds, over every Region — what a refusal
/// must leave exactly as it was.
fn held(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let held = state.lock();
    held.custom_key_stores.len()
}

/// One row of the success table: the request, and the entry the reader must report
/// for the key store it created.
struct Row {
    case: &'static str,
    request: Value,
    expected: Value,
}

/// A created custom key store is the record the request describes: the reader reports
/// exactly the members a key store of that type has, it has never been connected, and
/// the whole list is what the creates left, in creation order.
#[tokio::test]
async fn create_custom_key_store_persists_the_key_store_the_request_describes() {
    let (router, engine) = emulator();
    let named_cluster = active_cluster(&router, &engine, "a CloudHSM key store").await;
    let default_cluster = active_cluster(&router, &engine, "a CloudHSM key store by default").await;
    let longest_name_cluster = active_cluster(&router, &engine, "the longest friendly name").await;
    let elsewhere_cluster = active_cluster(&router, &engine, "a key store of another Region").await;
    let shortest = "n".repeat(MIN_NAME_LEN);
    let longest = "n".repeat(MAX_NAME_LEN);

    let rows = vec![
        Row {
            case: "a CloudHSM key store, with its type named",
            request: cloud_hsm_request("hsm-named", &named_cluster),
            expected: json!({
                "CustomKeyStoreName": "hsm-named",
                "CustomKeyStoreType": "AWS_CLOUDHSM",
                "CloudHsmClusterId": named_cluster,
                "TrustAnchorCertificate": TRUST_ANCHOR,
                "ConnectionState": "DISCONNECTED",
            }),
        },
        Row {
            // "The default value is AWS_CLOUDHSM. For a custom key store backed by an
            // CloudHSM cluster, omit the parameter or enter AWS_CLOUDHSM."
            case: "a CloudHSM key store, with its type left to the default",
            request: json!({
                "CustomKeyStoreName": shortest,
                "CloudHsmClusterId": default_cluster,
                "KeyStorePassword": KEY_STORE_PASSWORD,
                "TrustAnchorCertificate": TRUST_ANCHOR,
            }),
            expected: json!({
                "CustomKeyStoreName": shortest,
                "CustomKeyStoreType": "AWS_CLOUDHSM",
                "CloudHsmClusterId": default_cluster,
                "TrustAnchorCertificate": TRUST_ANCHOR,
                "ConnectionState": "DISCONNECTED",
            }),
        },
        Row {
            case: "a CloudHSM key store with the longest friendly name",
            request: cloud_hsm_request(&longest, &longest_name_cluster),
            expected: json!({
                "CustomKeyStoreName": longest,
                "CustomKeyStoreType": "AWS_CLOUDHSM",
                "CloudHsmClusterId": longest_name_cluster,
                "TrustAnchorCertificate": TRUST_ANCHOR,
                "ConnectionState": "DISCONNECTED",
            }),
        },
    ];

    let mut ids: Vec<String> = Vec::new();
    for row in &rows {
        let id = created(&router, row.case, &row.request).await;
        assert!(
            !ids.contains(&id),
            "case {}: every key store gets its own id, got {id} again",
            row.case
        );
        let mut expected = row.expected.clone();
        expected["CustomKeyStoreId"] = json!(id);
        assert_eq!(
            entry(&router, row.case, &id).await,
            expected,
            "case {}: the reader must serve exactly the key store the request described",
            row.case
        );
        ids.push(id);
    }

    // The list of the Region is what the creates left, in creation order.
    let answer = ok(&router, "DescribeCustomKeyStores", &json!({})).await;
    let listed: Vec<String> = answer["CustomKeyStores"]
        .as_array()
        .unwrap_or_else(|| panic!("case every created key store: expected a list in {answer}"))
        .iter()
        .filter_map(|entry| entry["CustomKeyStoreId"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        listed, ids,
        "case every created key store: the reader lists the created key stores in creation order"
    );

    // A custom key store is a regional resource: the same name is free in another
    // Region's endpoint only if the account does not hold it, and a key store created
    // there is served there.
    let elsewhere = created_in(
        &router,
        OTHER_REGION,
        "a key store of another Region",
        &cloud_hsm_request("hsm-elsewhere", &elsewhere_cluster),
    )
    .await;
    let (status, payload) = call(
        &router,
        REGION,
        "DescribeCustomKeyStores",
        &json!({"CustomKeyStoreId": elsewhere}),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload).as_str()),
        (400, "CustomKeyStoreNotFoundException"),
        "case a key store of another Region: it is not served by this Region's endpoint, got \
         {status} with {payload}"
    );
}

/// A created custom key store is a real record the rest of the family acts on: it can
/// be connected, which is what takes it out of the state a create leaves it in, and
/// renamed, and it is reported under either selector.
#[tokio::test]
async fn a_created_custom_key_store_is_one_the_family_acts_on() {
    let (router, engine) = emulator();
    let case = "a created key store the family acts on";
    let cluster_id = active_cluster(&router, &engine, case).await;
    let id = created(&router, case, &cloud_hsm_request("hsm-lifecycle", &cluster_id)).await;

    // "You can create and use KMS keys in your custom key stores only when its
    // ConnectionState is CONNECTED": the create leaves the key store disconnected, and
    // connecting it starts the connection the clock then completes.
    let answer = ok(&router, "ConnectCustomKeyStore", &json!({"CustomKeyStoreId": id})).await;
    super::harness::assert_empty(case, "ConnectCustomKeyStore", &answer);
    assert_eq!(
        entry(&router, case, &id).await["ConnectionState"],
        json!("CONNECTING"),
        "case {case}: the key store enters the transient state at once"
    );
    tokio::time::sleep(past_the_span()).await;
    assert_eq!(
        entry(&router, case, &id).await["ConnectionState"],
        json!("CONNECTED"),
        "case {case}: the connection the call started falls due"
    );

    // The record is also the one an update changes, and the reader serves it under its
    // new name.
    let answer = ok(
        &router,
        "UpdateCustomKeyStore",
        &json!({"CustomKeyStoreId": id, "NewCustomKeyStoreName": "hsm-renamed"}),
    )
    .await;
    super::harness::assert_empty(case, "UpdateCustomKeyStore", &answer);
    let by_name = ok(
        &router,
        "DescribeCustomKeyStores",
        &json!({"CustomKeyStoreName": "hsm-renamed"}),
    )
    .await;
    assert_eq!(
        by_name["CustomKeyStores"][0]["CustomKeyStoreId"],
        json!(id),
        "case {case}: the renamed key store is the one the create made, got {by_name}"
    );
}

/// A request that cannot produce a custom key store is refused with the modelled
/// refusal for its case and persists NOTHING.
#[tokio::test]
async fn create_custom_key_store_refuses_a_request_that_cannot_produce_a_key_store() {
    let (router, engine) = emulator();

    // The fixture: one CloudHSM key store over an active cluster, one external key
    // store on a public endpoint, one behind a VPC endpoint service, and one key store
    // of another Region of the same account. The external ones are SEEDED, since no
    // external key store proxy is reachable from here and so no create makes one.
    let taken_cluster = active_cluster(&router, &engine, "the fixture's cluster").await;
    created(
        &router,
        "the fixture's CloudHSM key store",
        &cloud_hsm_request("taken-here", &taken_cluster),
    )
    .await;
    seed_external_key_store(
        &engine,
        REGION,
        "xks-taken",
        "PUBLIC_ENDPOINT",
        "https://xks-taken.example.com",
        "/kms/xks/v1",
        None,
    );
    seed_external_key_store(
        &engine,
        REGION,
        "xks-vpce-taken",
        "VPC_ENDPOINT_SERVICE",
        "https://xks-vpce-taken.example.com",
        "/kms/xks/v1",
        Some("com.amazonaws.vpce.us-east-1.vpce-svc-00000000000000001"),
    );
    seed_external_key_store(
        &engine,
        OTHER_REGION,
        "taken-elsewhere",
        "PUBLIC_ENDPOINT",
        "https://xks-other-region.example.com",
        "/kms/xks/v1",
        None,
    );

    // Two clusters that cannot back a key store: one that was never activated, and one
    // that is active but whose only private subnet is in a single Availability Zone.
    let inactive_cluster = cloud_hsm_cluster(&router, "a cluster that is not active", 2).await;
    let one_zone_cluster = cloud_hsm_cluster(&router, "a cluster in one zone", 1).await;
    activate(&engine, "a cluster in one zone", &one_zone_cluster);

    let credential = json!({
        "AccessKeyId": PROXY_ACCESS_KEY_ID,
        "RawSecretAccessKey": PROXY_SECRET,
    });
    let rows: Vec<(String, Value, &str)> = vec![
        (
            "a cluster id no cluster of the account has".to_string(),
            cloud_hsm_request("refused", "cluster-2vkpqmfjzck"),
            "CloudHsmClusterNotFoundException",
        ),
        (
            "a cluster that has not been activated".to_string(),
            cloud_hsm_request("refused", &inactive_cluster),
            "CloudHsmClusterNotActiveException",
        ),
        (
            "an active cluster with private subnets in one Availability Zone".to_string(),
            cloud_hsm_request("refused", &one_zone_cluster),
            "CloudHsmClusterInvalidConfigurationException",
        ),
        (
            "a cluster another key store of the account already holds".to_string(),
            cloud_hsm_request("refused", &taken_cluster),
            "CloudHsmClusterInUseException",
        ),
        (
            "a CloudHSM key store with no CloudHsmClusterId".to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "KeyStorePassword": KEY_STORE_PASSWORD,
                   "TrustAnchorCertificate": TRUST_ANCHOR}),
            "ValidationException",
        ),
        (
            "a CloudHSM key store with no KeyStorePassword".to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "CloudHsmClusterId": taken_cluster,
                   "TrustAnchorCertificate": TRUST_ANCHOR}),
            "ValidationException",
        ),
        (
            "a CloudHSM key store with no TrustAnchorCertificate".to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "CloudHsmClusterId": taken_cluster,
                   "KeyStorePassword": KEY_STORE_PASSWORD}),
            "ValidationException",
        ),
        (
            "a CloudHSM key store carrying an external key store property".to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "CustomKeyStoreType": "AWS_CLOUDHSM",
                   "CloudHsmClusterId": taken_cluster,
                   "KeyStorePassword": KEY_STORE_PASSWORD,
                   "TrustAnchorCertificate": TRUST_ANCHOR,
                   "XksProxyUriEndpoint": "https://xks-wrong-type.example.com"}),
            "ValidationException",
        ),
        (
            "an external key store with no XksProxyUriEndpoint".to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
                   "XksProxyConnectivity": "PUBLIC_ENDPOINT",
                   "XksProxyUriPath": "/kms/xks/v1",
                   "XksProxyAuthenticationCredential": credential}),
            "ValidationException",
        ),
        (
            "an external key store with no XksProxyUriPath".to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
                   "XksProxyConnectivity": "PUBLIC_ENDPOINT",
                   "XksProxyUriEndpoint": "https://xks-no-path.example.com",
                   "XksProxyAuthenticationCredential": credential}),
            "ValidationException",
        ),
        (
            "an external key store with no XksProxyConnectivity".to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
                   "XksProxyUriEndpoint": "https://xks-no-connectivity.example.com",
                   "XksProxyUriPath": "/kms/xks/v1",
                   "XksProxyAuthenticationCredential": credential}),
            "ValidationException",
        ),
        (
            "an external key store with no XksProxyAuthenticationCredential".to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
                   "XksProxyConnectivity": "PUBLIC_ENDPOINT",
                   "XksProxyUriEndpoint": "https://xks-no-credential.example.com",
                   "XksProxyUriPath": "/kms/xks/v1"}),
            "ValidationException",
        ),
        (
            "a VPC endpoint service connectivity with no XksProxyVpcEndpointServiceName"
                .to_string(),
            json!({"CustomKeyStoreName": "refused",
                   "CustomKeyStoreType": "EXTERNAL_KEY_STORE",
                   "XksProxyConnectivity": "VPC_ENDPOINT_SERVICE",
                   "XksProxyUriEndpoint": "https://xks-no-service.example.com",
                   "XksProxyUriPath": "/kms/xks/v1",
                   "XksProxyAuthenticationCredential": credential}),
            "ValidationException",
        ),
        (
            "an external key store carrying a CloudHSM property".to_string(),
            {
                let mut request = public_endpoint_request(
                    "refused",
                    "https://xks-wrong-type-two.example.com",
                    "/kms/xks/v1",
                );
                request["CloudHsmClusterId"] = json!(taken_cluster);
                request
            },
            "ValidationException",
        ),
        (
            "a friendly name a key store of this Region holds".to_string(),
            public_endpoint_request(
                "taken-here",
                "https://xks-name-taken.example.com",
                "/kms/xks/v1",
            ),
            "CustomKeyStoreNameInUseException",
        ),
        (
            "a friendly name a key store of another Region of the account holds".to_string(),
            public_endpoint_request(
                "taken-elsewhere",
                "https://xks-name-taken-two.example.com",
                "/kms/xks/v1",
            ),
            "CustomKeyStoreNameInUseException",
        ),
        (
            "the proxy endpoint and API path another external key store holds".to_string(),
            public_endpoint_request(
                "refused",
                "https://xks-taken.example.com",
                "/kms/xks/v1",
            ),
            "XksProxyUriInUseException",
        ),
        (
            "a public endpoint another external key store reaches through a VPC endpoint \
             service"
                .to_string(),
            public_endpoint_request(
                "refused",
                "https://xks-vpce-taken.example.com",
                "/kms/xks/v2",
            ),
            "XksProxyUriEndpointInUseException",
        ),
        (
            "a private DNS name another external key store holds".to_string(),
            vpc_endpoint_request(
                "refused",
                "https://xks-vpce-taken.example.com",
                "/kms/xks/v2",
                "com.amazonaws.vpce.us-east-1.vpce-svc-00000000000000002",
            ),
            "XksProxyUriEndpointInUseException",
        ),
        (
            "the Amazon VPC endpoint service another external key store holds".to_string(),
            vpc_endpoint_request(
                "refused",
                "https://xks-vpce-new.example.com",
                "/kms/xks/v1",
                "com.amazonaws.vpce.us-east-1.vpce-svc-00000000000000001",
            ),
            "XksProxyVpcEndpointServiceInUseException",
        ),
    ];

    let before = held(&engine);
    for (case, request, expected) in rows {
        let (status, payload) = call(&router, REGION, "CreateCustomKeyStore", &request).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, expected),
            "case {case}: expected HTTP 400 {expected}, got {status} with {payload}"
        );
        assert_eq!(
            held(&engine),
            before,
            "case {case}: a refused create must persist nothing"
        );
        // Each refusal about the CloudHSM cluster carries the message its OWN class
        // carries: the not-active class is worded with its fixed sentence, and the
        // cluster-not-found and invalid-configuration classes carry the error code
        // alone, as the recorded refusal of a cluster this account does not hold does.
        if expected == "CloudHsmClusterNotActiveException" {
            assert_eq!(
                error_message(&payload),
                CLUSTER_NOT_ACTIVE_MESSAGE,
                "case {case}: expected the not-active refusal worded \
                 {CLUSTER_NOT_ACTIVE_MESSAGE:?}, got {payload}"
            );
        } else if expected.starts_with("CloudHsmCluster")
            && expected != "CloudHsmClusterInUseException"
        {
            assert!(
                error_message(&payload).is_empty(),
                "case {case}: the refusal carries no message text, got {payload}"
            );
        }
        // A collision refusal names the value that is taken, which is the request's
        // own, so a client can tell which one to change.
        for (code, taken) in [
            ("CustomKeyStoreNameInUseException", request["CustomKeyStoreName"].as_str()),
            ("CloudHsmClusterInUseException", request["CloudHsmClusterId"].as_str()),
            (
                "XksProxyUriEndpointInUseException",
                request["XksProxyUriEndpoint"].as_str(),
            ),
            (
                "XksProxyVpcEndpointServiceInUseException",
                request["XksProxyVpcEndpointServiceName"].as_str(),
            ),
        ] {
            if expected == code {
                let value = taken.unwrap_or_default();
                assert!(
                    error_message(&payload).contains(value),
                    "case {case}: the refusal names {value:?}, got {payload}"
                );
            }
        }
    }
}

/// The refusal of a cluster that exists but is not serving is worded with the fixed
/// sentence of its class, whatever left the cluster out of service, while the sibling
/// refusals of the family send the error code alone — so a client can tell the
/// not-active refusal from the message-less ones.
///
/// The table's rows differ in the `ClusterState` the cluster is in, in how many
/// Availability Zones its private subnets span (the next requirement an active cluster
/// is judged by, which this refusal comes before), in the friendly name the request
/// asks for — the shortest and the longest the model admits — and in the Region whose
/// endpoint is called.
#[tokio::test]
async fn create_custom_key_store_words_the_not_active_refusal_for_every_inactive_cluster() {
    let (router, engine) = emulator();
    let before = held(&engine);

    for (index, cluster_state) in INACTIVE_CLUSTER_STATES.iter().enumerate() {
        let subnets = if index % 2 == 0 { 2 } else { 1 };
        let name = "n".repeat(if index % 2 == 0 { MIN_NAME_LEN } else { MAX_NAME_LEN });
        let region = if index % 3 == 0 { OTHER_REGION } else { REGION };
        let case = format!("a cluster in {cluster_state} over {subnets} subnet(s) in {region}");
        let cluster_id = cloud_hsm_cluster(&router, &case, subnets).await;
        put_cluster_state(&engine, &case, &cluster_id, cluster_state);

        let (status, payload) = call(
            &router,
            region,
            "CreateCustomKeyStore",
            &cloud_hsm_request(&name, &cluster_id),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&payload).as_str(),
                error_message(&payload).as_str()
            ),
            (
                400,
                "CloudHsmClusterNotActiveException",
                CLUSTER_NOT_ACTIVE_MESSAGE
            ),
            "case {case}: expected HTTP 400 CloudHsmClusterNotActiveException worded \
             {CLUSTER_NOT_ACTIVE_MESSAGE:?}, got {status} with {payload}"
        );
        assert_eq!(
            held(&engine),
            before,
            "case {case}: a refused create must persist nothing"
        );

        // The rejected create left no key store behind, and the reader's own refusal
        // for the name it asked for carries NO message: the wording above is what
        // distinguishes the not-active class from the message-less ones.
        let (status, payload) = call(
            &router,
            region,
            "DescribeCustomKeyStores",
            &json!({"CustomKeyStoreName": name}),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&payload).as_str(),
                error_message(&payload).as_str()
            ),
            (400, "CustomKeyStoreNotFoundException", ""),
            "case {case}: expected an unworded HTTP 400 CustomKeyStoreNotFoundException, got \
             {status} with {payload}"
        );

        // The sibling refusal of the SAME operation: a cluster id of the modelled shape
        // that no cluster of the account has is refused with a different code and no
        // message at all.
        // Of the modelled shape — `^cluster-[2-7a-zA-Z]{11,16}$`
        // (`smithy_query(query="shape",shape="CloudHsmClusterIdType")`) — and held by no
        // cluster, one id per row.
        let absent = format!("cluster-absentcluster{}", (b'a' + index as u8) as char);
        let (status, payload) = call(
            &router,
            region,
            "CreateCustomKeyStore",
            &cloud_hsm_request(&name, &absent),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&payload).as_str(),
                error_message(&payload).as_str()
            ),
            (400, "CloudHsmClusterNotFoundException", ""),
            "case {case}: expected an unworded HTTP 400 CloudHsmClusterNotFoundException for \
             {absent}, got {status} with {payload}"
        );
    }

    // The same cluster, once it is serving over subnets in two Availability Zones, is
    // not refused at all: the wording belongs to the refusal, not to every create.
    let case = "an activated cluster";
    let cluster_id = active_cluster(&router, &engine, case).await;
    let id = created(&router, case, &cloud_hsm_request("hsm-now-active", &cluster_id)).await;
    let entry = entry(&router, case, &id).await;
    assert_eq!(
        entry["CloudHsmClusterId"].as_str().unwrap_or_default(),
        cluster_id,
        "case {case}: expected the created key store to report {cluster_id}, got {entry}"
    );
}
