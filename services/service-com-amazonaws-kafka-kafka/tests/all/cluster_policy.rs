// SPDX-License-Identifier: CC0-1.0

//! `PutClusterPolicy`, `GetClusterPolicy` and `DeleteClusterPolicy`, driven end
//! to end over the wire through the same `MultiServiceRouter` the emulator
//! serves from (the shared `kafka_harness`).
//!
//! The rules here are about the ONE policy a cluster carries: what a put stores
//! is what the reader reports and what the delete removes, each put is a new
//! version of that policy, and a request this endpoint resolves no policy for is
//! refused and changes nothing. Each suite asserts its rule over a table of
//! inputs rather than one recorded request: the rows differ along the policy
//! document's form, size and byte content, along the cluster kind the policy is
//! attached to, and along what makes a `ClusterArn` resolve to no policy
//! (unparsable, another resource kind, never minted, another region, a cluster
//! carrying no policy, a policy already deleted, a cluster already gone).

use serde_json::{json, Value};

// The service's own refusal wordings, so the suite and the handlers cannot drift
// apart on what the service says.
use service_com_amazonaws_kafka_kafka::state::{
    BAD_REQUEST_MESSAGE, CLUSTER_ARN_MEMBER, CLUSTER_POLICY_MEMBER, CURRENT_VERSION_MEMBER,
    MAX_CLUSTER_POLICY_BYTES, RESOURCE_NOT_FOUND_MESSAGE,
};

// The router harness, the signing region, the account id and the URI encoder
// every suite of this crate shares, so this one adds only its own rule.
use super::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The form a submitted document is STORED and reported in: the service
/// canonicalizes a policy rather than echoing the caller's bytes, which is the
/// one minification every service of this emulator that stores a JSON document
/// applies. Read from that shared canonicalization, so this suite and the
/// handlers cannot drift apart on what a read reports; the
/// `cluster_policy_write_rules` suite is what states the rule itself.
fn stored_form(document: &str) -> String {
    service_common::json_document::minified(document)
}

/// The resource URI of one cluster's policy: the model binds all three policy
/// operations to `/v1/clusters/{ClusterArn}/policy`, with the ARN
/// percent-encoded into the single `@httpLabel` path segment.
fn policy_uri(arn: &str) -> String {
    format!("/v1/clusters/{}/policy", pct(arn))
}

/// `PutClusterPolicy`, as an endpoint for `region` serves it. `current_version`
/// is `None` where the request omits the member.
async fn put_in(
    emulator: &Emulator,
    region: &str,
    arn: &str,
    policy: &str,
    current_version: Option<&str>,
) -> Reply {
    let mut body = json!({ "policy": policy });
    if let Some(version) = current_version {
        body["currentVersion"] = json!(version);
    }
    emulator
        .call_in(region, "PUT", &policy_uri(arn), Some(body))
        .await
}

async fn put(emulator: &Emulator, arn: &str, policy: &str, current_version: Option<&str>) -> Reply {
    put_in(emulator, REGION, arn, policy, current_version).await
}

/// `GetClusterPolicy`, as an endpoint for `region` serves it.
async fn get_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(region, "GET", &policy_uri(arn), None)
        .await
}

async fn get(emulator: &Emulator, arn: &str) -> Reply {
    get_in(emulator, REGION, arn).await
}

/// `DeleteClusterPolicy`, as an endpoint for `region` serves it.
async fn delete_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(region, "DELETE", &policy_uri(arn), None)
        .await
}

async fn delete(emulator: &Emulator, arn: &str) -> Reply {
    delete_in(emulator, REGION, arn).await
}

