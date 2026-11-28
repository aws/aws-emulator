// SPDX-License-Identifier: CC0-1.0

//! The WRITE rules of the cluster-policy surface, driven end to end over the
//! same `MultiServiceRouter` the emulator serves from (the shared
//! `kafka_harness`): what the service accepts as a cluster policy, the
//! optimistic lock a replacement passes through, and the form a stored document
//! is reported in.
//!
//! Three rules, each asserted over a TABLE rather than one request:
//!
//! * a cluster policy is a document in the IAM JSON policy language, so one that
//!   breaks that language's grammar is refused `BadRequestException` (HTTP 400)
//!   naming the element it omits, and nothing is attached or replaced;
//! * the `CurrentVersion` a put quotes is an optimistic lock over the one policy
//!   a cluster carries: a replacement must quote the version the stored policy
//!   is at, a put that quotes none is refused once a policy exists, and a put
//!   that quotes a version token the policy is not at is refused — both in fixed
//!   sentences that carry no version at all, while a value that is not a version
//!   token at all is the parameter refusal every version-quoting operation of
//!   this service answers;
//! * a stored policy is reported CANONICALIZED to compact JSON, with every
//!   value, member order, escape and non-ASCII character as submitted.
//!
//! The rows differ along what those rules generalise over: which element of
//! which statement is missing, how many statements the document holds, where a
//! quoted version comes from, how many writers race for one policy, and how the
//! submitted text is laid out.

use serde_json::{json, Value};

// The service's own sentences and version-token predicate, so this suite and the
// handlers cannot drift apart on what the service says or on what a version is.
use service_com_amazonaws_kafka_kafka::state::{
    is_current_version_token, BAD_REQUEST_MESSAGE, CLUSTER_POLICY_MEMBER,
    CLUSTER_POLICY_VERSION_NOT_CURRENT_MESSAGE, CLUSTER_POLICY_VERSION_REQUIRED_MESSAGE,
    CURRENT_VERSION_MEMBER, RESOURCE_NOT_FOUND_MESSAGE,
};

use super::kafka_harness::{pct, ClusterRequest, Emulator, Reply};

/// The resource URI of one cluster's policy: the model binds all three policy
/// operations to `/v1/clusters/{ClusterArn}/policy`, with the ARN
/// percent-encoded into the single `@httpLabel` path segment.
fn policy_uri(arn: &str) -> String {
    format!("/v1/clusters/{}/policy", pct(arn))
}

/// `PutClusterPolicy`. `current_version` is `None` where the request omits the
/// member, which is how the first attachment is written.
async fn put(emulator: &Emulator, arn: &str, policy: &str, current_version: Option<&str>) -> Reply {
    let mut body = json!({ "policy": policy });
    if let Some(version) = current_version {
        body["currentVersion"] = json!(version);
    }
    emulator.call("PUT", &policy_uri(arn), Some(body)).await
}

/// `GetClusterPolicy`.
async fn get(emulator: &Emulator, arn: &str) -> Reply {
    emulator.call("GET", &policy_uri(arn), None).await
}

/// The policy document and the version a cluster's stored policy is at right
/// now, as the reader reports them.
async fn stored(emulator: &Emulator, case: &str, arn: &str) -> (String, String) {
    let read = get(emulator, arn).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected the stored policy to be readable, got {} {}",
        read.status, read.body
    );
    let document = read
        .at("/policy")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: GetClusterPolicy reported no policy: {}", read.body))
        .to_string();
    let version = read
        .at("/currentVersion")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: GetClusterPolicy reported no currentVersion: {}",
                read.body
            )
        })
        .to_string();
    (document, version)
}

/// Attach `document` as the FIRST policy of the cluster — the one put that needs
/// no version — and hand back the version it answered with.
async fn attach(emulator: &Emulator, case: &str, arn: &str, document: &str) -> String {
    let reply = put(emulator, arn, document, None).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (200, ""),
        "case {case}: expected the first attachment to be stored, got {} {}",
        reply.status,
        reply.body
    );
    reply
        .at("/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: PutClusterPolicy answered no currentVersion"))
        .to_string()
}

/// Assert `reply` is HTTP 400 `BadRequestException` carrying `message` as its
/// WHOLE text and naming `member` as the parameter at fault.
fn assert_refused(reply: &Reply, case: &str, member: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected the whole fixed sentence {message:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(member),
        "case {case}: the refusal must name {member} as the parameter at fault, got {}",
        reply.body
    );
}

