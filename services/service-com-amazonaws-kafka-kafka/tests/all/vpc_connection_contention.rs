// SPDX-License-Identifier: CC0-1.0

//! `CreateVpcConnection` under CONTENTION, and the multi-VPC setup sequence a
//! caller runs before it — both driven end to end over the same
//! `MultiServiceRouter` the emulator serves from, through the crate's shared
//! harness.
//!
//! RULE 1 (contention): an operation whose model carries no idempotency token
//! mints one INDEPENDENT resource per accepted call, so simultaneous
//! byte-identical calls each succeed and each get their own identity — none is
//! refused for contention, none is merged into another, and every minted
//! resource is readable and is reported exactly once by the listing that scopes
//! them to their parent.
//!
//! RULE 2 (the clock owes every transition): a status transition this service
//! reports asynchronously becomes DUE at the span the record stored for it and
//! is materialised by the next read of that record, so a caller that polls a
//! reader reaches the terminal value in bounded time; and because the operation
//! in flight on a cluster is part of the cluster's stored record, both cluster
//! views report the same `ActiveOperationArn` for it.
//!
//! The rows differ along what the rules generalise over: how many callers race
//! (2, 3 and 5), the authentication type (every entry of the service's catalog),
//! the EC2 id form (the 8- and the 17-hex-digit one), the subnet and security
//! group count, whether the request carries tags, and the cluster the
//! connections target.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    CLUSTER_ACTIVATION_DELAY, CLUSTER_UPDATE_DELAY, VPC_CONNECTION_AUTHENTICATION_TYPES,
};

use crate::kafka_harness::{pct, ClusterRequest, ConnectionRequest, Emulator, Reply, REGION};

/// How long a polling caller is given, past the span the record stored, before
/// the suite calls a transition never-due. Generous enough that a loaded machine
/// does not fail the assertion, and far short of any client's own ceiling.
const POLL_SLACK: Duration = Duration::from_secs(5);

/// How long the poll loops below wait between probes — a probing cadence, not a
/// span of the service: the service stores instants and sleeps for none.
const PROBE_INTERVAL: Duration = Duration::from_millis(25);

