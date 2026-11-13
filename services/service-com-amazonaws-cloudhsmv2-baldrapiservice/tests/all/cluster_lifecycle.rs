// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the cluster/HSM lifecycle this crate implements:
//! `CreateCluster`, `DescribeClusters`, `CreateHsm`, `DeleteHsm`, `DeleteCluster`.
//!
//! Every case drives the real wire path — `POST /` with
//! `X-Amz-Target: BaldrApiService.<Operation>` through the multi-service router — so
//! what is asserted is what a client sees, body and HTTP status included.
//!
//! Every wait here is on a CONDITION — a record reaching the state the clock owes it, or
//! leaving the store — so a case looks again until that holds rather than sleeping for a
//! span: the deadline is generous, so a loaded or a faster host changes nothing about what
//! is asserted ([`poll_until`]).

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cloudhsmv2_baldrapiservice::state::{MAX_CLUSTERS, MAX_HSMS_PER_CLUSTER};
use service_com_amazonaws_cloudhsmv2_baldrapiservice::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// A router carrying this service and the owner of subnets, over one shared engine.
///
/// EC2 is mounted because a subnet is ITS resource: `CreateCluster` resolves every
/// identifier in `SubnetIds` against the subnets the account has, so a case has to name
/// subnets that account really holds — the ones EC2 answers with.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// An EC2 client over the in-process callback into this same emulator, signed for
    /// `region` — the same hop the service under test makes.
    pub(crate) fn ec2(&self, region: &str) -> ::aws_sdk_ec2::Client {
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
            .http_client(EmbeddedEmulator::from_router(self.router.clone()).http_client())
            .credentials_provider(
                EmbeddedEmulator::from_router(self.router.clone()).aws_credentials_provider(),
            )
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_ec2::Client::from_conf(config)
    }

    /// The account's subnets in `region`, as (Availability Zone, subnet id) pairs in zone
    /// order — ASKED of EC2, never assumed: these are the ids `CreateCluster` resolves
    /// `SubnetIds` against. Only the subnets in a zone OF THIS REGION are kept, so a case
    /// names a subnet of the region its requests are signed for.
    pub(crate) async fn subnets_in(&self, region: &str) -> Vec<(String, String)> {
        let described = self
            .ec2(region)
            .describe_subnets()
            .send()
            .await
            .expect("EC2 must report the account's subnets");
        let mut reported: Vec<(String, String)> = described
            .subnets()
            .iter()
            .filter_map(|subnet| {
                Some((
                    subnet.availability_zone()?.to_string(),
                    subnet.subnet_id()?.to_string(),
                ))
            })
            .filter(|(zone, _)| zone.len() == region.len() + 1 && zone.starts_with(region))
            .collect();
        reported.sort();
        assert!(
            !reported.is_empty(),
            "the account must have subnets in {region}, got none"
        );
        reported
    }

    /// The ids of [`Self::subnets_in`], for a case that only names them.
    pub(crate) async fn subnet_ids_in(&self, region: &str) -> Vec<String> {
        self.subnets_in(region)
            .await
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    }

    /// The VPC EC2 reports `subnet_id` in — what a cluster placed in that subnet must
    /// report as its own `VpcId`.
    pub(crate) async fn vpc_of(&self, region: &str, subnet_id: &str) -> String {
        self.ec2(region)
            .describe_subnets()
            .subnet_ids(subnet_id)
            .send()
            .await
            .expect("EC2 must report the subnet")
            .subnets()
            .first()
            .and_then(::aws_sdk_ec2::types::Subnet::vpc_id)
            .expect("the subnet must name its VPC")
            .to_string()
    }

    /// Call `operation` with `body`, signed for `region` so the service resolves the
    /// request's region the way it does in production (the SigV4 credential scope).
    pub(crate) async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("BaldrApiService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cloudhsm/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Call `operation` in the default test region.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Call `operation`, asserting a 200 and returning the response body.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} {response}"
        );
        response
    }
}

/// The region most cases sign for.
pub(crate) const REGION: &str = "us-east-1";

/// Whether `value` is one of the cloudhsm ids modelled as
/// `^<prefix>[2-7a-zA-Z]{11,16}$`.
fn is_modelled_id(value: &str, prefix: &str) -> bool {
    let Some(body) = value.strip_prefix(prefix) else {
        return false;
    };
    (11..=16).contains(&body.len())
        && body
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || (b'2'..=b'7').contains(&b))
}