/// A principal a cluster policy may name: an account root of ANOTHER account,
/// which is what a cluster policy is attached to grant.
const PRINCIPAL: &str = "arn:aws:iam::111122223333:root";

/// A well-formed one-statement cluster policy, as the positive control of every
/// suite below: it declares a version the policy language defines and a
/// statement carrying an effect, a principal and an action.
fn valid_policy(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": { "AWS": PRINCIPAL },
            "Action": ["kafka:CreateVpcConnection"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// One row of the grammar table: what a failure calls it, the document
/// submitted, and the WHOLE sentence the service refuses it with.
struct Malformed {
    case: &'static str,
    document: String,
    message: &'static str,
}

/// The documents that are not a cluster policy, as rows.
///
/// The elements a statement must carry are the IAM JSON policy language's for a
/// RESOURCE-based policy — an `Effect` of `Allow`/`Deny`, an action and a
/// principal — plus the one top-level element the language requires
/// (`Statement`); the sentence naming the one a document OMITS is AWS's policy
/// validator's ("Missing required field <Element>"), after MSK's own opening.
/// An element whose VALUE the language does not define is the other class: such
/// a document is one the policy parser cannot read rather than one missing an
/// element, so it earns the service's generic parameter sentence, like any other
/// body it cannot read as a policy.
///
/// The rows differ along WHICH element is missing, WHICH statement of a
/// multi-statement document omits it, and whether the element is absent or
/// declares a value the language does not define.
fn malformed_rows() -> Vec<Malformed> {
    let statement = |members: Value| {
        json!({ "Version": "2012-10-17", "Statement": [members] }).to_string()
    };
    vec![
        Malformed {
            case: "a statement omitting Effect",
            document: statement(json!({
                "Sid": "no-effect",
                "Principal": { "AWS": PRINCIPAL },
                "Action": ["kafka:GetBootstrapBrokers"],
                "Resource": "*",
            })),
            message: "The cluster policy is not valid. Missing required field Effect",
        },
        Malformed {
            // DECLARED, but no spelling of either word the language defines: a
            // value the policy parser cannot read rather than an element the
            // statement left out, so it is not worded as a missing field.
            case: "a statement declaring an Effect the language does not define",
            document: statement(json!({
                "Effect": "Permit",
                "Principal": { "AWS": PRINCIPAL },
                "Action": ["kafka:GetBootstrapBrokers"],
            })),
            message: BAD_REQUEST_MESSAGE,
        },
        Malformed {
            case: "the SECOND statement of a two-statement document omitting Effect",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "first",
                        "Effect": "Allow",
                        "Principal": { "AWS": PRINCIPAL },
                        "Action": ["kafka:DescribeCluster"],
                    },
                    {
                        "Sid": "second",
                        "Principal": { "AWS": PRINCIPAL },
                        "Action": ["kafka:DescribeClusterV2"],
                    },
                ],
            })
            .to_string(),
            message: "The cluster policy is not valid. Missing required field Effect",
        },
        Malformed {
            case: "a statement naming no action",
            document: statement(json!({
                "Effect": "Allow",
                "Principal": { "AWS": PRINCIPAL },
                "Resource": "*",
            })),
            message: "The cluster policy is not valid. Missing required field Action",
        },
        Malformed {
            case: "a statement whose action element names nothing",
            document: statement(json!({
                "Effect": "Allow",
                "Principal": { "AWS": PRINCIPAL },
                "Action": [],
            })),
            message: "The cluster policy is not valid. Missing required field Action",
        },
        Malformed {
            case: "a statement naming no principal",
            document: statement(json!({
                "Effect": "Allow",
                "Action": ["kafka:GetBootstrapBrokers"],
                "Resource": "*",
            })),
            message: "The cluster policy is not valid. Missing required field Principal",
        },
        Malformed {
            case: "a document carrying no Statement element",
            document: json!({ "Version": "2012-10-17", "Id": "nothing-stated" }).to_string(),
            message: "The cluster policy is not valid. Missing required field Statement",
        },
        Malformed {
            // The element IS carried, so it is not one the document omits: it
            // simply states no permission, which is a document the language
            // cannot read as a policy rather than a missing element.
            case: "a Statement element naming no statement",
            document: json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
            message: BAD_REQUEST_MESSAGE,
        },
        Malformed {
            case: "a Version the policy language does not define",
            document: json!({
                "Version": "2020-01-01",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": PRINCIPAL },
                    "Action": ["kafka:GetBootstrapBrokers"],
                }],
            })
            .to_string(),
            message:
                "The cluster policy is not valid. The policy must contain a valid version string",
        },
    ]
}