/// Assert `reply` is the refusal the service answers a request whose policy it
/// cannot find: HTTP 404 `NotFoundException` carrying the service's one fixed
/// not-found sentence and naming the parameter at fault.
fn assert_policy_not_found(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (404, "NotFoundException"),
        "case {case}: expected 404 NotFoundException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        RESOURCE_NOT_FOUND_MESSAGE,
        "case {case}: expected the not-found wording, got {}",
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(CLUSTER_ARN_MEMBER),
        "case {case}: the refusal must name {CLUSTER_ARN_MEMBER} as the parameter at fault, got {}",
        reply.body
    );
}

/// Assert `reply` is the refusal the service answers a value that is not a
/// cluster ARN at all: HTTP 400 `BadRequestException` carrying the service's one
/// fixed parameter sentence and naming the cluster-ARN member by its wire name.
fn assert_not_a_cluster_arn(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(CLUSTER_ARN_MEMBER),
        "case {case}: the refusal must name {CLUSTER_ARN_MEMBER} as the parameter at fault, got {}",
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        BAD_REQUEST_MESSAGE,
        "case {case}: expected the fixed parameter sentence, got {}",
        reply.body
    );
}

/// The policy documents the suites below walk, as (case name, document) rows.
/// They differ along what the rules generalise over: how many statements the
/// document holds, whether it carries a condition block, whether it is
/// pretty-printed or compact, whether its text is ASCII, and how large it is.
/// Every row is a resource-based policy document, which is what the service
/// stores: the rejected forms live in their own table below.
fn policy_rows() -> Vec<(&'static str, String)> {
    let principal = format!("arn:aws:iam::{ACCOUNT}:root");
    let one_statement = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": principal },
            "Action": ["kafka:CreateVpcConnection"],
            "Resource": "*",
        }],
    });
    let two_statements = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "one",
                "Effect": "Allow",
                "Principal": { "AWS": principal },
                "Action": ["kafka:CreateVpcConnection"],
                "Resource": "*",
            },
            {
                "Sid": "two",
                "Effect": "Deny",
                "Principal": { "AWS": "*" },
                "Action": ["kafka:DeleteVpcConnection"],
                "Resource": "*",
            },
        ],
    });
    let with_condition = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": principal },
            "Action": ["kafka:GetBootstrapBrokers", "kafka:DescribeCluster"],
            "Resource": "*",
            "Condition": { "StringEquals": { "aws:PrincipalOrgID": "o-0a1b2c3d4e" } },
        }],
    });
    let non_ascii = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "héllo — ünicode",
            "Effect": "Allow",
            "Principal": { "AWS": principal },
            "Action": ["kafka:DescribeClusterV2"],
            "Resource": "*",
        }],
    });
    let many: Vec<Value> = (0..64)
        .map(|index| {
            json!({
                "Sid": format!("statement-{index}"),
                "Effect": "Allow",
                "Principal": { "AWS": principal },
                "Action": ["kafka:DescribeCluster"],
                "Resource": "*",
            })
        })
        .collect();
    let large = json!({ "Version": "2012-10-17", "Statement": many });

    vec![
        ("one-statement", one_statement.to_string()),
        ("two-statements", two_statements.to_string()),
        ("with-condition", with_condition.to_string()),
        ("non-ascii-sid", non_ascii.to_string()),
        ("sixty-four-statements", large.to_string()),
        // The same document pretty-printed: the service canonicalizes what it
        // stores, so a read reports it without that whitespace
        // ([`stored_form`]).
        (
            "pretty-printed",
            serde_json::to_string_pretty(&one_statement).expect("render the policy"),
        ),
    ]
}

/// The bodies that are not a policy document at all, as (case name, body) rows:
/// text that is not JSON, and JSON that is not an object.
fn rejected_policy_rows() -> Vec<(&'static str, &'static str)> {
    vec![
        ("empty", ""),
        ("not-json", "kafka:*"),
        ("truncated-json", "{\"Version\": \"2012-10-17\""),
        ("json-array", "[]"),
        ("json-string", "\"a policy\""),
        ("json-number", "42"),
        ("json-null", "null"),
    ]
}