/// Whether `value` is one of the ids modelled as `^<prefix>[0-9a-fA-F]{8,17}$`.
fn is_hex_id(value: &str, prefix: &str) -> bool {
    let Some(body) = value.strip_prefix(prefix) else {
        return false;
    };
    (8..=17).contains(&body.len()) && body.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The `__type` an awsJson1_1 error body carries.
pub(crate) fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// A cluster's `SubnetMapping` as a sorted AZ -> subnet map.
fn subnet_mapping(cluster: &Value) -> BTreeMap<String, String> {
    cluster
        .get("SubnetMapping")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(az, subnet_id)| {
                    (
                        az.clone(),
                        subnet_id.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How long a case waits for a condition the clock owes before it gives up. Generous: it
/// is not the span under test, only the ceiling on waiting for one.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long a case waits before looking again. See [`DEADLINE`].
const LOOK_AGAIN: Duration = Duration::from_millis(20);

/// Look again until `holds` answers true, or [`DEADLINE`] passes.
pub(crate) async fn poll_until<Look, Held>(what: &str, mut holds: Look)
where
    Look: FnMut() -> Held,
    Held: Future<Output = bool>,
{
    let started = Instant::now();
    loop {
        if holds().await {
            return;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {what:?}: the condition did not hold within {DEADLINE:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The clusters `DescribeClusters` reports with no filter at all.
async fn described_clusters(emulator: &Emulator, case: &str) -> Vec<Value> {
    emulator.ok(case, "DescribeClusters", &json!({})).await["Clusters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One `CreateCluster` request, and what the cluster it mints must report.
struct CreateCase {
    name: &'static str,
    region: &'static str,
    subnets: usize,
    hsm_type: &'static str,
    mode: Option<&'static str>,
    network_type: Option<&'static str>,
    retention: Option<(&'static str, &'static str)>,
    tags: Vec<(&'static str, &'static str)>,
    expected_mode: &'static str,
    expected_network_type: &'static str,
    expected_retention: (&'static str, &'static str),
}

/// The table: a `CreateCluster` answer is computed from the request, the subnets EC2
/// holds for the account and the service's documented defaults, and `DescribeClusters`
/// reports exactly the cluster that was stored. Rows vary the region, the subnet count
/// ("You must specify at least one subnet" and only one per Availability Zone), the mode,
/// the network type, the retention bounds and the tag count.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            name: "defaults, one subnet",
            region: "us-east-1",
            subnets: 1,
            hsm_type: "hsm2m.medium",
            mode: None,
            network_type: None,
            retention: None,
            tags: Vec::new(),
            expected_mode: "FIPS",
            expected_network_type: "IPV4",
            // CloudHSM's default retention window.
            expected_retention: ("DAYS", "90"),
        },
        CreateCase {
            name: "dualstack hsm2m across two subnets",
            region: "eu-west-2",
            subnets: 2,
            hsm_type: "hsm2m.medium",
            mode: Some("NON_FIPS"),
            network_type: Some("DUALSTACK"),
            // The published lower bound of `BackupRetentionPolicy$Value`.
            retention: Some(("DAYS", "7")),
            tags: vec![("Team", "crypto"), ("Env", "test")],
            expected_mode: "NON_FIPS",
            expected_network_type: "DUALSTACK",
            expected_retention: ("DAYS", "7"),
        },
        CreateCase {
            name: "gov region, one subnet in every Availability Zone it has",
            region: "us-gov-west-1",
            subnets: 3,
            hsm_type: "hsm2m.medium",
            mode: Some("FIPS"),
            network_type: Some("IPV4"),
            // The published upper bound of `BackupRetentionPolicy$Value`.
            retention: Some(("DAYS", "379")),
            tags: vec![("Owner", "sec")],
            expected_mode: "FIPS",
            expected_network_type: "IPV4",
            expected_retention: ("DAYS", "379"),
        },
    ]
}

impl CreateCase {
    /// This row as a `CreateCluster` request body, over `subnets` — the account's
    /// subnets in this row's region, as EC2 reports them.
    fn request(&self, subnets: &[(String, String)]) -> Value {
        let requested: Vec<String> = self
            .placed_in(subnets)
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        let mut body = json!({ "HsmType": self.hsm_type, "SubnetIds": requested });
        if let Some(mode) = self.mode {
            body["Mode"] = json!(mode);
        }
        if let Some(network_type) = self.network_type {
            body["NetworkType"] = json!(network_type);
        }
        if let Some((retention_type, value)) = self.retention {
            body["BackupRetentionPolicy"] = json!({ "Type": retention_type, "Value": value });
        }
        if !self.tags.is_empty() {
            body["TagList"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect(),
            );
        }
        body
    }

    /// The subnets of `subnets` this row places its cluster in: the first `self.subnets`
    /// of the account's subnets in this row's region, each in its own Availability Zone.
    fn placed_in(&self, subnets: &[(String, String)]) -> Vec<(String, String)> {
        assert!(
            subnets.len() >= self.subnets,
            "case {:?}: the account must have {} subnets in {}, got {subnets:?}",
            self.name,
            self.subnets,
            self.region
        );
        subnets[..self.subnets].to_vec()
    }

    /// The AZ -> subnet map this row's cluster must carry: the zone EC2 reports for each
    /// requested subnet, mapped to that subnet.
    fn expected_subnet_mapping(&self, subnets: &[(String, String)]) -> BTreeMap<String, String> {
        self.placed_in(subnets).into_iter().collect()
    }
}

#[tokio::test]
async fn create_cluster_computes_the_cluster_and_describe_clusters_returns_it() {
    let emulator = Emulator::new();
    let mut minted: Vec<String> = Vec::new();

    for case in create_cases() {
        let name = case.name;
        let subnets = emulator.subnets_in(case.region).await;
        let (status, response) = emulator
            .call_in(case.region, "CreateCluster", &case.request(&subnets))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateCluster to answer 200, got {status} {response}"
        );
        let cluster = response
            .get("Cluster")
            .cloned()
            .unwrap_or_else(|| panic!("case {name:?}: no Cluster in {response}"));

        let cluster_id = cluster["ClusterId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_modelled_id(&cluster_id, "cluster-"),
            "case {name:?}: expected an id matching ^cluster-[2-7a-zA-Z]{{11,16}}$, got \
             {cluster_id:?}"
        );
        assert!(
            !minted.contains(&cluster_id),
            "case {name:?}: expected a fresh cluster id, got the already-minted \
             {cluster_id:?}"
        );
        minted.push(cluster_id.clone());

        assert_eq!(
            cluster["HsmType"].as_str(),
            Some(case.hsm_type),
            "case {name:?}: expected the requested HsmType, got {:?}",
            cluster["HsmType"]
        );
        assert_eq!(
            cluster["State"].as_str(),
            Some("CREATE_IN_PROGRESS"),
            "case {name:?}: expected a cluster that is still being created, got {:?}",
            cluster["State"]
        );
        assert_eq!(
            cluster["BackupPolicy"].as_str(),
            Some("DEFAULT"),
            "case {name:?}: expected the only modelled BackupPolicy, got {:?}",
            cluster["BackupPolicy"]
        );
        assert_eq!(
            cluster["Mode"].as_str(),
            Some(case.expected_mode),
            "case {name:?}: expected Mode {:?}, got {:?}",
            case.expected_mode,
            cluster["Mode"]
        );
        assert_eq!(
            cluster["NetworkType"].as_str(),
            Some(case.expected_network_type),
            "case {name:?}: expected NetworkType {:?}, got {:?}",
            case.expected_network_type,
            cluster["NetworkType"]
        );
        assert_eq!(
            (
                cluster["BackupRetentionPolicy"]["Type"].as_str(),
                cluster["BackupRetentionPolicy"]["Value"].as_str()
            ),
            (
                Some(case.expected_retention.0),
                Some(case.expected_retention.1)
            ),
            "case {name:?}: expected retention {:?}, got {:?}",
            case.expected_retention,
            cluster["BackupRetentionPolicy"]
        );
        assert_eq!(
            subnet_mapping(&cluster),
            case.expected_subnet_mapping(&subnets),
            "case {name:?}: expected each requested subnet mapped under the Availability \
             Zone EC2 reports for it"
        );
        assert_eq!(
            cluster["Hsms"].as_array().map(Vec::len),
            Some(0),
            "case {name:?}: expected a new cluster to carry no HSMs, got {:?}",
            cluster["Hsms"]
        );
        assert!(
            cluster["CreateTimestamp"].is_number(),
            "case {name:?}: expected a CreateTimestamp, got {:?}",
            cluster["CreateTimestamp"]
        );
        // The VPC is the one that OWNS the requested subnets, as EC2 reports it — never
        // an id this service invented.
        let owning_vpc = emulator
            .vpc_of(case.region, &case.placed_in(&subnets)[0].1)
            .await;
        assert_eq!(
            cluster["VpcId"].as_str(),
            Some(owning_vpc.as_str()),
            "case {name:?}: expected the VPC owning the requested subnets, got {:?}",
            cluster["VpcId"]
        );
        assert!(
            is_hex_id(cluster["SecurityGroup"].as_str().unwrap_or_default(), "sg-"),
            "case {name:?}: expected a minted SecurityGroup, got {:?}",
            cluster["SecurityGroup"]
        );
        let expected_tags: Vec<Value> = case
            .tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect();
        if expected_tags.is_empty() {
            assert!(
                cluster.get("TagList").is_none_or(Value::is_null),
                "case {name:?}: expected no TagList on an untagged cluster, got {:?}",
                cluster["TagList"]
            );
        } else {
            assert_eq!(
                cluster["TagList"].as_array(),
                Some(&expected_tags),
                "case {name:?}: expected the requested tags, got {:?}",
                cluster["TagList"]
            );
        }

        // The round trip: the stored cluster is what a describe returns.
        let described = emulator
            .ok(
                name,
                "DescribeClusters",
                &json!({ "Filters": { "clusterIds": [cluster_id] } }),
            )
            .await;
        let clusters = described["Clusters"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            clusters.len(),
            1,
            "case {name:?}: expected the created cluster to be described once, got \
             {described}"
        );
        let mut stored = clusters[0].clone();
        // The only member that may legitimately differ between the two answers is the
        // state, which the clock moves on its own.
        stored["State"] = cluster["State"].clone();
        assert_eq!(
            stored, cluster,
            "case {name:?}: expected DescribeClusters to return the cluster CreateCluster \
             computed"
        );
    }
}

#[tokio::test]
async fn an_hsm_is_visible_in_its_cluster_and_gone_after_it_is_deleted() {
    let emulator = Emulator::new();
    let subnets = emulator.subnets_in(REGION).await;

    // One cluster over every subnet the account has in this region, so each HSM lands in
    // a different Availability Zone and each row below selects a different subnet from
    // the stored mapping.
    let created = emulator
        .ok(
            "fixture",
            "CreateCluster",
            &json!({
                "HsmType": "hsm2m.medium",
                "SubnetIds": subnets.iter().map(|(_, id)| id.clone()).collect::<Vec<String>>(),
                "NetworkType": "DUALSTACK",
            }),
        )
        .await;
    let cluster_id = created["Cluster"]["ClusterId"]
        .as_str()
        .expect("a cluster id")
        .to_string();

    // Rows differ in the Availability Zone, in whether the caller chose the ENI
    // address, and in which of the three documented selectors deletes the HSM.
    let cases: Vec<(&str, usize, Option<&str>, &str)> = vec![
        ("the first zone, address chosen by the service, deleted by HsmId", 0, None, "HsmId"),
        ("the second zone, address chosen by the caller, deleted by EniIp", 1, Some("10.0.1.77"), "EniIp"),
        ("the third zone, address chosen by the service, deleted by EniId", 2, None, "EniId"),
    ];

    for (name, zone_index, requested_ip, selector) in cases {
        let (availability_zone, zone_subnet) = subnets[zone_index].clone();
        let mut request = json!({ "ClusterId": cluster_id, "AvailabilityZone": availability_zone });
        if let Some(ip) = requested_ip {
            request["IpAddress"] = json!(ip);
        }
        let created_hsm = emulator.ok(name, "CreateHsm", &request).await;
        let hsm = created_hsm["Hsm"].clone();

        let hsm_id = hsm["HsmId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_modelled_id(&hsm_id, "hsm-"),
            "case {name:?}: expected an id matching ^hsm-[2-7a-zA-Z]{{11,16}}$, got {hsm_id:?}"
        );
        let eni_id = hsm["EniId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_hex_id(&eni_id, "eni-"),
            "case {name:?}: expected an id matching ^eni-[0-9a-fA-F]{{8,17}}$, got {eni_id:?}"
        );
        assert_eq!(
            hsm["SubnetId"].as_str(),
            Some(zone_subnet.as_str()),
            "case {name:?}: expected the cluster's subnet for {availability_zone}, got {:?}",
            hsm["SubnetId"]
        );
        assert_eq!(
            hsm["HsmType"].as_str(),
            Some("hsm2m.medium"),
            "case {name:?}: expected the cluster's HSM type, got {:?}",
            hsm["HsmType"]
        );
        assert_eq!(
            hsm["State"].as_str(),
            Some("CREATE_IN_PROGRESS"),
            "case {name:?}: expected a new HSM to still be creating, got {:?}",
            hsm["State"]
        );
        let eni_ip = hsm["EniIp"].as_str().unwrap_or_default().to_string();
        match requested_ip {
            Some(ip) => assert_eq!(
                eni_ip, ip,
                "case {name:?}: expected the requested IpAddress, got {eni_ip:?}"
            ),
            None => assert!(
                eni_ip.starts_with(&format!("10.0.{zone_index}.")),
                "case {name:?}: expected an address from that zone's range, got {eni_ip:?}"
            ),
        }
        assert!(
            hsm["EniIpV6"].as_str().is_some_and(|v6| v6.contains(':')),
            "case {name:?}: expected a DUALSTACK cluster's HSM to carry an IPv6 address, \
             got {:?}",
            hsm["EniIpV6"]
        );

        // The HSM is visible inside its cluster.
        let described = emulator
            .ok(name, "DescribeClusters", &json!({}))
            .await;
        let listed = described["Clusters"][0]["Hsms"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            listed.iter().any(|listed| listed["HsmId"] == hsm["HsmId"]),
            "case {name:?}: expected {hsm_id} in the cluster's Hsms, got {listed:?}"
        );

        // Deleted by whichever selector this row names.
        let mut delete = json!({ "ClusterId": cluster_id });
        delete[selector] = match selector {
            "HsmId" => json!(hsm_id),
            "EniId" => json!(eni_id),
            _ => json!(eni_ip),
        };
        let deleted = emulator.ok(name, "DeleteHsm", &delete).await;
        assert_eq!(
            deleted["HsmId"].as_str(),
            Some(hsm_id.as_str()),
            "case {name:?}: expected DeleteHsm to report the deleted HSM, got {deleted}"
        );

        // The deletion the clock owes leaves the HSM out of its cluster: look again
        // until it is gone rather than waiting for the span.
        poll_until(name, || async {
            !described_clusters(&emulator, name).await[0]["Hsms"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .any(|listed| listed["HsmId"] == hsm["HsmId"])
        })
        .await;
        let remaining = described_clusters(&emulator, name).await[0]["Hsms"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !remaining.iter().any(|listed| listed["HsmId"] == hsm["HsmId"]),
            "case {name:?}: expected {hsm_id} to be gone, got {remaining:?}"
        );

        // Deleting it a second time finds nothing.
        let (status, response) = emulator.call("DeleteHsm", &delete).await;
        assert_eq!(
            (status, error_code(&response).contains("CloudHsmResourceNotFoundException")),
            (400, true),
            "case {name:?}: expected a second delete to be refused as not found, got \
             {status} {response}"
        );
    }
}

#[tokio::test]
async fn every_transition_this_emulator_owes_comes_due_on_its_own() {
    let emulator = Emulator::new();
    let subnets = emulator.subnets_in(REGION).await;
    let (zone, zone_subnet) = subnets[0].clone();

    let created = emulator
        .ok(
            "cluster",
            "CreateCluster",
            &json!({ "HsmType": "hsm2m.medium", "SubnetIds": [zone_subnet] }),
        )
        .await;
    let cluster_id = created["Cluster"]["ClusterId"]
        .as_str()
        .expect("a cluster id")
        .to_string();
    assert_eq!(
        created["Cluster"]["State"].as_str(),
        Some("CREATE_IN_PROGRESS"),
        "case \"cluster\": expected the create answer to show the transient state, got {:?}",
        created["Cluster"]["State"]
    );

    // The cluster is owed UNINITIALIZED: look again until it reports it.
    poll_until("cluster", || async {
        described_clusters(&emulator, "cluster").await[0]["State"] == json!("UNINITIALIZED")
    })
    .await;
    let described = described_clusters(&emulator, "cluster").await;
    assert_eq!(
        described[0]["State"].as_str(),
        Some("UNINITIALIZED"),
        "case \"cluster\": expected a created cluster to settle at UNINITIALIZED, got \
         {described:?}"
    );

    let hsm = emulator
        .ok(
            "hsm",
            "CreateHsm",
            &json!({ "ClusterId": cluster_id, "AvailabilityZone": zone }),
        )
        .await;
    assert_eq!(
        hsm["Hsm"]["State"].as_str(),
        Some("CREATE_IN_PROGRESS"),
        "case \"hsm\": expected the create answer to show the transient state, got {hsm}"
    );
    let hsm_id = hsm["Hsm"]["HsmId"].as_str().expect("an hsm id").to_string();

    // The HSM is owed ACTIVE.
    poll_until("hsm", || async {
        described_clusters(&emulator, "hsm").await[0]["Hsms"][0]["State"] == json!("ACTIVE")
    })
    .await;
    let described = described_clusters(&emulator, "hsm").await;
    assert_eq!(
        described[0]["Hsms"][0]["State"].as_str(),
        Some("ACTIVE"),
        "case \"hsm\": expected a created HSM to settle at ACTIVE, got {described:?}"
    );

    // A cluster that still contains an HSM cannot be deleted.
    let (status, response) = emulator
        .call("DeleteCluster", &json!({ "ClusterId": cluster_id }))
        .await;
    assert_eq!(
        (status, error_code(&response).contains("CloudHsmInvalidRequestException")),
        (400, true),
        "case \"cluster with an HSM\": expected the delete to be refused, got {status} {response}"
    );

    emulator
        .ok(
            "hsm",
            "DeleteHsm",
            &json!({ "ClusterId": cluster_id, "HsmId": hsm_id }),
        )
        .await;
    // The HSM leaves the cluster once its own deletion comes due, which is what lets the
    // cluster be deleted at all.
    poll_until("hsm", || async {
        described_clusters(&emulator, "hsm").await[0]["Hsms"]
            .as_array()
            .is_none_or(Vec::is_empty)
    })
    .await;

    let deleted = emulator
        .ok("cluster", "DeleteCluster", &json!({ "ClusterId": cluster_id }))
        .await;
    assert_eq!(
        deleted["Cluster"]["State"].as_str(),
        Some("DELETE_IN_PROGRESS"),
        "case \"cluster\": expected the delete answer to show the transient state, got {deleted}"
    );

    // The cluster is owed its terminal DELETED.
    poll_until("cluster", || async {
        described_clusters(&emulator, "cluster").await[0]["State"] == json!("DELETED")
    })
    .await;
    let described = described_clusters(&emulator, "cluster").await;
    assert_eq!(
        described[0]["State"].as_str(),
        Some("DELETED"),
        "case \"cluster\": expected a deleted cluster to settle at DELETED, got {described:?}"
    );


    // And it is no longer a resource another operation can act on.
    for (operation, body) in [
        (
            "CreateHsm",
            json!({ "ClusterId": cluster_id, "AvailabilityZone": zone }),
        ),
        ("DeleteCluster", json!({ "ClusterId": cluster_id })),
        (
            "DeleteHsm",
            json!({ "ClusterId": cluster_id, "HsmId": hsm_id }),
        ),
    ] {
        let (status, response) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&response).contains("CloudHsmResourceNotFoundException")),
            (400, true),
            "case {operation:?} on a deleted cluster: expected 400 \
             CloudHsmResourceNotFoundException, got {status} {response}"
        );
    }

    // Once the retention window on the terminal state has passed, the store reclaims the
    // record instead of holding it for the life of the process: look again until the
    // store reports no cluster at all.
    poll_until("cluster", || async {
        described_clusters(&emulator, "cluster").await.is_empty()
    })
    .await;
    let described = described_clusters(&emulator, "cluster").await;
    assert!(
        described.is_empty(),
        "case \"cluster\": expected the deleted cluster to be reclaimed, got {described:?}"
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_the_ceilings_it_keeps_on_itself() {
    // Clusters: the store holds MAX_CLUSTERS of them and refuses the next create.
    let emulator = Emulator::new();
    let subnets = emulator.subnets_in(REGION).await;
    let (_, zone_subnet) = subnets[0].clone();
    for index in 0..MAX_CLUSTERS {
        emulator
            .ok(
                &format!("cluster {index}"),
                "CreateCluster",
                &json!({ "HsmType": "hsm2m.medium", "SubnetIds": [zone_subnet] }),
            )
            .await;
    }
    let (status, response) = emulator
        .call(
            "CreateCluster",
            &json!({ "HsmType": "hsm2m.medium", "SubnetIds": [zone_subnet] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).contains("CloudHsmServiceException")),
        (400, true),
        "case \"one cluster past the ceiling\": expected 400 CloudHsmServiceException, got \
         {status} {response}"
    );

    // HSMs: one cluster holds MAX_HSMS_PER_CLUSTER of them — including the ones whose
    // address the caller chose, which is the path that bypasses address exhaustion —
    // and refuses the next create.
    let emulator = Emulator::new();
    // A fresh emulator holds a fresh account, so its subnets are its own.
    let subnets = emulator.subnets_in(REGION).await;
    let (zone, zone_subnet) = subnets[0].clone();
    let created = emulator
        .ok(
            "fixture",
            "CreateCluster",
            &json!({ "HsmType": "hsm2m.medium", "SubnetIds": [zone_subnet] }),
        )
        .await;
    let cluster_id = created["Cluster"]["ClusterId"]
        .as_str()
        .expect("a cluster id")
        .to_string();
    let availability_zone = zone;
    for index in 0..MAX_HSMS_PER_CLUSTER {
        let mut request =
            json!({ "ClusterId": cluster_id, "AvailabilityZone": availability_zone });
        // Half the rows let the service choose the address, half choose it themselves.
        if index % 2 == 0 {
            request["IpAddress"] = json!(format!("10.1.0.{}", index + 4));
        }
        emulator
            .ok(&format!("hsm {index}"), "CreateHsm", &request)
            .await;
    }
    let (status, response) = emulator
        .call(
            "CreateHsm",
            &json!({
                "ClusterId": cluster_id,
                "AvailabilityZone": availability_zone,
                "IpAddress": "10.2.0.9",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).contains("CloudHsmServiceException")),
        (400, true),
        "case \"one HSM past the ceiling\": expected 400 CloudHsmServiceException, got \
         {status} {response}"
    );
}

#[tokio::test]
async fn describe_clusters_pages_by_max_results_and_round_trips_next_token() {
    let emulator = Emulator::new();
    let subnets = emulator.subnet_ids_in(REGION).await;

    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let response = emulator
            .ok(
                "fixture",
                "CreateCluster",
                &json!({
                    "HsmType": "hsm2m.medium",
                    "SubnetIds": [subnets[index % subnets.len()]],
                }),
            )
            .await;
        created.push(
            response["Cluster"]["ClusterId"]
                .as_str()
                .expect("a cluster id")
                .to_string(),
        );
    }
    created.sort();

    // Rows vary the page size across the modelled `range 1..25`: both bounds and two
    // sizes that do not divide the cluster count.
    for page_size in [1_usize, 2, 3, 25] {
        let name = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let response = emulator.ok(&name, "DescribeClusters", &request).await;
            let page = response["Clusters"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {name:?}: expected at most {page_size} clusters in a page, got {}",
                page.len()
            );
            for cluster in &page {
                seen.push(
                    cluster["ClusterId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            next_token = response["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= 10,
                "case {name:?}: expected paging to terminate, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }

        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {name:?}: expected no cluster on two pages, got {seen:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            sorted, created,
            "case {name:?}: expected paging to report every stored cluster exactly once"
        );
        assert_eq!(
            pages,
            created.len().div_ceil(page_size),
            "case {name:?}: expected {} pages for {} clusters",
            created.len().div_ceil(page_size),
            created.len()
        );
    }
}

#[tokio::test]
async fn requests_outside_the_documented_rules_are_refused() {
    let emulator = Emulator::new();
    let subnets = emulator.subnet_ids_in(REGION).await;
    let first = subnets[0].clone();
    let second = subnets[1].clone();
    let zone = emulator.subnets_in(REGION).await[0].0.clone();

    let created = emulator
        .ok(
            "fixture",
            "CreateCluster",
            &json!({ "HsmType": "hsm2m.medium", "SubnetIds": [first, second] }),
        )
        .await;
    let cluster_id = created["Cluster"]["ClusterId"]
        .as_str()
        .expect("a cluster id")
        .to_string();
    let unknown_cluster = "cluster-aaaaaaaaaaa2";
    let unknown_hsm = "hsm-aaaaaaaaaaa2";

    // Each row: the rule, the request that breaks it, and the modelled error it must
    // answer with. Every one is a 400.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an HSM type outside the documented catalog",
            "CreateCluster",
            json!({ "HsmType": "hsm9.large", "SubnetIds": [first] }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "an HSM type CloudHSM no longer provisions",
            "CreateCluster",
            json!({ "HsmType": "hsm1.medium", "SubnetIds": [first] }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "the same subnet named twice, so one Availability Zone twice",
            "CreateCluster",
            json!({ "HsmType": "hsm2m.medium", "SubnetIds": [first, first] }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a retention window one day below the published range",
            "CreateCluster",
            json!({
                "HsmType": "hsm2m.medium",
                "SubnetIds": [first],
                "BackupRetentionPolicy": { "Type": "DAYS", "Value": "6" },
            }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a retention window one day above the published range",
            "CreateCluster",
            json!({
                "HsmType": "hsm2m.medium",
                "SubnetIds": [first],
                "BackupRetentionPolicy": { "Type": "DAYS", "Value": "380" },
            }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a retention policy missing its Value",
            "CreateCluster",
            json!({
                "HsmType": "hsm2m.medium",
                "SubnetIds": [first],
                "BackupRetentionPolicy": { "Type": "DAYS" },
            }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a source backup this emulator does not hold",
            "CreateCluster",
            json!({
                "HsmType": "hsm2m.medium",
                "SubnetIds": [first],
                "SourceBackupId": "backup-aaaaaaaaaaa2",
            }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "an HSM in a cluster that does not exist",
            "CreateHsm",
            json!({ "ClusterId": unknown_cluster, "AvailabilityZone": zone }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "an HSM in a zone the cluster has no subnet in",
            "CreateHsm",
            json!({ "ClusterId": cluster_id, "AvailabilityZone": format!("{REGION}f") }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "an HSM in another region's zone",
            "CreateHsm",
            json!({ "ClusterId": cluster_id, "AvailabilityZone": "eu-west-2a" }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a delete naming no HSM at all",
            "DeleteHsm",
            json!({ "ClusterId": cluster_id }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a delete naming an HSM the cluster does not contain",
            "DeleteHsm",
            json!({ "ClusterId": cluster_id, "HsmId": unknown_hsm }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "a delete in a cluster that does not exist",
            "DeleteHsm",
            json!({ "ClusterId": unknown_cluster, "HsmId": unknown_hsm }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "a cluster delete naming a cluster that does not exist",
            "DeleteCluster",
            json!({ "ClusterId": unknown_cluster }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "a filter field the operation does not document",
            "DescribeClusters",
            json!({ "Filters": { "clusterNames": ["anything"] } }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a NextToken this emulator never minted",
            "DescribeClusters",
            json!({ "NextToken": "not-a-token" }),
            "CloudHsmInvalidRequestException",
        ),
    ];

    for (name, operation, body, expected) in cases {
        let (status, response) = emulator.call(operation, &body).await;
        let code = error_code(&response);
        assert_eq!(
            (status, code.contains(expected)),
            (400, true),
            "case {name:?}: expected {operation} to answer 400 {expected}, got {status} \
             {response}"
        );
    }
}

#[tokio::test]
async fn describe_clusters_filters_on_every_documented_field() {
    let emulator = Emulator::new();
    let subnets = emulator.subnet_ids_in(REGION).await;

    let mut clusters: Vec<Value> = Vec::new();
    for subnet_id in subnets.iter().take(3) {
        let response = emulator
            .ok(
                "fixture",
                "CreateCluster",
                &json!({ "HsmType": "hsm2m.medium", "SubnetIds": [subnet_id] }),
            )
            .await;
        clusters.push(response["Cluster"].clone());
    }
    // `states` selects on the state the clusters report, so look again until every one of
    // them has settled at the state the clock owes it.
    let created = clusters.len();
    poll_until("fixture", || async {
        described_clusters(&emulator, "fixture")
            .await
            .iter()
            .filter(|cluster| cluster["State"] == json!("UNINITIALIZED"))
            .count()
            == created
    })
    .await;

    let first_id = clusters[0]["ClusterId"].as_str().unwrap_or_default();
    // Every one of them was placed in a subnet of the account's default VPC, so that id
    // selects all of them — the VPC a cluster reports is the one that owns its subnets.
    let owning_vpc = clusters[1]["VpcId"].as_str().unwrap_or_default();

    let cases: Vec<(&str, Value, usize)> = vec![
        ("clusterIds", json!({ "clusterIds": [first_id] }), 1),
        ("vpcIds", json!({ "vpcIds": [owning_vpc] }), created),
        (
            "vpcIds, a VPC no cluster is in",
            json!({ "vpcIds": ["vpc-ffffffffffffffff0"] }),
            0,
        ),
        ("states", json!({ "states": ["UNINITIALIZED"] }), created),
        ("states, none matching", json!({ "states": ["ACTIVE"] }), 0),
        (
            "clusterIds and states together",
            json!({ "clusterIds": [first_id], "states": ["ACTIVE"] }),
            0,
        ),
        (
            "an id no cluster carries",
            json!({ "clusterIds": ["cluster-aaaaaaaaaaa2"] }),
            0,
        ),
    ];

    for (name, filters, expected) in cases {
        let response = emulator
            .ok(name, "DescribeClusters", &json!({ "Filters": filters }))
            .await;
        let returned = response["Clusters"].as_array().map(Vec::len).unwrap_or(0);
        assert_eq!(
            returned, expected,
            "case {name:?}: expected {expected} clusters, got {returned} in {response}"
        );
    }
}

#[tokio::test]
async fn operations_outside_this_slice_still_answer_501() {
    let emulator = Emulator::new();

    for (operation, body) in [
        (
            "InitializeCluster",
            json!({
                "ClusterId": "cluster-aaaaaaaaaaa2",
                "SignedCert": "c2lnbmVk",
                "TrustAnchor": "YW5jaG9y",
            }),
        ),
        (
            "ModifyCluster",
            json!({
                "ClusterId": "cluster-aaaaaaaaaaa2",
                "BackupRetentionPolicy": { "Type": "DAYS", "Value": "90" },
            }),
        ),
        ("DescribeBackups", json!({})),
        ("ListTags", json!({ "ResourceId": "cluster-aaaaaaaaaaa2" })),
    ] {
        let (status, response) = emulator.call(operation, &body).await;
        assert_eq!(
            status, 501,
            "case {operation:?}: expected a scaffolded operation to answer 501, got \
             {status} {response}"
        );
    }
}