/// RULE: a document that breaks the IAM JSON policy language's grammar is not a
/// cluster policy — it is refused HTTP 400 `BadRequestException` naming `Policy`
/// and the element it omits, and the cluster's policy is left exactly as the
/// refused call found it (none at all, or the one already attached).
#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused_and_attaches_nothing() {
    let emulator = Emulator::new();

    // Refused both before anything is attached and against a cluster that
    // already carries a policy: it is the document that refuses, not the
    // cluster's state. A row with a policy attached quotes NO version, so the
    // document's own fault is what is reported rather than the missing lock.
    for attached in [false, true] {
        let cluster = emulator
            .create_ok(&ClusterRequest::named(&format!(
                "policy-grammar-attached-{attached}"
            )))
            .await;
        let held = valid_policy("already-attached");
        let held_version = if attached {
            Some(attach(&emulator, "setup", &cluster, &held).await)
        } else {
            None
        };

        for Malformed {
            case,
            document,
            message,
        } in malformed_rows()
        {
            let case = format!("{case} (attached={attached})");
            let reply = put(&emulator, &cluster, &document, None).await;
            assert_refused(&reply, &case, CLUSTER_POLICY_MEMBER, message);
            // Nothing of the submitted document reaches the sentence: the
            // statement ids above are the values a composed message would quote.
            for quoted in ["no-effect", "second", "nothing-stated", "Permit", PRINCIPAL] {
                assert!(
                    !reply.error_message().contains(quoted),
                    "case {case}: the refusal must not quote {quoted:?}, got {:?}",
                    reply.error_message()
                );
            }

            match held_version.as_deref() {
                // A policy was attached: the refused put replaced neither the
                // document nor its version.
                Some(version) => {
                    let (document, at) = stored(&emulator, &case, &cluster).await;
                    assert_eq!(
                        (document.as_str(), at.as_str()),
                        (held.as_str(), version),
                        "case {case}: a refused put must leave the stored policy alone"
                    );
                }
                // Nothing was attached, so the policy the request addressed
                // still does not exist.
                None => {
                    let read = get(&emulator, &cluster).await;
                    assert_eq!(
                        (read.status, read.error_code(), read.error_message()),
                        (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
                        "case {case}: after the refusal no policy may be attached, got {} {}",
                        read.status,
                        read.body
                    );
                }
            }
        }
    }

    // The control: a document that IS a policy attaches, so the rows above are
    // about the grammar and not about the attach being broken.
    let cluster = emulator
        .create_ok(&ClusterRequest::named("policy-grammar-control"))
        .await;
    let document = valid_policy("control");
    attach(&emulator, "control", &cluster, &document).await;
    let (read, _) = stored(&emulator, "control", &cluster).await;
    assert_eq!(
        read, document,
        "case control: the well-formed document must be the stored policy"
    );

    // And a document the language lets OMIT `Version` is one too: the version
    // element is optional in the IAM policy language, so only a version outside
    // the ones it defines is refused.
    let bare = emulator
        .create_ok(&ClusterRequest::named("policy-grammar-no-version"))
        .await;
    let versionless = json!({
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": PRINCIPAL },
            "Action": ["kafka:DescribeCluster"],
        }],
    })
    .to_string();
    let next = attach(&emulator, "no-version", &bare, &versionless).await;
    let replaced = put(&emulator, &bare, &versionless, Some(&next)).await;
    assert_eq!(
        replaced.status, 200,
        "case no-version: a document declaring no Version must be accepted, got {} {}",
        replaced.status, replaced.body
    );
}