/// Create one cluster of `kind` under this endpoint and hand back its ARN. The
/// two kinds the model names both carry a policy, so every rule below is
/// exercised on each.
async fn cluster_of_kind(emulator: &Emulator, kind: &str, name: &str) -> String {
    match kind {
        "provisioned" => emulator.create_ok(&ClusterRequest::named(name)).await,
        _ => {
            emulator
                .create_serverless_ok(name, &[vec!["subnet-0a1b2c3d"]], None)
                .await
        }
    }
}

/// RULE: a cluster carries at most one resource-based policy; attaching one
/// stores the document as submitted, the reader reports that stored document
/// with the version the put answered, and the delete removes it, after which
/// every later caller sees the cluster carrying none.
#[tokio::test]
async fn a_stored_policy_is_what_the_reader_reports_and_the_delete_removes() {
    let emulator = Emulator::new();

    for (index, (case, document)) in policy_rows().into_iter().enumerate() {
        for kind in ["provisioned", "serverless"] {
            let case = format!("{case}/{kind}");
            let cluster = cluster_of_kind(&emulator, kind, &format!("pol-{kind}-{index}")).await;

            // Nothing is attached yet, so the policy the request addresses does
            // not exist: what follows is a change the put made rather than a
            // policy that was always there.
            let before = get(&emulator, &cluster).await;
            assert_policy_not_found(&before, &format!("{case}: before the put"));
            let delete_before = delete(&emulator, &cluster).await;
            assert_policy_not_found(&delete_before, &format!("{case}: delete before the put"));

            let stored = put(&emulator, &cluster, &document, None).await;
            assert_eq!(
                (stored.status, stored.error_code()),
                (200, ""),
                "case {case}: expected PutClusterPolicy to succeed, got {} {}",
                stored.status,
                stored.body
            );
            let version = stored
                .at("/currentVersion")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: PutClusterPolicy returned no currentVersion: {}", stored.body)
                })
                .to_string();
            assert!(
                !version.is_empty(),
                "case {case}: the stored policy must have a version, got {}",
                stored.body
            );

            let read = get(&emulator, &cluster).await;
            assert_eq!(
                (read.status, read.error_code()),
                (200, ""),
                "case {case}: expected GetClusterPolicy to succeed, got {} {}",
                read.status,
                read.body
            );
            assert_eq!(
                read.at("/policy").as_str(),
                Some(stored_form(&document).as_str()),
                "case {case}: the reader must report the stored document, got {}",
                read.body
            );
            assert_eq!(
                read.at("/currentVersion").as_str(),
                Some(version.as_str()),
                "case {case}: the reader must report the version the put answered, got {}",
                read.body
            );

            // A read changes nothing a later caller sees.
            let again = get(&emulator, &cluster).await;
            assert_eq!(
                (again.at("/policy"), again.at("/currentVersion")),
                (read.at("/policy"), read.at("/currentVersion")),
                "case {case}: a second read must report the same policy, got {}",
                again.body
            );

            let deleted = delete(&emulator, &cluster).await;
            assert_eq!(
                (deleted.status, deleted.error_code()),
                (200, ""),
                "case {case}: expected DeleteClusterPolicy to succeed, got {} {}",
                deleted.status,
                deleted.body
            );

            // Gone from the reader, and gone for a second delete: one removal,
            // every caller.
            let after = get(&emulator, &cluster).await;
            assert_policy_not_found(&after, &format!("{case}: after the delete"));
            let twice = delete(&emulator, &cluster).await;
            assert_policy_not_found(&twice, &format!("{case}: deleted twice"));

            // The cluster itself is untouched: only its policy was removed.
            let described = emulator.describe_v2(&cluster).await;
            assert_eq!(
                described.status, 200,
                "case {case}: the cluster must still resolve after its policy was deleted, \
                 got {} {}",
                described.status, described.body
            );

            // And the cluster can carry a policy again, as the first revision of
            // a new policy rather than the one that was deleted.
            let remade = put(&emulator, &cluster, &document, None).await;
            assert_eq!(
                (remade.status, remade.at("/currentVersion").as_str()),
                (200, Some(version.as_str())),
                "case {case}: a policy attached after the delete starts the history again, \
                 got {} {}",
                remade.status,
                remade.body
            );
        }
    }
}

