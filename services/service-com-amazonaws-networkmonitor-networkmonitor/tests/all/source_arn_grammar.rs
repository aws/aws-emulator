// SPDX-License-Identifier: CC0-1.0

//! A probe's `sourceArn` is read against the ONE statement of what an ARN is.
//!
//! The rule under test: the six colon-separated fields of an ARN, and which of
//! them may be empty (the region and the account may; the partition, the vendor
//! and the resource may not), are the workspace's single ARN parse
//! (`service_common::arn`, which this service already reads a Region label
//! from), and this service applies only its OWN rules on top of those fields:
//! the vendor the ARN must name (`ec2`, since `ProbeInput$sourceArn` is "The ARN
//! of the subnet" —
//! `smithy_query(query="doc",shape="ProbeInput",member="sourceArn")` — and a VPC
//! subnet is an Amazon EC2 resource), the account that has to hold the subnet,
//! the `subnet/<subnetId>` resource form Amazon EC2 reports for a subnet, and
//! the EC2 id grammar (AWS's "Resource IDs" documentation: an 8- or
//! 17-character identifier after the prefix).
//!
//! Every operation that reads a `sourceArn` reads it that one way: `CreateProbe`,
//! `CreateMonitor`'s inline `probes` list, and `UpdateProbe`, which re-checks the
//! record it has merged its request into. So each row is asserted end-to-end over
//! real HTTP through the router: a spelling the rule admits is accepted and round
//! trips through `GetMonitor`, and one outside it is refused with the modelled
//! `ValidationException` (HTTP 400) and stores nothing.
//!
//! The rows differ along what the rule generalises over: which segment is empty
//! (partition, vendor, region, account, resource), which vendor, account and
//! resource type the ARN names, the case of the vendor token, which separator the
//! resource is written with, how many fields the value carries, and which id form
//! follows the `subnet-` prefix. Which SUBNET two admitted spellings name — and
//! so the per-subnet bound they share — is `probe_subnet_identity`'s subject and
//! is not repeated here.

use serde_json::Value;
use service_com_amazonaws_networkmonitor_networkmonitor::state::arn_resource_id;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, error_code, error_message, get_monitor,
    is_modelled_probe_id, probes_of, string_at, ACCOUNT, REGION,
};

/// The 17-character subnet id most rows name, so that two rows differ only in the
/// way they SPELL the ARN of one subnet.
const SUBNET_ID: &str = "subnet-0123456789abcdef0";

/// `(case, sourceArn)` rows that name a subnet of this account.
fn admitted() -> Vec<(&'static str, String)> {
    vec![
        (
            "the plain spelling",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/{SUBNET_ID}"),
        ),
        (
            // The partition and the region are not part of a subnet's identity,
            // and the one parse reports them as submitted.
            "another partition and region",
            format!("arn:aws-cn:ec2:cn-north-1:{ACCOUNT}:subnet/{SUBNET_ID}"),
        ),
        (
            // An empty region is well formed: AWS writes a partition-global
            // resource that way (`services/common/src/arn.rs`).
            "an empty region segment",
            format!("arn:aws-us-gov:ec2::{ACCOUNT}:subnet/{SUBNET_ID}"),
        ),
        (
            // The shorter of the two id forms AWS publishes for an EC2 resource.
            "an 8-character subnet id",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-0a1b2c3d"),
        ),
    ]
}

