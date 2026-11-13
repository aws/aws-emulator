// SPDX-License-Identifier: CC0-1.0

//! The DNS name a cluster reports for its master node.
//!
//! Every case drives the real HTTP path through the wire harness
//! `tests/all/job_flow_lifecycle.rs` owns, signed for the Region the case names, and reads
//! the cluster back only through `DescribeCluster`.
//!
//! THE RULE: a cluster's master node is provisioned while the cluster comes up, so
//! `DescribeCluster` reports `MasterPublicDnsName` from the moment the clock brings the
//! cluster out of `BOOTSTRAPPING` — the instant its status records as `ReadyDateTime` — for
//! every cluster that became ready, whether it went on to `RUNNING` or to `WAITING`, and
//! still once it has terminated; and reports none for a cluster that never became ready. The
//! name is the Amazon-provided hostname of the address that node holds: `ip-<the address with
//! dashes for dots>` followed by the Region's private DNS domain, which is `ec2.internal` in
//! `us-east-1` and `<region>.compute.internal` everywhere else (Amazon VPC User Guide, "DHCP
//! option sets"; Amazon EC2 User Guide, "Amazon EC2 instance hostname types"). It is a fact
//! of the cluster: every read answers the same name, and no two clusters of the account carry
//! one.
//!
//! The rows span what the rule generalises over: the cluster's state (coming up, running,
//! waiting, terminated), the Region the launch was signed for, and how many clusters the
//! account holds.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{await_cluster_state, await_cluster_where, Emr, PAST_TRANSITION};

/// The Regions a launch below is signed for: the one Region whose private DNS domain is the
/// short `ec2.internal`, and two that build theirs from the Region name.
const REGIONS: &[&str] = &["us-east-1", "us-west-2", "eu-central-1"];

/// The private DNS domain of `region`, by the rule this suite asserts.
fn private_dns_domain(region: &str) -> String {
    if region == "us-east-1" {
        "ec2.internal".to_string()
    } else {
        format!("{region}.compute.internal")
    }
}

/// A `RunJobFlow` body; `kept_alive` is the cluster that stays up with no steps, which is
/// what reaches `WAITING` rather than `RUNNING`.
fn launch(name: &str, kept_alive: bool) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE,
        "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": {
            "InstanceCount": 1,
            "MasterInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": kept_alive,
        },
    })
}

/// Assert the name a cluster reports is in the Amazon-provided private hostname form for
/// `region`, and names the address of the node it belongs to.
fn assert_hostname_form(case: &str, region: &str, reported: &str) {
    let domain = private_dns_domain(region);
    let host = reported.strip_suffix(&format!(".{domain}")).unwrap_or_else(|| {
        panic!("case {case:?}: {reported:?} must end in the {domain:?} private DNS domain")
    });
    let address = host
        .strip_prefix("ip-")
        .unwrap_or_else(|| panic!("case {case:?}: {reported:?} must start with ip-"));
    let octets: Vec<&str> = address.split('-').collect();
    assert_eq!(
        octets.len(),
        4,
        "case {case:?}: {reported:?} must name a dotted-quad address, got {octets:?}"
    );
    for octet in &octets {
        let value: u32 = octet.parse().unwrap_or_else(|_| {
            panic!("case {case:?}: {reported:?} must name an IPv4 address, got {octet:?}")
        });
        assert!(
            value < 256,
            "case {case:?}: {reported:?} must name an IPv4 address, got {octet:?}"
        );
    }
    assert_eq!(
        octets.first().copied(),
        Some("10"),
        "case {case:?}: a master node holds a private address, got {reported:?}"
    );
}

/// One Region's emulator and the clusters launched through it: the account is per
/// emulator, so the uniqueness of a master node's address is asserted within one.
struct Account {
    emr: Emr,
    region: &'static str,
    /// `(case, the states the cluster may report once it is up, its id)` per launch.
    clusters: Vec<(String, &'static [&'static str], String)>,
}