/// RULE: once a cluster carries a policy the put that replaces it is
/// version-gated — a request quoting NO version is refused HTTP 400
/// `BadRequestException` with the service's fixed sentence and replaces nothing,
/// while one quoting the version the stored policy is at replaces the document
/// and advances the version.
#[tokio::test]
async fn a_replacement_must_quote_the_version_the_stored_policy_is_at() {
    let emulator = Emulator::new();

    // Both cluster kinds carry a policy, and the rule is the cluster's policy
    // rather than the cluster's kind.
    for kind in ["provisioned", "serverless"] {
        let name = format!("policy-lock-{kind}");
        let cluster = if kind == "provisioned" {
            emulator.create_ok(&ClusterRequest::named(&name)).await
        } else {
            emulator
                .create_serverless_ok(&name, &[vec!["subnet-0a1b2c3d"]], None)
                .await
        };

        let first = valid_policy("round-0");
        let mut version = attach(&emulator, kind, &cluster, &first).await;
        let mut document = first;
        let mut seen = vec![version.clone()];

        // Several rounds, so the rule holds for every revision rather than only
        // for the second put.
        for round in 1..4u32 {
            let case = format!("{kind}/round-{round}");
            let replacement = valid_policy(&format!("round-{round}"));

            let versionless = put(&emulator, &cluster, &replacement, None).await;
            assert_refused(
                &versionless,
                &case,
                CURRENT_VERSION_MEMBER,
                CLUSTER_POLICY_VERSION_REQUIRED_MESSAGE,
            );
            let (held, at) = stored(&emulator, &case, &cluster).await;
            assert_eq!(
                (held.as_str(), at.as_str()),
                (document.as_str(), version.as_str()),
                "case {case}: the version-less put must leave the stored policy untouched"
            );

            let replaced = put(&emulator, &cluster, &replacement, Some(&version)).await;
            assert_eq!(
                (replaced.status, replaced.error_code()),
                (200, ""),
                "case {case}: a put quoting the stored version must be served, got {} {}",
                replaced.status,
                replaced.body
            );
            let next = replaced
                .at("/currentVersion")
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: the put answered no currentVersion"))
                .to_string();
            assert!(
                !seen.contains(&next),
                "case {case}: a replacement must advance the version, {next} repeats {seen:?}"
            );
            let (held, at) = stored(&emulator, &case, &cluster).await;
            assert_eq!(
                (held.as_str(), at.as_str()),
                (replacement.as_str(), next.as_str()),
                "case {case}: the replacement must be the stored policy"
            );

            seen.push(next.clone());
            version = next;
            document = replacement;
        }
    }
}

