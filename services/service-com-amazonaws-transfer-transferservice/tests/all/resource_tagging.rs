// SPDX-License-Identifier: CC0-1.0

//! The tag plane: `TagResource`, `UntagResource` and `ListTagsForResource`.
//!
//! RULE under test: a resource's tags are a key-value MAP owned by the resource the
//! request's `Arn` identifies. `TagResource` sets the value of each key it sends —
//! replacing the value of a key the resource already carries, adding one it does not —
//! `UntagResource` detaches exactly the keys it names, and `ListTagsForResource`
//! reports the resulting set, the same set the resource's own `Describe*` reports. An
//! `Arn` no stored resource answers to is refused by both writes with the modelled
//! `ResourceNotFoundException`.
//!
//! The rows differ along what that rule generalises over: the resource KIND the ARN
//! addresses (every taggable kind this service stores), which keys a write touches and
//! whether they are already present, the key and value lengths (including the modelled
//! bounds `TagKey` `len 0..128` and `TagValue` `len 0..256`), the number of pairs per
//! request, the page bound, and WHY an ARN resolves to nothing (never minted, another
//! region, another account, another service, deleted).
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    drain_pages, generated_host_key, listed_tags, member_of, tag_map, Emulator, ACCOUNT, REGION,
    ROLE,
};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s
/// modelled pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// A `ProfileId` (`len 19..19`, pattern `^p-([0-9a-f]{17})$`) for a connector's local
/// AS2 profile.
const LOCAL_PROFILE: &str = "p-0123456789abcdef0";

/// The partner side of the same AS2 connector.
const PARTNER_PROFILE: &str = "p-00000000000000001";

/// Every resource kind this service stores a tag set for, i.e. every kind whose record
/// carries an `Arn` and a `Tags` of its own (`src/state.rs`). Each one is created over
/// the wire by [`create_resource`].
const TAGGABLE_KINDS: [&str; 7] = [
    "server",
    "user",
    "host key",
    "web app",
    "AS2 profile",
    "connector",
    "workflow",
];

/// `MaxResults`' modelled maximum (`range 1..1000`), which is also the page bound a
/// call that sends no `MaxResults` gets.
const MAX_PAGE_SIZE: usize = 1000;

/// One taggable resource, created over the wire: the ARN the tag plane addresses it by,
/// and the call that reads the resource's OWN tag set back.
struct Resource {
    arn: String,
    describe_op: &'static str,
    describe_request: Value,
    describe_member: &'static str,
}

/// A `Tags` request member from `pairs`.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// `pairs` as the key -> value map a tag set is.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Read one resource back through its own reader and hand back the described structure.
async fn describe(emulator: &Emulator, case: &str, resource: &Resource) -> Value {
    let answer = emulator
        .call(resource.describe_op, resource.describe_request.clone())
        .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected {} 200, got {} with body {}",
        resource.describe_op,
        answer.status,
        answer.body
    );
    member_of(&answer.body, resource.describe_member).clone()
}