/// One row of the contention table: the request every racing caller sends,
/// byte for byte, and how many of them send it.
#[derive(Debug, Clone)]
struct Case {
    /// Names the row and the cluster the race targets.
    name: &'static str,
    /// How many callers issue the identical request at the same time.
    callers: usize,
    /// A `VpcConnectionAuthentication` value, from
    /// [`VPC_CONNECTION_AUTHENTICATION_TYPES`].
    authentication: &'static str,
    vpc_id: &'static str,
    client_subnets: Vec<&'static str>,
    security_groups: Vec<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

impl Case {
    /// The one request every caller of this row sends.
    fn request(&self, cluster_arn: &str) -> ConnectionRequest {
        ConnectionRequest {
            target_cluster_arn: cluster_arn.to_string(),
            authentication: self.authentication,
            vpc_id: self.vpc_id,
            client_subnets: self.client_subnets.clone(),
            security_groups: self.security_groups.clone(),
            tags: self.tags.clone(),
        }
    }
}

/// The contention table. One row per authentication type the service accepts —
/// the catalog is `state::VPC_CONNECTION_AUTHENTICATION_TYPES`, which the rows
/// below are checked against — with the caller count, the EC2 id form, the
/// member counts and the tag map varied across them, so no row is the recorded
/// request repeated.
fn cases() -> Vec<Case> {
    let cases = vec![
        // Two callers, the minimal request: the smallest race there is.
        Case {
            name: "two-callers-sasl-iam",
            callers: 2,
            authentication: "SASL_IAM",
            vpc_id: "vpc-0a1b2c3d",
            client_subnets: vec!["subnet-0a1b2c3d", "subnet-0a1b2c3e", "subnet-0a1b2c3f"],
            security_groups: vec!["sg-0a1b2c3d"],
            tags: Vec::new(),
        },
        // Three callers, the 17-hex-digit EC2 id form, three subnets and two
        // security groups, with tags: a bigger request and a bigger race.
        Case {
            name: "three-callers-tls-long-ids",
            callers: 3,
            authentication: "TLS",
            vpc_id: "vpc-0123456789abcdef0",
            client_subnets: vec![
                "subnet-0123456789abcdef0",
                "subnet-0123456789abcdef1",
                "subnet-0123456789abcdef2",
            ],
            security_groups: vec!["sg-0123456789abcdef0", "sg-0123456789abcdef1"],
            tags: vec![("Team", "platform"), ("Env", "test")],
        },
        // Five callers: more racers than the service has worker threads, so the
        // minting path is entered while it is already busy.
        Case {
            name: "five-callers-sasl-scram",
            callers: 5,
            authentication: "SASL_SCRAM",
            vpc_id: "vpc-0a1b2c3d",
            client_subnets: vec!["subnet-0a1b2c3d", "subnet-0a1b2c4e", "subnet-0a1b2c5f"],
            security_groups: vec!["sg-0a1b2c3d"],
            tags: vec![("Only", "one-tag")],
        },
    ];
    for authentication in VPC_CONNECTION_AUTHENTICATION_TYPES {
        assert!(
            cases
                .iter()
                .any(|case| case.authentication == authentication),
            "the contention table must cover the authentication type {authentication:?}"
        );
    }
    cases
}

/// Which cluster reader a poll or a read goes through. The two readers report
/// the same stored record, so every rule below holds through both.
#[derive(Debug, Clone, Copy)]
enum View {
    /// `DescribeCluster`, which reports `ClusterInfo`.
    V1,
    /// `DescribeClusterV2`, which reports `Cluster`.
    V2,
}

impl View {
    fn name(self) -> &'static str {
        match self {
            View::V1 => "DescribeCluster",
            View::V2 => "DescribeClusterV2",
        }
    }

    async fn read(self, emulator: &Emulator, arn: &str) -> Reply {
        match self {
            View::V1 => emulator.describe(arn).await,
            View::V2 => emulator.describe_v2(arn).await,
        }
    }
}

/// A cluster that accepts multi-VPC private connectivity, created and polled to
/// `ACTIVE` exactly as a client does it.
async fn active_cluster(emulator: &Emulator, name: &str) -> String {
    let arn = emulator
        .create_ok(&ClusterRequest {
            vpc_iam: Some(true),
            vpc_tls: Some(true),
            vpc_scram: Some(true),
            sasl_iam: Some(true),
            ..ClusterRequest::named(name)
        })
        .await;
    poll_cluster_state(emulator, name, View::V1, &arn, "ACTIVE").await;
    arn
}

/// Poll a cluster reader until the cluster reports `wanted`, the way a client
/// waits for a state the clock owes. Answers how many probes it took; fails the
/// case when the transition is not due within the span the record stored.
async fn poll_cluster_state(
    emulator: &Emulator,
    case: &str,
    view: View,
    arn: &str,
    wanted: &str,
) -> usize {
    let deadline = Instant::now() + CLUSTER_ACTIVATION_DELAY + CLUSTER_UPDATE_DELAY + POLL_SLACK;
    let mut probes = 0;
    loop {
        let reply = view.read(emulator, arn).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected {} to return 200, got {} {}",
            view.name(),
            reply.status,
            reply.body
        );
        probes += 1;
        let state = reply.at("/clusterInfo/state").as_str().unwrap_or_default().to_string();
        if state == wanted {
            return probes;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the cluster never reported {wanted:?} within \
             {CLUSTER_ACTIVATION_DELAY:?}+{CLUSTER_UPDATE_DELAY:?}+{POLL_SLACK:?} \
             ({probes} probes), it is still {state:?}"
        );
        std::thread::sleep(PROBE_INTERVAL);
    }
}