/// RULE: each attachment is the next version of that cluster's policy, so the
/// version a put answers with changes exactly when the stored document does, and
/// a put that quotes a version the stored policy is not at is refused HTTP 400
/// `BadRequestException` and replaces nothing.
#[tokio::test]
async fn each_put_is_a_new_version_and_a_stale_one_is_refused() {
    let emulator = Emulator::new();
    let rows = policy_rows();

    for kind in ["provisioned", "serverless"] {
        let cluster = cluster_of_kind(&emulator, kind, &format!("pol-versions-{kind}")).await;

        // A version quoted against a cluster that carries no policy matches
        // nothing, so it is refused and attaches nothing.
        let premature = put(&emulator, &cluster, &rows[0].1, Some("K0000000000000")).await;
        assert_eq!(
            (premature.status, premature.error_code()),
            (400, "BadRequestException"),
            "case {kind}: a version quoted before any policy exists must be refused, got {} {}",
            premature.status,
            premature.body
        );
        assert_eq!(
            premature.at("/invalidParameter").as_str(),
            Some(CURRENT_VERSION_MEMBER),
            "case {kind}: the refusal must name {CURRENT_VERSION_MEMBER}, got {}",
            premature.body
        );
        assert_policy_not_found(
            &get(&emulator, &cluster).await,
            &format!("{kind}: after the refused premature put"),
        );

        let mut seen: Vec<String> = Vec::new();
        let mut version = String::new();
        for (case, document) in &rows {
            let case = format!("{kind}/{case}");
            // The put quotes the version the reader last reported, which is the
            // version the stored policy is at.
            let quoted = if version.is_empty() {
                None
            } else {
                Some(version.as_str())
            };
            let stored = put(&emulator, &cluster, document, quoted).await;
            assert_eq!(
                (stored.status, stored.error_code()),
                (200, ""),
                "case {case}: a put quoting the stored version must succeed, got {} {}",
                stored.status,
                stored.body
            );
            let next = stored
                .at("/currentVersion")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: PutClusterPolicy returned no currentVersion")
                })
                .to_string();
            assert!(
                !seen.contains(&next),
                "case {case}: every attachment must get its own version, {next} repeats {seen:?}"
            );

            // A stale version is refused, and the document it quoted stays.
            let stale = put(&emulator, &cluster, &rows[0].1, Some("K0000000000000")).await;
            assert_eq!(
                (stale.status, stale.error_code()),
                (400, "BadRequestException"),
                "case {case}: a stale version must be refused, got {} {}",
                stale.status,
                stale.body
            );
            let read = get(&emulator, &cluster).await;
            assert_eq!(
                (read.at("/policy").as_str(), read.at("/currentVersion").as_str()),
                (Some(stored_form(document).as_str()), Some(next.as_str())),
                "case {case}: a refused put must leave the stored policy alone, got {}",
                read.body
            );

            seen.push(next.clone());
            version = next;
        }
    }
}