/// `(case, sourceArn)` rows that name no subnet of this account.
///
/// Every row still satisfies the shape's own constraints — `@pattern("^arn:.*$")`
/// and `@length(20..2048)` — so what refuses it is the service's reading of the
/// value rather than the boundary validator.
fn refused() -> Vec<(&'static str, String)> {
    vec![
        (
            // The partition, the vendor and the resource are the fields the one
            // parse requires to be non-empty: a value with none of them is not an
            // ARN at all, so it names no subnet.
            "an empty partition segment",
            format!("arn::ec2:{REGION}:{ACCOUNT}:subnet/{SUBNET_ID}"),
        ),
        (
            "an empty vendor segment",
            format!("arn:aws::{REGION}:{ACCOUNT}:subnet/{SUBNET_ID}"),
        ),
        (
            "an empty resource segment",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:"),
        ),
        (
            "another vendor",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:subnet/{SUBNET_ID}"),
        ),
        (
            // An ARN namespace is a fixed lowercase token, so a value differing
            // in case names another vendor.
            "the vendor in another case",
            format!("arn:aws:EC2:{REGION}:{ACCOUNT}:subnet/{SUBNET_ID}"),
        ),
        (
            "another account",
            format!("arn:aws:ec2:{REGION}:111122223333:subnet/{SUBNET_ID}"),
        ),
        (
            // The ARN grammar admits an account-less resource; the subnet still
            // has to be one THIS account holds.
            "an empty account segment",
            format!("arn:aws:ec2:{REGION}::subnet/{SUBNET_ID}"),
        ),
        (
            // An ARN's resource field is written in either of two forms,
            // `<type>/<id>` and `<type>:<id>`, and Amazon EC2 reports a subnet's
            // in the first: the ARN it answers for a subnet it created is
            // `arn:aws:ec2:<region>:<account>:subnet/<subnetId>`
            // (`service-com-amazonaws-ec2-amazonec2`, `CreateSubnet`). So the
            // colon form names no subnet.
            "the colon resource form",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet:{SUBNET_ID}"),
        ),
        (
            "another resource type",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:vpc/vpc-0123456789abcdef0"),
        ),
        (
            "a resource type that is a prefix of `subnet`",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subn/{SUBNET_ID}"),
        ),
        (
            "the resource type with no id at all",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet"),
        ),
        (
            "an empty resource id",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/"),
        ),
        (
            "five fields instead of six",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}"),
        ),
        (
            "an id without the subnet prefix",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/sn-0123456789abcdef0"),
        ),
        (
            // Neither of the two id lengths AWS publishes.
            "an id of an unpublished length",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-0123456"),
        ),
        (
            "an id carrying a character outside the EC2 alphabet",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:subnet/subnet-0123456789abcd_f0"),
        ),
    ]
}

/// One probe body sourced from `source_arn`, with a destination of its own so no
/// two probes of a case are the same request twice.
fn probe_from(source_arn: &str, destination_host: usize) -> String {
    format!(
        r#"{{"sourceArn":"{source_arn}","destination":"203.0.113.{destination_host}",
             "protocol":"ICMP","packetSize":56}}"#
    )
}