/// Create a resource of `kind` and resolve the ARN its own reader reports for it.
async fn create_resource(emulator: &Emulator, kind: &'static str) -> Resource {
    let (describe_op, describe_request, describe_member) = match kind {
        "server" => {
            let server_id = emulator.create_server(json!({})).await;
            ("DescribeServer", json!({"ServerId": server_id}), "Server")
        }
        "user" => {
            let server_id = emulator.create_server(json!({})).await;
            emulator.create_user(&server_id, "tagged-user").await;
            (
                "DescribeUser",
                json!({"ServerId": server_id, "UserName": "tagged-user"}),
                "User",
            )
        }
        "host key" => {
            let server_id = emulator.create_server(json!({})).await;
            let (material, _fingerprint) = generated_host_key();
            let answer = emulator
                .call(
                    "ImportHostKey",
                    json!({"ServerId": &server_id, "HostKeyBody": material}),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {kind:?}: expected ImportHostKey 200, got {} with body {}",
                answer.status, answer.body
            );
            let host_key_id = answer
                .get("HostKeyId")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {kind:?}: ImportHostKey returned no HostKeyId: {}", answer.body)
                })
                .to_string();
            (
                "DescribeHostKey",
                json!({"ServerId": server_id, "HostKeyId": host_key_id}),
                "HostKey",
            )
        }
        "web app" => {
            let answer = emulator
                .call(
                    "CreateWebApp",
                    json!({
                        "IdentityProviderDetails": {
                            "IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE},
                        },
                    }),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {kind:?}: expected CreateWebApp 200, got {} with body {}",
                answer.status, answer.body
            );
            let web_app_id = answer
                .get("WebAppId")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {kind:?}: CreateWebApp returned no WebAppId: {}", answer.body)
                })
                .to_string();
            ("DescribeWebApp", json!({"WebAppId": web_app_id}), "WebApp")
        }
        "AS2 profile" => {
            let profile_id = emulator
                .create_profile(json!({"As2Id": "tagged-as2", "ProfileType": "LOCAL"}))
                .await;
            ("DescribeProfile", json!({"ProfileId": profile_id}), "Profile")
        }
        "connector" => {
            let answer = emulator
                .call(
                    "CreateConnector",
                    json!({
                        "AccessRole": ROLE,
                        "Url": "https://partner.example.com/as2",
                        "As2Config": {
                            "LocalProfileId": LOCAL_PROFILE,
                            "PartnerProfileId": PARTNER_PROFILE,
                            "Compression": "DISABLED",
                            "EncryptionAlgorithm": "NONE",
                            "SigningAlgorithm": "NONE",
                        },
                    }),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {kind:?}: expected CreateConnector 200, got {} with body {}",
                answer.status, answer.body
            );
            let connector_id = answer
                .get("ConnectorId")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {kind:?}: CreateConnector returned no ConnectorId: {}", answer.body)
                })
                .to_string();
            (
                "DescribeConnector",
                json!({"ConnectorId": connector_id}),
                "Connector",
            )
        }
        "workflow" => {
            let answer = emulator
                .call(
                    "CreateWorkflow",
                    json!({"Steps": [{"Type": "DELETE", "DeleteStepDetails": {"Name": "delete-step"}}]}),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {kind:?}: expected CreateWorkflow 200, got {} with body {}",
                answer.status, answer.body
            );
            let workflow_id = answer
                .get("WorkflowId")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {kind:?}: CreateWorkflow returned no WorkflowId: {}", answer.body)
                })
                .to_string();
            (
                "DescribeWorkflow",
                json!({"WorkflowId": workflow_id}),
                "Workflow",
            )
        }
        other => panic!("{other} is not a taggable resource kind in this service"),
    };

    let mut resource = Resource {
        arn: String::new(),
        describe_op,
        describe_request,
        describe_member,
    };
    let described = describe(emulator, kind, &resource).await;
    resource.arn = member_of(&described, "Arn")
        .as_str()
        .unwrap_or_else(|| panic!("case {kind:?}: the created resource reports no Arn: {described}"))
        .to_string();
    resource
}