/// RULE: a put whose `CurrentVersion` is a version token the stored policy is
/// not at is refused with ONE fixed sentence, whatever the quoted token is and
/// whether or not a policy is there to be at a version — and neither the quoted
/// version nor the stored one appears in it. A value that is not a version token
/// at all is the service's generic PARAMETER refusal instead, as it is wherever
/// this service reads a version.
#[tokio::test]
async fn a_version_that_is_not_the_stored_one_is_refused_in_the_services_fixed_words() {
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("policy-stale-version"))
        .await;
    let document = valid_policy("stale");

    // Quoted before any policy exists: there is no version to be at, so the same
    // sentence answers it and nothing is attached.
    let premature = put(&emulator, &cluster, &document, Some("K0000000000000")).await;
    assert_refused(
        &premature,
        "no policy attached",
        CURRENT_VERSION_MEMBER,
        CLUSTER_POLICY_VERSION_NOT_CURRENT_MESSAGE,
    );
    let read = get(&emulator, &cluster).await;
    assert_eq!(
        (read.status, read.error_code()),
        (404, "NotFoundException"),
        "case no policy attached: the refused put must attach nothing, got {} {}",
        read.status,
        read.body
    );

    let first = attach(&emulator, "stale", &cluster, &document).await;
    let replacement = valid_policy("replacement");
    let second = put(&emulator, &cluster, &replacement, Some(&first))
        .await
        .at("/currentVersion")
        .as_str()
        .expect("the replacement answered a version")
        .to_string();

    // The version of ANOTHER cluster's policy: a token this emulator really
    // minted, and still not the version THIS cluster's stored policy is at.
    let elsewhere = emulator
        .create_ok(&ClusterRequest::named("policy-stale-elsewhere"))
        .await;
    let other_version = attach(&emulator, "elsewhere", &elsewhere, &document).await;

    // The rows differ along where the quoted version comes from: a token of the
    // service's own shape that nothing is at, the version the policy was at one
    // revision ago, and the version of another policy. Each is a token the
    // service could have minted, which is what makes the stale-version sentence
    // the answer rather than the parameter refusal below; the shape is asserted
    // through the service's own predicate, never assumed.
    let stale: Vec<(&str, String)> = vec![
        ("a token nothing is at", "K0000000000000".to_string()),
        ("the previous revision's version", first.clone()),
        ("another policy's version", other_version.clone()),
    ];
    for (case, quoted) in &stale {
        assert!(
            is_current_version_token(quoted),
            "case {case}: the quoted value must be a token of the service's shape, got {quoted:?}"
        );
        let reply = put(&emulator, &cluster, &valid_policy("clobber"), Some(quoted)).await;
        assert_refused(
            &reply,
            case,
            CURRENT_VERSION_MEMBER,
            CLUSTER_POLICY_VERSION_NOT_CURRENT_MESSAGE,
        );
        for leaked in [quoted.as_str(), second.as_str(), first.as_str()] {
            assert!(
                !reply.error_message().contains(leaked),
                "case {case}: the refusal must state no version, got {:?}",
                reply.error_message()
            );
        }
        let (held, at) = stored(&emulator, case, &cluster).await;
        assert_eq!(
            (held.as_str(), at.as_str()),
            (replacement.as_str(), second.as_str()),
            "case {case}: a refused put must leave the stored policy alone"
        );
    }

    // The OTHER class: a value that is not a version token of this service at
    // all names no version of anything, so it is the service's generic parameter
    // refusal naming the member the value travelled in — the same rule every
    // operation of this service that quotes a version applies. The rows differ
    // along HOW the value fails to be a token: its prefix, its case, its
    // alphabet and its length, one under and well over the one the service
    // mints.
    let not_tokens = [
        ("empty", ""),
        ("prose", "K0THISVERSIONISWRONG"),
        ("the token in lower case", "k0000000000000"),
        ("a character outside the alphabet", "K000000000000-"),
        ("another prefix", "X0000000000000"),
        ("one character short", "K000000000000"),
        ("far longer than a token", "KKKKKKKKKKKKKKKKKKKKKKKKKKKK"),
    ];
    for (case, quoted) in not_tokens {
        assert!(
            !is_current_version_token(quoted),
            "case {case}: the row must not be a token of the service's shape"
        );
        let reply = put(&emulator, &cluster, &valid_policy("clobber"), Some(quoted)).await;
        assert_refused(&reply, case, CURRENT_VERSION_MEMBER, BAD_REQUEST_MESSAGE);
        let (held, at) = stored(&emulator, case, &cluster).await;
        assert_eq!(
            (held.as_str(), at.as_str()),
            (replacement.as_str(), second.as_str()),
            "case {case}: a refused put must leave the stored policy alone"
        );
    }
}

/// RULE (the same lock, contended): when several writers replace one policy
/// quoting the SAME version, exactly one of them wins and every loser is refused
/// with that one fixed sentence — so a compare-and-set loop sees the conflict.
#[tokio::test]
async fn concurrent_replacements_leave_one_winner_and_every_loser_one_sentence() {
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("policy-contended"))
        .await;
    let version = attach(&emulator, "contended", &cluster, &valid_policy("base")).await;

    // Four writers, each with its own document, all quoting the version the
    // stored policy is at when they start.
    let documents: Vec<String> = (0..4)
        .map(|writer| valid_policy(&format!("writer-{writer}")))
        .collect();
    let (first, second, third, fourth) = tokio::join!(
        put(&emulator, &cluster, &documents[0], Some(&version)),
        put(&emulator, &cluster, &documents[1], Some(&version)),
        put(&emulator, &cluster, &documents[2], Some(&version)),
        put(&emulator, &cluster, &documents[3], Some(&version)),
    );

    let replies = [first, second, third, fourth];
    let winners: Vec<usize> = replies
        .iter()
        .enumerate()
        .filter(|(_, reply)| reply.status == 200)
        .map(|(writer, _)| writer)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "case contended: exactly one writer may win, got {winners:?} of {:?}",
        replies.iter().map(|reply| reply.status).collect::<Vec<_>>()
    );
    for (writer, reply) in replies.iter().enumerate() {
        if winners.contains(&writer) {
            continue;
        }
        assert_refused(
            reply,
            &format!("loser {writer}"),
            CURRENT_VERSION_MEMBER,
            CLUSTER_POLICY_VERSION_NOT_CURRENT_MESSAGE,
        );
    }

    // The winner's document is the stored one, at a version none of the four
    // quoted.
    let winner = winners[0];
    let (held, at) = stored(&emulator, "contended", &cluster).await;
    assert_eq!(
        held,
        documents[winner],
        "case contended: the stored policy must be the winner's document"
    );
    assert_ne!(
        at, version,
        "case contended: the winning replacement must advance the version"
    );
}