/// RULE: a body that is not a policy document is refused HTTP 400
/// `BadRequestException` naming `Policy`, and leaves the cluster's policy as it
/// found it.
#[tokio::test]
async fn a_body_that_is_not_a_policy_document_is_refused() {
    let emulator = Emulator::new();
    let (_, document) = policy_rows().remove(0);

    for kind in ["provisioned", "serverless"] {
        let cluster = cluster_of_kind(&emulator, kind, &format!("pol-malformed-{kind}")).await;

        // Refused both before anything is attached and after a document is
        // stored: it is the body that refuses, not the cluster's state.
        for attached in [false, true] {
            if attached {
                let stored = put(&emulator, &cluster, &document, None).await;
                assert_eq!(
                    stored.status, 200,
                    "case {kind}: expected the setup put to succeed, got {} {}",
                    stored.status, stored.body
                );
            }
            for (case, body) in rejected_policy_rows() {
                let case = format!("{kind}/{case}/attached={attached}");
                let reply = put(&emulator, &cluster, body, None).await;
                assert_eq!(
                    (reply.status, reply.error_code()),
                    (400, "BadRequestException"),
                    "case {case}: expected 400 BadRequestException, got {} {}",
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    reply.at("/invalidParameter").as_str(),
                    Some(CLUSTER_POLICY_MEMBER),
                    "case {case}: the refusal must name {CLUSTER_POLICY_MEMBER} as the parameter \
                     at fault, got {}",
                    reply.body
                );

                let read = get(&emulator, &cluster).await;
                if attached {
                    assert_eq!(
                        read.at("/policy").as_str(),
                        Some(stored_form(&document).as_str()),
                        "case {case}: a refused put must leave the stored policy alone, got {}",
                        read.body
                    );
                } else {
                    assert_policy_not_found(&read, &case);
                }
            }
        }
    }
}

/// A policy document of exactly `total_bytes` bytes: one statement whose `Sid`
/// is padded out to the length wanted. Used to drive the stored-size ceiling at
/// its bound and one byte past it.
fn padded_policy(total_bytes: usize) -> String {
    let rendered = |sid: String| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": sid,
                "Effect": "Allow",
                "Principal": { "AWS": "*" },
                "Action": ["kafka:DescribeCluster"],
                "Resource": "*",
            }],
        })
        .to_string()
    };
    let empty = rendered(String::new());
    let padding = total_bytes.saturating_sub(empty.len());
    rendered("a".repeat(padding))
}

/// RULE: a policy document larger than the ceiling this emulator stores for one
/// cluster is refused HTTP 400 `BadRequestException` naming `Policy` and stores
/// nothing; a document at the ceiling is stored and read back whole.
#[tokio::test]
async fn a_policy_document_is_bounded_at_the_stored_ceiling() {
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("pol-bounded"))
        .await;

    // The published bound of this emulator's own ceiling, and one byte past it.
    let at_bound = padded_policy(MAX_CLUSTER_POLICY_BYTES);
    assert_eq!(
        at_bound.len(),
        MAX_CLUSTER_POLICY_BYTES,
        "case at-bound: the padded document must be exactly the ceiling, got {}",
        at_bound.len()
    );
    let stored = put(&emulator, &cluster, &at_bound, None).await;
    assert_eq!(
        (stored.status, stored.error_code()),
        (200, ""),
        "case at-bound: a document at the ceiling must be stored, got {} {}",
        stored.status,
        stored.body
    );
    let read = get(&emulator, &cluster).await;
    assert_eq!(
        read.at("/policy").as_str(),
        Some(at_bound.as_str()),
        "case at-bound: the whole document must read back, got {} bytes",
        read.at("/policy").as_str().unwrap_or_default().len()
    );

    let past_bound = padded_policy(MAX_CLUSTER_POLICY_BYTES + 1);
    assert_eq!(
        past_bound.len(),
        MAX_CLUSTER_POLICY_BYTES + 1,
        "case past-bound: the padded document must be one byte past the ceiling, got {}",
        past_bound.len()
    );
    let refused = put(&emulator, &cluster, &past_bound, None).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "BadRequestException"),
        "case past-bound: a document past the ceiling must be refused, got {} {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.at("/invalidParameter").as_str(),
        Some(CLUSTER_POLICY_MEMBER),
        "case past-bound: the refusal must name {CLUSTER_POLICY_MEMBER}, got {}",
        refused.body
    );
    let after = get(&emulator, &cluster).await;
    assert_eq!(
        after.at("/policy").as_str(),
        Some(at_bound.as_str()),
        "case past-bound: a refused put must leave the stored document alone, got {} bytes",
        after.at("/policy").as_str().unwrap_or_default().len()
    );
}