/// Attach `tags` to `arn`, asserting the write succeeded.
async fn tag(emulator: &Emulator, case: &str, arn: &str, tags: Value) {
    let answer = emulator
        .call("TagResource", json!({"Arn": arn, "Tags": tags}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected TagResource 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// Detach `keys` from `arn`, asserting the write succeeded.
async fn untag(emulator: &Emulator, case: &str, arn: &str, keys: Value) {
    let answer = emulator
        .call("UntagResource", json!({"Arn": arn, "TagKeys": keys}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UntagResource 200, got {} with body {}",
        answer.status, answer.body
    );
}

// ---------------------------------------------------------------------------
// Rule 1: the tag set belongs to the resource the ARN names, whatever its kind.
// ---------------------------------------------------------------------------

/// RULE: for EVERY taggable resource kind, `TagResource` writes the tag set of the
/// resource its `Arn` identifies and `UntagResource` detaches from that same set, and
/// both `ListTagsForResource` and the resource's own reader report one and the same set.
///
/// The rows are the resource kinds; the ARN each is addressed by is the one that
/// resource's own reader reports, never one the test formats.
#[tokio::test]
async fn every_taggable_resource_kind_round_trips_its_tag_set() {
    let emulator = Emulator::new();

    for kind in TAGGABLE_KINDS {
        let resource = create_resource(&emulator, kind).await;
        let arn = resource.arn.clone();

        // A resource created without tags carries none.
        let listed = listed_tags(&emulator, kind, &arn).await;
        assert!(
            listed.is_empty(),
            "case {kind:?}: expected no tags on a freshly created resource, got {listed:?}"
        );

        let attached = expected(&[("env", "prod"), ("team", "data")]);
        tag(
            &emulator,
            kind,
            &arn,
            tag_list(&[("env", "prod"), ("team", "data")]),
        )
        .await;
        let listed = listed_tags(&emulator, kind, &arn).await;
        assert_eq!(
            listed, attached,
            "case {kind:?}: expected ListTagsForResource to report {attached:?}, got {listed:?}"
        );
        let described = describe(&emulator, kind, &resource).await;
        let reported = tag_map(member_of(&described, "Tags"));
        assert_eq!(
            reported, attached,
            "case {kind:?}: expected {} to report {attached:?}, got {reported:?}",
            resource.describe_op
        );

        // One key detached leaves the rest exactly as they were.
        untag(&emulator, kind, &arn, json!(["team"])).await;
        let remaining = expected(&[("env", "prod")]);
        let listed = listed_tags(&emulator, kind, &arn).await;
        assert_eq!(
            listed, remaining,
            "case {kind:?}: expected ListTagsForResource to report {remaining:?} after one key \
             was detached, got {listed:?}"
        );
        let described = describe(&emulator, kind, &resource).await;
        let reported = tag_map(member_of(&described, "Tags"));
        assert_eq!(
            reported, remaining,
            "case {kind:?}: expected {} to report {remaining:?} after one key was detached, \
             got {reported:?}",
            resource.describe_op
        );

        // And detaching the last key empties the set.
        untag(&emulator, kind, &arn, json!(["env"])).await;
        let listed = listed_tags(&emulator, kind, &arn).await;
        assert!(
            listed.is_empty(),
            "case {kind:?}: expected no tags after every key was detached, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a tag key identifies one tag — a write sets it, an untag removes it.
// ---------------------------------------------------------------------------

/// RULE: `TagResource` sets the value of each key it carries (replacing the value of a
/// key the resource already holds, adding one it does not) and `UntagResource` detaches
/// exactly the keys it names, leaving every other tag untouched.
///
/// The rows are a SEQUENCE of writes against one resource, each stating the whole set
/// the resource must then report: they differ along how many pairs the request carries,
/// whether each key is already present, a key repeated inside one request, the key and
/// value lengths at the modelled bounds (`TagKey` `len 0..128`, `TagValue` `len 0..256`,
/// from `smithy_query(query="shape",shape="Tag")`), and keys the resource does not
/// carry.
#[tokio::test]
async fn a_tag_key_identifies_one_tag_and_a_write_sets_only_the_keys_it_names() {
    let emulator = Emulator::new();
    let resource = create_resource(&emulator, "server").await;
    let arn = resource.arn.clone();

    // `TagKey` is modelled `len 0..128` and `TagValue` `len 0..256`: the upper bounds.
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);

    let steps: Vec<(&str, &str, Value, BTreeMap<String, String>)> = vec![
        (
            "two new keys in one request",
            "TagResource",
            tag_list(&[("env", "prod"), ("team", "data")]),
            expected(&[("env", "prod"), ("team", "data")]),
        ),
        (
            "a third key added beside them",
            "TagResource",
            tag_list(&[("owner", "sftp")]),
            expected(&[("env", "prod"), ("team", "data"), ("owner", "sftp")]),
        ),
        (
            "a key already present takes the new value in place",
            "TagResource",
            tag_list(&[("env", "staging")]),
            expected(&[("env", "staging"), ("team", "data"), ("owner", "sftp")]),
        ),
        (
            "a key repeated inside one request keeps the last value sent",
            "TagResource",
            tag_list(&[("env", "first"), ("env", "last")]),
            expected(&[("env", "last"), ("team", "data"), ("owner", "sftp")]),
        ),
        (
            "an empty value (TagValue len 0)",
            "TagResource",
            tag_list(&[("owner", "")]),
            expected(&[("env", "last"), ("team", "data"), ("owner", "")]),
        ),
        (
            "an empty key (TagKey len 0)",
            "TagResource",
            tag_list(&[("", "unkeyed")]),
            expected(&[
                ("env", "last"),
                ("team", "data"),
                ("owner", ""),
                ("", "unkeyed"),
            ]),
        ),
        (
            "the longest key and value the model allows",
            "TagResource",
            tag_list(&[(&long_key, &long_value)]),
            expected(&[
                ("env", "last"),
                ("team", "data"),
                ("owner", ""),
                ("", "unkeyed"),
                (&long_key, &long_value),
            ]),
        ),
        (
            "one key detached",
            "UntagResource",
            json!(["team"]),
            expected(&[
                ("env", "last"),
                ("owner", ""),
                ("", "unkeyed"),
                (&long_key, &long_value),
            ]),
        ),
        (
            "a key the resource does not carry detaches nothing",
            "UntagResource",
            json!(["team"]),
            expected(&[
                ("env", "last"),
                ("owner", ""),
                ("", "unkeyed"),
                (&long_key, &long_value),
            ]),
        ),
        (
            "several keys at once, one of them absent",
            "UntagResource",
            json!(["env", "owner", "never-attached"]),
            expected(&[("", "unkeyed"), (&long_key, &long_value)]),
        ),
        (
            "every remaining key",
            "UntagResource",
            json!(["", long_key.clone()]),
            expected(&[]),
        ),
    ];

    for (case, op, payload, want) in steps {
        match op {
            "TagResource" => tag(&emulator, case, &arn, payload).await,
            "UntagResource" => untag(&emulator, case, &arn, payload).await,
            other => panic!("case {case:?}: {other} is not a tag write"),
        }

        let listed = listed_tags(&emulator, case, &arn).await;
        assert_eq!(
            listed, want,
            "case {case:?}: expected ListTagsForResource to report {want:?}, got {listed:?}"
        );

        // The resource's own reader answers out of the same stored set.
        let described = describe(&emulator, case, &resource).await;
        let reported = tag_map(member_of(&described, "Tags"));
        assert_eq!(
            reported, want,
            "case {case:?}: expected DescribeServer to report {want:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: an ARN nothing answers to.
// ---------------------------------------------------------------------------

/// RULE: a tag write whose `Arn` is not the stored ARN of any resource is refused with
/// the modelled `ResourceNotFoundException` — answered, like every not-found in this
/// service, with HTTP 400, the KIND the ARN names in `ResourceType` and that kind's own
/// identifier in `Resource` — and writes nothing. `ListTagsForResource` models no
/// not-found error, so the same ARN simply has no tags: HTTP 200, the ARN echoed, and an
/// EMPTY `Tags`.
///
/// The rows differ along WHY the ARN resolves to nothing: an id never minted, the right
/// id in another region or under another account (the ARN is the identity, not the id),
/// a kind the store holds no record of, an ARN of another service entirely (whose kind
/// this service cannot name, so the whole ARN is what it reports), and the ARN of a
/// resource that has been deleted.
#[tokio::test]
async fn a_tag_write_refuses_an_arn_no_stored_resource_answers_to() {
    let emulator = Emulator::new();
    let resource = create_resource(&emulator, "server").await;
    let arn = resource.arn.clone();
    let server_id = resource
        .describe_request
        .get("ServerId")
        .and_then(Value::as_str)
        .expect("the server row describes by ServerId")
        .to_string();
    tag(&emulator, "the bystander", &arn, tag_list(&[("env", "prod")])).await;

    // A deleted resource's ARN answers to nothing either.
    let doomed = create_resource(&emulator, "server").await;
    let answer = emulator
        .call(
            "DeleteServer",
            doomed.describe_request.clone(),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "DeleteServer: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    // (case, the ARN nothing answers to, the kind it names and the identifier it
    // addresses that kind by — what the refusal reports in ResourceType and Resource).
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "a ServerId the store never minted",
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/s-fffffffffffffffff"),
            "Server",
            "s-fffffffffffffffff".to_string(),
        ),
        (
            "the stored resource's id in another region",
            format!("arn:aws:transfer:eu-west-1:{ACCOUNT}:server/{server_id}"),
            "Server",
            server_id.clone(),
        ),
        (
            "the stored resource's id under another account",
            format!("arn:aws:transfer:{REGION}:111122223333:server/{server_id}"),
            "Server",
            server_id.clone(),
        ),
        (
            "a user under a server that holds no such user",
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:user/{server_id}/absent-user"),
            "User",
            "absent-user".to_string(),
        ),
        (
            "a workflow nothing answers to",
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:workflow/w-00000000000000001"),
            "Workflow",
            "w-00000000000000001".to_string(),
        ),
        (
            "an ARN of another service entirely",
            "arn:aws:s3:::tag-plane-bucket".to_string(),
            // No kind of this service: there is no identifier to decode out of another
            // service's resource field, so the whole ARN is what is reported.
            "Resource",
            "arn:aws:s3:::tag-plane-bucket".to_string(),
        ),
        (
            "the ARN of a deleted server",
            doomed.arn.clone(),
            "Server",
            doomed
                .arn
                .rsplit('/')
                .next()
                .expect("the deleted server's ARN names its id")
                .to_string(),
        ),
    ];

    for (case, missing, resource_type, identifier) in &cases {
        let writes: [(&str, Value); 2] = [
            (
                "TagResource",
                json!({"Arn": missing, "Tags": tag_list(&[("env", "prod")])}),
            ),
            (
                "UntagResource",
                json!({"Arn": missing, "TagKeys": ["env"]}),
            ),
        ];
        for (op, request) in writes {
            let answer = emulator.call(op, request).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected {op} to answer HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected {op} to answer ResourceNotFoundException, got {}",
                answer.body
            );
            let reported = member_of(&answer.body, "Resource")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                &reported, identifier,
                "case {case:?}: expected {op} to report the identifier {identifier:?} in \
                 Resource, got {reported:?}"
            );
            let reported_type = member_of(&answer.body, "ResourceType")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                &reported_type, resource_type,
                "case {case:?}: expected {op} to report ResourceType {resource_type:?}, got \
                 {reported_type:?}"
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !message.is_empty() && !message.contains(missing.as_str()),
                "case {case:?}: expected {op} to answer a non-empty Message carrying no ARN, \
                 got {message:?}"
            );
        }

        // The read models no not-found: the ARN simply has no tags.
        let answer = emulator
            .call("ListTagsForResource", json!({"Arn": missing}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListTagsForResource 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("Arn").as_str(),
            Some(missing.as_str()),
            "case {case:?}: expected the requested Arn echoed, got {}",
            answer.body
        );
        assert_eq!(
            answer.get("Tags"),
            &json!([]),
            "case {case:?}: expected an empty Tags member for an ARN nothing answers to, got {}",
            answer.body
        );
    }

    // None of those refusals touched the resource that does exist.
    let listed = listed_tags(&emulator, "the bystander", &arn).await;
    let want = expected(&[("env", "prod")]);
    assert_eq!(
        listed, want,
        "case \"the bystander\": expected the refused writes to change nothing, \
         so {want:?}, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: the listing pages its tags.
// ---------------------------------------------------------------------------

/// RULE: `ListTagsForResource` reports exactly the tags the resource holds, in `Key`
/// order, one page at a time — no more than `MaxResults` per call, a `NextToken`
/// exactly while a further page remains, and the whole set recovered by following it.
///
/// The rows are the page bound: 1, a divisor of the set size, a bound between 1 and the
/// size, the size exactly, one past it, and none at all (the `MAX_PAGE_SIZE` default).
/// The set includes the EMPTY key the model allows (`TagKey` `len 0..128`), which sorts
/// first: a `NextToken` must still be one a caller can send back (`NextToken` is
/// modelled `len 1..6144`), so paging a set whose first key is empty must terminate at
/// every page bound.
#[tokio::test]
async fn list_tags_for_resource_pages_the_tag_set_in_key_order() {
    let emulator = Emulator::new();
    let resource = create_resource(&emulator, "server").await;
    let arn = resource.arn.clone();

    let pairs: [(&str, &str); 6] = [
        ("zone", "a"),
        ("env", "prod"),
        ("", "unkeyed"),
        ("owner", "sftp"),
        ("cost-centre", "42"),
        ("app", "ingest"),
    ];
    tag(&emulator, "six tags", &arn, tag_list(&pairs)).await;

    let mut keys_in_order: Vec<String> = pairs.iter().map(|(key, _)| (*key).to_string()).collect();
    keys_in_order.sort();

    let base = json!({"Arn": &arn});
    for max_results in [Some(1), Some(2), Some(4), Some(6), Some(7), None] {
        let (page_sizes, keys) =
            drain_pages(&emulator, "ListTagsForResource", &base, "Tags", "Key", max_results).await;
        let limit = max_results.unwrap_or(MAX_PAGE_SIZE as i64) as usize;
        for size in &page_sizes {
            assert!(
                *size <= limit,
                "case {max_results:?}: expected no page beyond {limit} tags, got {page_sizes:?}"
            );
        }
        assert_eq!(
            keys, keys_in_order,
            "case {max_results:?}: expected every key once in Key order, got {keys:?}"
        );
        let expected_pages = keys_in_order.len().div_ceil(limit);
        assert_eq!(
            page_sizes.len(),
            expected_pages,
            "case {max_results:?}: expected {expected_pages} page(s), got {page_sizes:?}"
        );
    }

    // Every token the listing mints satisfies `NextToken`'s modelled `len 1..6144`, so
    // a caller can always send it back — including the one that ends a page at the
    // empty key.
    let answer = emulator
        .call("ListTagsForResource", json!({"Arn": &arn, "MaxResults": 1}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"the first page of one\": expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let token = answer.get("NextToken").as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty() && token.len() <= 6144,
        "case \"the first page of one\": expected a NextToken within the modelled \
         len 1..6144, got {token:?} with body {}",
        answer.body
    );
    // A token this service did not mint is refused rather than silently restarting the
    // listing.
    let answer = emulator
        .call(
            "ListTagsForResource",
            json!({"Arn": &arn, "NextToken": "not-a-token"}),
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "case \"an invented NextToken\": expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "InvalidNextTokenException",
        "case \"an invented NextToken\": expected InvalidNextTokenException, got {}",
        answer.body
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the tag set is bounded.
// ---------------------------------------------------------------------------

/// RULE: a resource carries at most 50 tags — the modelled `Tags` `len 1..50` every
/// reader reports the set through — so a `TagResource` that would push it past that is
/// refused with the modelled `InvalidRequestException` (HTTP 400) and writes NOTHING,
/// while a write that only re-values keys already present is accepted at the bound.
///
/// The rows differ along how far past the bound the request reaches and whether its
/// keys are new or already present.
#[tokio::test]
async fn a_tag_write_past_the_bound_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let resource = create_resource(&emulator, "server").await;
    let arn = resource.arn.clone();

    // 50 keys: `Tags` is modelled `len 1..50`, so the whole set fits in one request.
    let keys: Vec<String> = (0..50).map(|n| format!("key-{n:02}")).collect();
    let full: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "first")).collect();
    tag(&emulator, "the set at the bound", &arn, tag_list(&full)).await;
    let at_bound = expected(&full);
    let listed = listed_tags(&emulator, "the set at the bound", &arn).await;
    assert_eq!(
        listed.len(),
        50,
        "case \"the set at the bound\": expected 50 tags, got {}",
        listed.len()
    );
    assert_eq!(
        listed, at_bound,
        "case \"the set at the bound\": expected {at_bound:?}, got {listed:?}"
    );

    let refusals: Vec<(&str, Value)> = vec![
        (
            "one new key past the bound",
            tag_list(&[("one-too-many", "x")]),
        ),
        (
            "several new keys past the bound",
            tag_list(&[("a", "x"), ("b", "y"), ("c", "z")]),
        ),
        (
            "a new key beside a key already present",
            tag_list(&[(keys[0].as_str(), "second"), ("one-too-many", "x")]),
        ),
    ];
    for (case, tags) in refusals {
        let answer = emulator
            .call("TagResource", json!({"Arn": &arn, "Tags": tags}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            answer.body
        );
        let listed = listed_tags(&emulator, case, &arn).await;
        assert_eq!(
            listed, at_bound,
            "case {case:?}: expected the refused write to change nothing, got {listed:?}"
        );
    }

    // At the bound, a write that only re-values keys already present is NOT past it.
    let revalued: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "second")).collect();
    tag(
        &emulator,
        "every key re-valued at the bound",
        &arn,
        tag_list(&revalued),
    )
    .await;
    let want = expected(&revalued);
    let listed = listed_tags(&emulator, "every key re-valued at the bound", &arn).await;
    assert_eq!(
        listed, want,
        "case \"every key re-valued at the bound\": expected {want:?}, got {listed:?}"
    );

    // And releasing one key makes room for a new one.
    untag(
        &emulator,
        "one key released",
        &arn,
        json!([keys[0].clone()]),
    )
    .await;
    tag(
        &emulator,
        "one key released",
        &arn,
        tag_list(&[("one-too-many", "x")]),
    )
    .await;
    let listed = listed_tags(&emulator, "one key released", &arn).await;
    assert_eq!(
        listed.len(),
        50,
        "case \"one key released\": expected 50 tags again, got {}",
        listed.len()
    );
    assert_eq!(
        listed.get("one-too-many").map(String::as_str),
        Some("x"),
        "case \"one key released\": expected the new key attached, got {listed:?}"
    );
}