/// Poll `DescribeClusterOperationV2` until the operation reports `wanted`, the
/// way a client waits for an asynchronous update to finish. Answers how many
/// probes it took; fails the case when the completion is not due within the span
/// the record stored.
async fn poll_operation_state(
    emulator: &Emulator,
    case: &str,
    operation_arn: &str,
    wanted: &str,
) -> usize {
    let deadline = Instant::now() + CLUSTER_UPDATE_DELAY + POLL_SLACK;
    let mut probes = 0;
    loop {
        let reply = emulator
            .call("GET", &format!("/api/v2/operations/{}", pct(operation_arn)), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected DescribeClusterOperationV2 to return 200, got {} {}",
            reply.status, reply.body
        );
        probes += 1;
        let state = reply
            .at("/clusterOperationInfo/operationState")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == wanted {
            return probes;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the cluster operation never reported {wanted:?} within \
             {CLUSTER_UPDATE_DELAY:?}+{POLL_SLACK:?} ({probes} probes), it is still {state:?}"
        );
        std::thread::sleep(PROBE_INTERVAL);
    }
}

/// How many times each VPC connection of `cluster_arn` appears in
/// `ListClientVpcConnections`, walking every page the listing serves.
async fn listed_counts(emulator: &Emulator, case: &str, cluster_arn: &str) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut token: Option<String> = None;
    // More rounds than any row below can need, so a listing that never stops
    // handing back a token fails here instead of looping forever.
    for _ in 0..16 {
        let query = token
            .as_deref()
            .map(|token| format!("?nextToken={}", pct(token)))
            .unwrap_or_default();
        let reply = emulator
            .list_client_vpc_connections(cluster_arn, &query)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected ListClientVpcConnections to return 200, got {} {}",
            reply.status, reply.body
        );
        for entry in reply
            .at("/clientVpcConnections")
            .as_array()
            .unwrap_or(&Vec::new())
        {
            let arn = entry["vpcConnectionArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a listed connection carries no vpcConnectionArn: {entry}")
                })
                .to_string();
            *counts.entry(arn).or_default() += 1;
        }
        match reply.at("/nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return counts,
        }
    }
    panic!("case {case}: ListClientVpcConnections never stopped paging");
}

/// RULE 1: simultaneous byte-identical `CreateVpcConnection` calls against one
/// cluster each answer 200 with a DISTINCT `VpcConnectionArn`, each minted
/// connection reads back, and the cluster's listing reports each exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_identical_creates_each_mint_their_own_connection() {
    let emulator = Arc::new(Emulator::new());
    for case in cases() {
        let cluster = active_cluster(&emulator, case.name).await;
        let request = case.request(&cluster);

        // Every caller sends the SAME request at the same time, on its own task.
        let mut racers = Vec::with_capacity(case.callers);
        for _ in 0..case.callers {
            let handle = Arc::clone(&emulator);
            let request = request.clone();
            racers.push(tokio::spawn(async move {
                handle.create_vpc_connection(&request).await
            }));
        }

        let mut arns: Vec<String> = Vec::with_capacity(case.callers);
        for racer in racers {
            let reply = racer.await.expect("a racing CreateVpcConnection panicked");
            assert_eq!(
                reply.status, 200,
                "case {case:?}: contention is not a modelled refusal for \
                 CreateVpcConnection, so every racing call must answer 200, got {} {}",
                reply.status, reply.body
            );
            let arn = reply
                .at("/vpcConnectionArn")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a racing create returned no vpcConnectionArn: {}", reply.body)
                })
                .to_string();
            assert_eq!(
                reply.at("/authentication").as_str(),
                Some(case.authentication),
                "case {case:?}: expected the submitted authentication back, got {}",
                reply.at("/authentication")
            );
            arns.push(arn);
        }

        // DISTINCT identities: nothing was merged into anything else.
        let mut distinct = arns.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            arns.len(),
            "case {case:?}: expected {} distinct VpcConnectionArns, got {arns:?}",
            arns.len()
        );

        // Every minted ARN names a connection the service holds, targeting the
        // cluster the race was against.
        for arn in &arns {
            let described = emulator.describe_vpc_connection(arn).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: expected DescribeVpcConnection({arn}) to return 200, got {} {}",
                described.status, described.body
            );
            assert_eq!(
                described.at("/targetClusterArn").as_str(),
                Some(cluster.as_str()),
                "case {case:?}: expected the connection to target {cluster}, got {}",
                described.at("/targetClusterArn")
            );
        }

        // EXACTLY ONCE each in the cluster's own listing, and nothing else.
        let counts = listed_counts(&emulator, case.name, &cluster).await;
        assert_eq!(
            counts.len(),
            arns.len(),
            "case {case:?}: expected the cluster to list {} connections, got {counts:?}",
            arns.len()
        );
        for arn in &arns {
            assert_eq!(
                counts.get(arn).copied(),
                Some(1),
                "case {case:?}: expected {arn} to be listed exactly once, got {counts:?}"
            );
        }
    }
}