#[tokio::test]
async fn a_cluster_reports_its_master_node_dns_name_once_the_node_is_up() {
    // Every row is launched first and read after ONE wait, so the suite waits out the
    // launch span once rather than once per row.
    let mut accounts: Vec<Account> = Vec::new();
    for region in REGIONS {
        let emr = Emr::with_region(region);
        let mut clusters = Vec::new();
        for kept_alive in [true, false] {
            let case = format!("{region}, kept alive {kept_alive}");
            let cluster_id = emr.run_job_flow(launch(&case, kept_alive)).await;

            // Coming up: the master node is still being provisioned, so there is no name
            // to report yet.
            let starting = emr.describe_cluster(&cluster_id).await;
            assert!(
                starting["Status"]["Timeline"]["ReadyDateTime"].is_null(),
                "case {case:?}: a new cluster is not ready yet, got {starting}"
            );
            assert!(
                starting["MasterPublicDnsName"].as_str().is_none(),
                "case {case:?}: a cluster whose master node is not up yet reports no \
                 MasterPublicDnsName, got {starting}"
            );

            // The states the cluster may report once its launch span has passed: a cluster
            // kept alive with no steps rests in WAITING, and a transient one has by then
            // run its (empty) step list and is walking, or has walked, the shutdown that
            // brings — each of which keeps the name of the master node the cluster brought
            // up, because the cluster was ready at some point.
            let ready_states: &[&str] = if kept_alive {
                &["WAITING"]
            } else {
                &["RUNNING", "TERMINATING", "TERMINATED"]
            };
            clusters.push((case, ready_states, cluster_id));
        }
        accounts.push(Account {
            emr,
            region,
            clusters,
        });
    }

    // `(the name reported, the cluster it belongs to)` per launch, in launch order.
    let mut reported_names: Vec<Vec<String>> = Vec::new();
    for account in &accounts {
        let mut seen: Vec<String> = Vec::new();
        for (case, ready_states, cluster_id) in &account.clusters {
            // Waits for the CONDITION this case is about — the master node being UP, which
            // is what the cluster reports a name for — rather than for a span.
            let up = await_cluster_where(
                &account.emr,
                cluster_id,
                case,
                "a cluster whose master node is up",
                |cluster| cluster["MasterPublicDnsName"].as_str().is_some(),
            )
            .await;
            let state = up["Status"]["State"].as_str().unwrap_or_default().to_string();
            assert!(
                ready_states.contains(&state.as_str()),
                "case {case:?}: expected one of {ready_states:?}, got {up}"
            );
            let reported = up["MasterPublicDnsName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a cluster whose master node is up must report \
                         MasterPublicDnsName, got {up}"
                    )
                })
                .to_string();
            assert_hostname_form(case, account.region, &reported);

            // A read changes nothing: the same name is answered again.
            let again = account.emr.describe_cluster(cluster_id).await;
            assert_eq!(
                again["MasterPublicDnsName"].as_str(),
                Some(reported.as_str()),
                "case {case:?}: every read must report the same name, got {again}"
            );

            // No two clusters of ONE account hold the same master node address.
            assert!(
                !seen.contains(&reported),
                "case {case:?}: {reported:?} was already reported by another cluster of \
                 this account, got {seen:?}"
            );
            seen.push(reported);
        }
        reported_names.push(seen);
    }

    // The node's name survives the cluster: a terminated cluster still names the master
    // node it ran.
    for account in &accounts {
        for (_, _, cluster_id) in &account.clusters {
            account
                .emr
                .ok(
                    "TerminateJobFlows",
                    json!({ "JobFlowIds": [cluster_id.clone()] }),
                )
                .await;
        }
    }
    for (account, names) in accounts.iter().zip(&reported_names) {
        for ((case, _, cluster_id), reported) in account.clusters.iter().zip(names) {
            // Waits for the CONDITION the shutdown is owed — the cluster reporting the
            // terminal state — rather than for a span.
            let terminated =
                await_cluster_state(&account.emr, cluster_id, case, "TERMINATED").await;
            assert_eq!(
                terminated["MasterPublicDnsName"].as_str(),
                Some(reported.as_str()),
                "case {case:?}: a terminated cluster still names the master node it ran, \
                 got {terminated}"
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_terminated_before_it_started_names_no_master_node() {
    let emr = Emr::with_region("us-west-2");
    let cluster_id = emr.run_job_flow(launch("never-started", true)).await;

    // Shut the cluster down while it is still coming up: it never becomes ready, so no
    // master node of it ever comes up.
    emr.ok(
        "TerminateJobFlows",
        json!({ "JobFlowIds": [cluster_id.clone()] }),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    let terminated = emr.describe_cluster(&cluster_id).await;
    assert_eq!(
        terminated["Status"]["State"].as_str(),
        Some("TERMINATED"),
        "the cluster must have terminated, got {terminated}"
    );
    assert!(
        terminated["Status"]["Timeline"]["ReadyDateTime"].is_null(),
        "a cluster shut down while coming up never became ready, got {terminated}"
    );
    assert!(
        terminated["MasterPublicDnsName"].as_str().is_none(),
        "a cluster that never became ready had no master node to name, got {terminated}"
    );
}