/// One layout row: what a failure calls it, the text submitted, and the text the
/// reader must report.
struct Layout {
    case: &'static str,
    submitted: String,
    reported: &'static str,
}

/// RULE: a stored policy is reported canonicalized to COMPACT JSON — the
/// insignificant whitespace between its JSON tokens dropped — with every value,
/// member order, escape, non-ASCII character and number spelling exactly as
/// submitted, and every statement intact and in order.
#[tokio::test]
async fn a_stored_policy_is_reported_as_canonical_compact_json() {
    let emulator = Emulator::new();

    // The one document every layout row carries, written compactly: the member
    // order is NOT the order a re-serialization would sort the members into
    // (`Version` before `Statement`, `Sid` before `Effect` before `Principal`),
    // so a row that reads it back unchanged shows the order survived.
    let compact = concat!(
        r#"{"Version":"2012-10-17","Statement":[{"Sid":"One","Effect":"Allow","#,
        r#""Principal":{"AWS":"arn:aws:iam::111122223333:root"},"#,
        r#""Action":"kafka:GetBootstrapBrokers","Resource":"*"}]}"#
    );
    // The same document laid out as a client that pretty-prints its JSON sends
    // it: tabs and newlines, three-space indentation, spaces after each
    // separator, and whitespace around the whole document.
    let tabbed = concat!(
        "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n",
        "\t\t\t\"Sid\": \"One\",\n\t\t\t\"Effect\": \"Allow\",\n",
        "\t\t\t\"Principal\": {\n\t\t\t\t\"AWS\": \"arn:aws:iam::111122223333:root\"\n\t\t\t},\n",
        "\t\t\t\"Action\": \"kafka:GetBootstrapBrokers\",\n\t\t\t\"Resource\": \"*\"\n",
        "\t\t}\n\t]\n}"
    );
    let indented = concat!(
        "{\r\n   \"Version\" : \"2012-10-17\",\r\n   \"Statement\" : [ {\r\n",
        "      \"Sid\" : \"One\",\r\n      \"Effect\" : \"Allow\",\r\n",
        "      \"Principal\" : { \"AWS\" : \"arn:aws:iam::111122223333:root\" },\r\n",
        "      \"Action\" : \"kafka:GetBootstrapBrokers\",\r\n      \"Resource\" : \"*\"\r\n",
        "   } ]\r\n}"
    );

    // A document whose values carry non-ASCII text, two spaces that belong to
    // the value, and the escapes a JSON string spells a tab and a newline with:
    // the whitespace INSIDE a string literal is part of the value, so none of it
    // is canonicalized away and no escape is rewritten.
    let unicode_submitted = concat!(
        "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [{\n",
        "\t\t\"Sid\": \"UnicodeAccess\",\n\t\t\"Effect\": \"Allow\",\n",
        "\t\t\"Principal\": { \"AWS\": \"arn:aws:iam::111122223333:root\" },\n",
        "\t\t\"Action\": \"kafka:GetBootstrapBrokers\",\n\t\t\"Resource\": \"*\",\n",
        "\t\t\"Condition\": { \"StringLike\": ",
        "{ \"aws:userid\": \"h\u{e9}llo-\u{4e16}\u{754c}-\u{394}\u{3bf}\u{3ba}\u{3b9}\u{3bc}\u{3ae}  test\\t\\n\" } }\n",
        "\t}]\n}"
    );
    let unicode_reported = concat!(
        r#"{"Version":"2012-10-17","Statement":[{"Sid":"UnicodeAccess","Effect":"Allow","#,
        r#""Principal":{"AWS":"arn:aws:iam::111122223333:root"},"#,
        r#""Action":"kafka:GetBootstrapBrokers","Resource":"*","Condition":{"StringLike":"#,
        "{\"aws:userid\":\"h\u{e9}llo-\u{4e16}\u{754c}-\u{394}\u{3bf}\u{3ba}\u{3b9}\u{3bc}\u{3ae}  test\\t\\n\"}}}]}"
    );

    // Four statements, in an order a reader must keep, one of them carrying a
    // numeric condition value: a number is reported with the digits it was
    // written with rather than respelled.
    let many_submitted = concat!(
        "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n",
        "    { \"Sid\": \"a\", \"Effect\": \"Allow\",\n",
        "      \"Principal\": { \"AWS\": \"arn:aws:iam::111122223333:root\" },\n",
        "      \"Action\": \"kafka:DescribeCluster\" },\n",
        "    { \"Sid\": \"b\", \"Effect\": \"Deny\",\n",
        "      \"Principal\": \"*\",\n",
        "      \"Action\": [ \"kafka:DeleteCluster\" ] },\n",
        "    { \"Sid\": \"c\", \"Effect\": \"Allow\",\n",
        "      \"Principal\": { \"AWS\": \"arn:aws:iam::111122223333:root\" },\n",
        "      \"Action\": \"kafka:GetBootstrapBrokers\",\n",
        "      \"Condition\": { \"NumericLessThan\": { \"aws:MultiFactorAuthAge\": 1e3 } } },\n",
        "    { \"Sid\": \"d\", \"Effect\": \"Allow\",\n",
        "      \"Principal\": { \"AWS\": \"arn:aws:iam::111122223333:root\" },\n",
        "      \"Action\": \"kafka:DescribeClusterV2\" }\n  ]\n}"
    );
    let many_reported = concat!(
        r#"{"Version":"2012-10-17","Statement":["#,
        r#"{"Sid":"a","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"#,
        r#""Action":"kafka:DescribeCluster"},"#,
        r#"{"Sid":"b","Effect":"Deny","Principal":"*","Action":["kafka:DeleteCluster"]},"#,
        r#"{"Sid":"c","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"#,
        r#""Action":"kafka:GetBootstrapBrokers","#,
        r#""Condition":{"NumericLessThan":{"aws:MultiFactorAuthAge":1e3}}},"#,
        r#"{"Sid":"d","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"#,
        r#""Action":"kafka:DescribeClusterV2"}]}"#
    );

    let rows: [Layout; 6] = [
        Layout {
            case: "a document already compact",
            submitted: compact.to_string(),
            reported: compact,
        },
        Layout {
            case: "tabs, newlines and spaces after each separator",
            submitted: tabbed.to_string(),
            reported: compact,
        },
        Layout {
            case: "three-space indentation across CRLF lines",
            submitted: indented.to_string(),
            reported: compact,
        },
        Layout {
            case: "wrapped in whitespace a parser ignores",
            submitted: format!(" \t\r\n{compact} \n"),
            reported: compact,
        },
        Layout {
            case: "non-ASCII values, inner spaces and escaped control characters",
            submitted: unicode_submitted.to_string(),
            reported: unicode_reported,
        },
        Layout {
            case: "four statements and a numeric condition value",
            submitted: many_submitted.to_string(),
            reported: many_reported,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let cluster = emulator
            .create_ok(&ClusterRequest::named(&format!("policy-canonical-{index}")))
            .await;
        let version = attach(&emulator, row.case, &cluster, &row.submitted).await;
        let (held, at) = stored(&emulator, row.case, &cluster).await;
        assert_eq!(
            held, row.reported,
            "case {}: expected the canonical compact document, got {held:?}",
            row.case
        );
        assert_eq!(
            at, version,
            "case {}: the reader must report the version the put answered",
            row.case
        );

        // A fixed point: putting back what was read reports the same bytes, so a
        // client that round-trips the policy sees no drift.
        let again = put(&emulator, &cluster, &held, Some(&version)).await;
        assert_eq!(
            again.status, 200,
            "case {}: the reported document must itself be storable, got {} {}",
            row.case, again.status, again.body
        );
        let (round_tripped, _) = stored(&emulator, row.case, &cluster).await;
        assert_eq!(
            round_tripped, row.reported,
            "case {}: the reported form must be a fixed point, got {round_tripped:?}",
            row.case
        );
    }
}