/// RULE 1, on the registry's own ordering: the connections two races mint
/// against two clusters stay with their own cluster, so a race never lands a
/// connection in another cluster's listing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn races_against_two_clusters_do_not_cross() {
    let emulator = Arc::new(Emulator::new());
    let first = active_cluster(&emulator, "crossing-first").await;
    let second = active_cluster(&emulator, "crossing-second").await;

    let mut racers = Vec::new();
    for cluster in [&first, &second] {
        for _ in 0..3 {
            let handle = Arc::clone(&emulator);
            let request = ConnectionRequest::to(cluster);
            let cluster = cluster.clone();
            racers.push(tokio::spawn(async move {
                let reply = handle.create_vpc_connection(&request).await;
                (cluster, reply)
            }));
        }
    }

    let mut minted: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for racer in racers {
        let (cluster, reply) = racer.await.expect("a racing CreateVpcConnection panicked");
        assert_eq!(
            reply.status, 200,
            "case two-clusters: expected every racing create to answer 200, got {} {}",
            reply.status, reply.body
        );
        let arn = reply
            .at("/vpcConnectionArn")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case two-clusters: a racing create returned no vpcConnectionArn: {}", reply.body)
            })
            .to_string();
        minted.entry(cluster).or_default().push(arn);
    }

    for (cluster, expected) in &minted {
        let counts = listed_counts(&emulator, "two-clusters", cluster).await;
        assert_eq!(
            counts.len(),
            expected.len(),
            "case two-clusters: expected {cluster} to list only its own {} connections, got {counts:?}",
            expected.len()
        );
        for arn in expected {
            assert_eq!(
                counts.get(arn).copied(),
                Some(1),
                "case two-clusters: expected {arn} to be listed exactly once under {cluster}, \
                 got {counts:?}"
            );
        }
    }
}