/// Create the monitor `name` holding no probes.
async fn monitor_named(router: &MultiServiceRouter, case: &str, name: &str) {
    let body = format!(r#"{{"monitorName":"{name}","aggregationPeriod":30}}"#);
    let (status, created) = create_monitor(router, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup CreateMonitor must succeed, got {status} {created:?}"
    );
}

/// Assert `body` is this service's refusal of a source that names no subnet: the
/// sentence names the resource the value addressed, through the service's own
/// reading of that (`crate::state::arn_resource_id`), so this expectation cannot
/// drift from the rule the message states.
fn assert_no_such_subnet(case: &str, status: u16, body: &Value, source_arn: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a source that names no subnet, \
         got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    let expected = format!(
        "The specified subnet {} does not exist or could not be found.",
        arn_resource_id(source_arn)
    );
    assert_eq!(
        error_message(body),
        expected,
        "case {case:?}: expected the refusal to say {expected:?}, got {body:?}"
    );
}

/// `CreateProbe` accepts every spelling that names a subnet of this account, and
/// the probe it stores round-trips through `GetMonitor`.
#[tokio::test]
async fn create_probe_admits_every_spelling_that_names_a_subnet() {
    let router = super::monitor_and_probe::router();
    for (index, (case, source_arn)) in admitted().iter().enumerate() {
        let name = format!("admitted-create-probe-{index}");
        monitor_named(&router, case, &name).await;
        let body = format!(r#"{{"probe":{}}}"#, probe_from(source_arn, index + 1));
        let (status, created) = create_probe(&router, &name, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a source naming a subnet of this account must be accepted, \
             got {status} {created:?}"
        );
        let probe_id = string_at(&created, "probeId");
        assert!(
            is_modelled_probe_id(&probe_id),
            "case {case:?}: expected a modelled probeId, got {probe_id:?}"
        );

        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        let probes = probes_of(&got);
        assert_eq!(
            probes.len(),
            1,
            "case {case:?}: expected the monitor to hold the one probe, got {got:?}"
        );
        assert_eq!(
            string_at(&probes[0], "sourceArn"),
            *source_arn,
            "case {case:?}: expected the stored sourceArn {source_arn:?}, got {got:?}"
        );
    }
}

/// `CreateMonitor`'s inline `probes` list reads a `sourceArn` the same way, so
/// every admitted spelling creates the monitor and its probe.
#[tokio::test]
async fn create_monitor_admits_every_spelling_that_names_a_subnet() {
    let router = super::monitor_and_probe::router();
    for (index, (case, source_arn)) in admitted().iter().enumerate() {
        let name = format!("admitted-create-monitor-{index}");
        let body = format!(
            r#"{{"monitorName":"{name}","aggregationPeriod":30,"probes":[{}]}}"#,
            probe_from(source_arn, index + 1)
        );
        let (status, created) = create_monitor(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an inline probe naming a subnet of this account must be \
             accepted, got {status} {created:?}"
        );

        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        let probes = probes_of(&got);
        assert_eq!(
            probes.len(),
            1,
            "case {case:?}: expected the monitor to hold the one inline probe, got {got:?}"
        );
        assert_eq!(
            string_at(&probes[0], "sourceArn"),
            *source_arn,
            "case {case:?}: expected the stored sourceArn {source_arn:?}, got {got:?}"
        );
    }
}

/// `UpdateProbe` re-reads the source of the record it writes the same way, so an
/// update to a probe created through any admitted spelling is accepted and leaves
/// the source exactly as the create stored it.
#[tokio::test]
async fn an_update_re_reads_the_stored_source_the_same_way() {
    let router = super::monitor_and_probe::router();
    for (index, (case, source_arn)) in admitted().iter().enumerate() {
        let name = format!("admitted-update-probe-{index}");
        monitor_named(&router, case, &name).await;
        let body = format!(r#"{{"probe":{}}}"#, probe_from(source_arn, index + 1));
        let (status, created) = create_probe(&router, &name, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateProbe must succeed, got {status} {created:?}"
        );
        let probe_id = string_at(&created, "probeId");

        let destination = format!("203.0.113.{}", index + 100);
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/monitors/{name}/probes/{probe_id}"),
            Some(&format!(r#"{{"destination":"{destination}"}}"#)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must be accepted, got {status} {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "destination"),
            destination,
            "case {case:?}: expected the update to store the destination, got {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "sourceArn"),
            *source_arn,
            "case {case:?}: expected the probe to keep its sourceArn, got {updated:?}"
        );
    }
}

/// A value that names no subnet of this account is refused — on `CreateProbe` and
/// on `CreateMonitor`'s inline list alike — and nothing is stored.
#[tokio::test]
async fn every_spelling_that_names_no_subnet_is_refused() {
    let router = super::monitor_and_probe::router();
    for (index, (case, source_arn)) in refused().iter().enumerate() {
        // Through CreateProbe, onto a monitor that must stay empty.
        let name = format!("refused-create-probe-{index}");
        monitor_named(&router, case, &name).await;
        let body = format!(r#"{{"probe":{}}}"#, probe_from(source_arn, index + 1));
        let (status, answered) = create_probe(&router, &name, &body).await;
        assert_no_such_subnet(case, status, &answered, source_arn);
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert!(
            probes_of(&got).is_empty(),
            "case {case:?}: the refused create must attach no probe, got {got:?}"
        );

        // And through CreateMonitor's inline list, which must create nothing.
        let inline = format!("refused-create-monitor-{index}");
        let body = format!(
            r#"{{"monitorName":"{inline}","aggregationPeriod":30,"probes":[{}]}}"#,
            probe_from(source_arn, index + 1)
        );
        let (status, answered) = create_monitor(&router, &body).await;
        assert_no_such_subnet(case, status, &answered, source_arn);
        let (status, missing) = get_monitor(&router, &inline).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store no monitor, got {status} {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {missing:?}"
        );
    }
}