/// RULE: a `ClusterArn` this endpoint resolves no policy for is refused and
/// removes nothing — HTTP 404 `NotFoundException` for a well-formed cluster ARN
/// whose policy does not exist, and HTTP 400 `BadRequestException` for a value
/// that is not a cluster ARN at all — and the reader and the delete draw that
/// line in exactly the same place.
#[tokio::test]
async fn an_arn_that_names_no_policy_is_refused() {
    let emulator = Emulator::new();
    let (_, document) = policy_rows().remove(0);

    // One cluster carrying a policy, one carrying none, one in another region
    // carrying one, one whose policy was deleted, and one that was deleted
    // outright.
    let held = emulator.create_ok(&ClusterRequest::named("pol-held")).await;
    assert_eq!(
        put(&emulator, &held, &document, None).await.status,
        200,
        "expected the policy of the held cluster to be stored"
    );
    let without_policy = emulator
        .create_ok(&ClusterRequest::named("pol-none"))
        .await;
    let elsewhere = emulator
        .create_ok_in("eu-central-1", &ClusterRequest::named("pol-elsewhere"))
        .await;
    assert_eq!(
        put_in(&emulator, "eu-central-1", &elsewhere, &document, None)
            .await
            .status,
        200,
        "expected the other region's policy to be stored there"
    );
    let already_deleted = emulator
        .create_ok(&ClusterRequest::named("pol-already-deleted"))
        .await;
    assert_eq!(
        put(&emulator, &already_deleted, &document, None).await.status,
        200,
        "expected the policy to be stored before it is deleted"
    );
    assert_eq!(
        delete(&emulator, &already_deleted).await.status,
        200,
        "expected the first delete to succeed"
    );
    let cluster_gone = emulator
        .create_ok(&ClusterRequest::named("pol-cluster-gone"))
        .await;
    assert_eq!(
        put(&emulator, &cluster_gone, &document, None).await.status,
        200,
        "expected the policy of the doomed cluster to be stored"
    );
    // A cluster is deletable once it has finished provisioning.
    sleep_past_activation();
    assert_eq!(
        emulator
            .call("DELETE", &format!("/v1/clusters/{}", pct(&cluster_gone)), None)
            .await
            .status,
        200,
        "expected DeleteCluster to succeed"
    );

    let never_minted = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/never-minted/\
         11111111-2222-3333-4444-555555555555-1"
    );
    let not_found: Vec<(&str, String)> = vec![
        ("never-minted-cluster", never_minted),
        ("cluster-without-policy", without_policy.clone()),
        ("another-regions-cluster", elsewhere.clone()),
        ("this-regions-arn-with-another-region", held.replace(REGION, "eu-central-1")),
        ("policy-already-deleted", already_deleted.clone()),
        ("cluster-already-deleted", cluster_gone.clone()),
    ];
    for (case, arn) in &not_found {
        assert_policy_not_found(&delete(&emulator, arn).await, &format!("delete {case}"));
        assert_policy_not_found(&get(&emulator, arn).await, &format!("get {case}"));
    }

    // The same ARN with only its ACCOUNT segment rewritten is not a policy this
    // endpoint cannot find: it is a cluster the caller holds no policy over at all,
    // which the authorization decision answers before any lookup — the rule the
    // `cluster_authorization` suite states over the whole cluster-scoped surface.
    let another_account = held.replace(ACCOUNT, "123456789012");
    for (case, reply) in [
        ("delete", delete(&emulator, &another_account).await),
        ("get", get(&emulator, &another_account).await),
    ] {
        assert_eq!(
            (reply.status, reply.error_code()),
            (403, "AccessDeniedException"),
            "case {case} another-account: expected the authorization denial, got {} {}",
            reply.status,
            reply.body
        );
    }

    let configuration_arn = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/a-configuration/\
         11111111-2222-3333-4444-555555555555-2"
    );
    let not_an_arn: Vec<(&str, String)> = vec![
        ("not-an-arn", "not-an-arn".to_string()),
        ("truncated-arn", "arn:aws:kafka".to_string()),
    ];
    for (case, arn) in &not_an_arn {
        assert_not_a_cluster_arn(&delete(&emulator, arn).await, &format!("delete {case}"));
        assert_not_a_cluster_arn(&get(&emulator, arn).await, &format!("get {case}"));
        let refused = put(&emulator, arn, &document, None).await;
        assert_not_a_cluster_arn(&refused, &format!("put {case}"));
    }

    // A value naming a resource of another KIND is no cluster reference either, so
    // no permission scoped to a cluster is matched against it: this surface answers
    // it as the parameter it cannot read, exactly as it answers a value that is no
    // ARN.
    let another_kind: Vec<(&str, String)> = vec![
        ("configuration-arn", configuration_arn),
        ("another-service-namespace", held.replace(":kafka:", ":kafka-cluster:")),
    ];
    for (case, arn) in &another_kind {
        assert_not_a_cluster_arn(&delete(&emulator, arn).await, &format!("delete {case}"));
        assert_not_a_cluster_arn(&get(&emulator, arn).await, &format!("get {case}"));
        let refused = put(&emulator, arn, &document, None).await;
        assert_not_a_cluster_arn(&refused, &format!("put {case}"));
    }

    // A cluster REFERENCE that identifies no cluster is not a parameter this
    // surface cannot read: it resolves to no cluster resource for a permission to be
    // scoped to, so the authorization decision answers it before any lookup — the
    // same gate the account-segment row above meets.
    let nameless = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/only-a-name");
    for (operation, reply) in [
        ("delete", delete(&emulator, &nameless).await),
        ("get", get(&emulator, &nameless).await),
        ("put", put(&emulator, &nameless, &document, None).await),
    ] {
        assert_eq!(
            (reply.status, reply.error_code()),
            (403, "AccessDeniedException"),
            "case {operation} a-cluster-reference-identifying-no-cluster: expected the \
             authorization refusal, got {} {}",
            reply.status,
            reply.body
        );
    }

    // Nothing above removed anything: the policy this endpoint holds is still
    // readable here, and the other region's is still readable and deletable
    // there.
    let read = get(&emulator, &held).await;
    assert_eq!(
        (read.status, read.at("/policy").as_str()),
        (200, Some(document.as_str())),
        "the stored policy must survive the refusals, got {} {}",
        read.status,
        read.body
    );
    let read_elsewhere = get_in(&emulator, "eu-central-1", &elsewhere).await;
    assert_eq!(
        (read_elsewhere.status, read_elsewhere.at("/policy").as_str()),
        (200, Some(document.as_str())),
        "the other region's policy must still be readable there, got {} {}",
        read_elsewhere.status,
        read_elsewhere.body
    );
    let deleted_there = delete_in(&emulator, "eu-central-1", &elsewhere).await;
    assert_eq!(
        (deleted_there.status, deleted_there.error_code()),
        (200, ""),
        "the region that holds the cluster must be able to delete its policy, got {} {}",
        deleted_there.status,
        deleted_there.body
    );

    // A cluster carrying no policy is unaffected by every call above, and the
    // service's generic bad-parameter wording is what a malformed body is
    // refused with — the one constant both the handler and this suite read.
    assert_policy_not_found(&get(&emulator, &without_policy).await, "untouched cluster");
    let malformed = put(&emulator, &without_policy, "not-json", None).await;
    assert_eq!(
        malformed.error_message(),
        BAD_REQUEST_MESSAGE,
        "expected the service's bad-parameter wording, got {}",
        malformed.body
    );
}