/// RULE 2: the multi-VPC setup sequence a caller runs before the race —
/// create the cluster, poll it to `ACTIVE`, start a connectivity update, poll
/// the operation to `UPDATE_COMPLETE` — resolves, with each transition due at
/// the span its record stored; and while the update is in flight BOTH cluster
/// views report its ARN as the cluster's active operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_multi_vpc_setup_sequence_resolves_and_both_cluster_views_agree() {
    let emulator = Emulator::new();

    // The rows differ in how the cluster was created — the v1 `CreateCluster`
    // body and the v2 `CreateClusterV2` provisioned body, polled through the
    // matching reader — and in the connectivity the update submits, since the
    // rule is about the transitions and the record, not about one configuration.
    let rows = [
        (
            "sasl-iam-multi-vpc",
            View::V1,
            json!({ "vpcConnectivity": { "clientAuthentication": { "sasl": { "iam": { "enabled": true } } } } }),
        ),
        (
            "tls-multi-vpc-and-public-access",
            View::V2,
            json!({
                "publicAccess": { "type": "SERVICE_PROVIDED_EIPS" },
                "vpcConnectivity": { "clientAuthentication": { "tls": { "enabled": true } } }
            }),
        ),
    ];

    for (case, view, connectivity) in rows {
        let request = ClusterRequest {
            sasl_iam: Some(true),
            ..ClusterRequest::named(case)
        };
        let before_create = Instant::now();
        let arn = match view {
            View::V1 => emulator.create_ok(&request).await,
            View::V2 => {
                // The v2 create carries the same configuration under its
                // `provisioned` member, with the cluster name at the top level.
                let mut provisioned = request.to_json();
                let members = provisioned
                    .as_object_mut()
                    .expect("the CreateCluster body is a JSON object");
                let name = members
                    .remove("clusterName")
                    .expect("the CreateCluster body names the cluster");
                let reply = emulator
                    .create_cluster_v2(json!({ "clusterName": name, "provisioned": provisioned }))
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {case}: expected CreateClusterV2 to return 200, got {} {}",
                    reply.status, reply.body
                );
                reply.body["clusterArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: CreateClusterV2 returned no clusterArn")
                    })
                    .to_string()
            }
        };

        // The span is never zero: a caller reading straight after the create
        // still sees the transient state the record was stored in. Judged
        // against the MEASURED elapsed time from before the create, so a
        // machine slow enough to have passed the stored span before the read
        // lands fails nothing for a scheduling reason.
        let fresh = view.read(&emulator, &arn).await;
        let elapsed = before_create.elapsed();
        let state = fresh
            .at("/clusterInfo/state")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let inside_span = elapsed < CLUSTER_ACTIVATION_DELAY;
        if inside_span {
            assert_eq!(
                state, "CREATING",
                "case {case}: {} read {elapsed:?} after the create, inside the stored \
                 {CLUSTER_ACTIVATION_DELAY:?} span, must report CREATING, got {state:?}",
                view.name()
            );
        } else {
            assert!(
                state == "CREATING" || state == "ACTIVE",
                "case {case}: {} read {elapsed:?} after the create must report CREATING or \
                 ACTIVE, got {state:?}",
                view.name()
            );
        }
        assert!(
            fresh.body.pointer("/clusterInfo/activeOperationArn").is_none(),
            "case {case}: a create starts no cluster operation, got {}",
            fresh.body
        );

        // ... and the transition IS due, so a polling caller reaches ACTIVE.
        let probes = poll_cluster_state(&emulator, case, view, &arn, "ACTIVE").await;
        assert!(
            probes > 1 || !inside_span,
            "case {case}: the cluster must be reported CREATING before it is ACTIVE, \
             but the poll was already done after {probes} probe(s)"
        );

        let version = view
            .read(&emulator, &arn)
            .await
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the cluster reported no currentVersion"))
            .to_string();

        let before_update = Instant::now();
        let started = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/connectivity", pct(&arn)),
                Some(json!({ "currentVersion": version, "connectivityInfo": connectivity })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {case}: expected UpdateConnectivity to return 200, got {} {}",
            started.status, started.body
        );
        let operation = started
            .at("/clusterOperationArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: UpdateConnectivity returned no clusterOperationArn"))
            .to_string();

        // In flight: the operation belongs to the cluster RECORD, so the
        // `ClusterInfo` reader and the `Cluster` reader report the same ARN.
        // Read inside the stored span, measured as above; once the span has
        // passed, the update has landed and there is nothing in flight to
        // report, which the settled assertions below cover instead.
        let v1 = View::V1.read(&emulator, &arn).await;
        let v2 = View::V2.read(&emulator, &arn).await;
        let update_inside_span = before_update.elapsed() < CLUSTER_UPDATE_DELAY;
        if update_inside_span {
            for (view, reply) in [(View::V1, &v1), (View::V2, &v2)] {
                assert_eq!(
                    reply.at("/clusterInfo/activeOperationArn").as_str(),
                    Some(operation.as_str()),
                    "case {case}: expected {} to report activeOperationArn {operation:?}, got {}",
                    view.name(),
                    reply.at("/clusterInfo/activeOperationArn")
                );
                assert_eq!(
                    reply.at("/clusterInfo/state").as_str(),
                    Some("UPDATING"),
                    "case {case}: expected {} to report UPDATING while the operation runs, got {}",
                    view.name(),
                    reply.at("/clusterInfo/state")
                );
            }
        }
        // Whichever side of the span the reads landed on, the two views report
        // ONE record: they never disagree about the operation in flight.
        assert_eq!(
            v1.at("/clusterInfo/activeOperationArn"),
            v2.at("/clusterInfo/activeOperationArn"),
            "case {case}: the two cluster views must report one record's active operation, \
             got {} and {}",
            v1.at("/clusterInfo/activeOperationArn"),
            v2.at("/clusterInfo/activeOperationArn")
        );

        // The completion is due at the span the operation stored, so a polling
        // caller reaches UPDATE_COMPLETE.
        let probes = poll_operation_state(&emulator, case, &operation, "UPDATE_COMPLETE").await;
        assert!(
            probes > 1 || !update_inside_span,
            "case {case}: the operation must be reported UPDATE_IN_PROGRESS before it is \
             UPDATE_COMPLETE, but the poll was already done after {probes} probe(s)"
        );

        // Settled: the cluster is ACTIVE again at a new version, with no
        // operation in flight in EITHER view, and the submitted connectivity is
        // the cluster's own.
        poll_cluster_state(&emulator, case, view, &arn, "ACTIVE").await;
        let v1 = View::V1.read(&emulator, &arn).await;
        let v2 = View::V2.read(&emulator, &arn).await;
        for (view, reply) in [(View::V1, &v1), (View::V2, &v2)] {
            assert!(
                reply
                    .body
                    .pointer("/clusterInfo/activeOperationArn")
                    .is_none_or(Value::is_null),
                "case {case}: expected {} to report no active operation once the update \
                 landed, got {}",
                view.name(),
                reply.body
            );
        }
        assert_ne!(
            v1.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: a landed update must give the cluster a new version, got {}",
            v1.at("/clusterInfo/currentVersion")
        );
        assert_eq!(
            v1.at("/clusterInfo/brokerNodeGroupInfo/connectivityInfo"),
            v2.at("/clusterInfo/provisioned/brokerNodeGroupInfo/connectivityInfo"),
            "case {case}: the two cluster views must report one record's connectivity, got {} and {}",
            v1.at("/clusterInfo/brokerNodeGroupInfo/connectivityInfo"),
            v2.at("/clusterInfo/provisioned/brokerNodeGroupInfo/connectivityInfo")
        );

        // The race the setup prepares then runs against the settled cluster, in
        // the region the cluster lives in.
        let request = ConnectionRequest::to(&arn);
        let first = emulator.create_vpc_connection(&request).await;
        let second = emulator.create_vpc_connection(&request).await;
        for (which, reply) in [("first", &first), ("second", &second)] {
            assert_eq!(
                reply.status, 200,
                "case {case}: expected the {which} CreateVpcConnection to return 200, got {} {}",
                reply.status, reply.body
            );
        }
        assert_ne!(
            first.at("/vpcConnectionArn"),
            second.at("/vpcConnectionArn"),
            "case {case}: expected two distinct VpcConnectionArns, got {} twice",
            first.at("/vpcConnectionArn")
        );
        let counts = listed_counts(&emulator, case, &arn).await;
        assert_eq!(
            counts.len(),
            2,
            "case {case}: expected the cluster in {REGION} to list both connections, got {counts:?}"
        );
    }
}
