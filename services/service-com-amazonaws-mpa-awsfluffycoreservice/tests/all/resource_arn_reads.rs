// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `ResourceArn` boundary every resource-ARN-keyed operation
//! of this service shares, and for `ListResourcePolicies`, the paginated read of the
//! policies a resource carries.
//!
//! The rule under test: `ResourceArn` is modelled as a bare `String`
//! (`@length(max: 1000)`, NO pattern), so the boundary validator passes any string and
//! the operation itself refuses a value that is not an ARN with the modelled
//! `ValidationException` (400) and the one sentence this service states for the
//! malformed-ARN class (`state::INVALID_ARN_MESSAGE`) — before the account boundary
//! (403) and before any lookup (404), so a caller can tell the three apart. The table
//! below therefore spans the ARN FIELDS the grammar
//! (`arn:<partition>:<service>:<region>:<account>:<resource>`) requires, crossed with
//! every implemented operation that takes such a member, rather than the one recorded
//! value; the contrast rows show a well-formed ARN is NOT refused this way.
//!
//! `ListResourcePolicies` is then exercised against the store: its page spans the
//! attached-policy counts `Policies` allows crossed with the page sizes `MaxResults`
//! allows, and every member of an entry is checked against the attachment the store
//! resolved, so nothing in the answer is this test's own.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON
//! envelope and the HTTP status are all exercised.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{
    encode_label, error_code, recorded_policy_document, text, Harness, REGION,
};
use crate::resource_policies::{create_body, get_resource_policy};

/// `MaxResults` is modelled `range(min: 1, max: 20)` with `default: 20`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 20;

/// `Policies` is `length(min: 1, max: 10)`, on create and on update alike.
const MAX_POLICIES: usize = 10;

/// `ResourceArn` is `length(max: 1000)` — the one bound the model does place on it.
const MAX_RESOURCE_ARN: usize = 1000;

/// The policy a fresh store seeds, from `state::seeded_policy_catalog`.
const SEEDED_POLICY: (&str, &str) = ("IdentityCenter", "MPAApprovalTeamPolicy");

/// `GET /tags/{ResourceArn}` — `ListTagsForResource`.
async fn list_tags(harness: &Harness, resource_arn: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/tags/{}", encode_label(resource_arn)), None)
        .await
}

/// `PUT /tags/{ResourceArn}` — `TagResource`.
async fn tag_resource(harness: &Harness, resource_arn: &str) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(resource_arn)),
            Some(json!({ "Tags": { "env": "test" } })),
        )
        .await
}

/// `POST /resource-policies/{ResourceArn}/?List` — `ListResourcePolicies`, with `query`
/// appended to the `?List` marker.
async fn list_resource_policies(
    harness: &Harness,
    resource_arn: &str,
    query: &str,
) -> (u16, Value) {
    let label = encode_label(resource_arn);
    let uri = if query.is_empty() {
        format!("/resource-policies/{label}/?List")
    } else {
        format!("/resource-policies/{label}/?List&{query}")
    };
    harness.call("POST", &uri, None).await
}

/// A fixed instant recorded catalog versions are dated from, so no assertion depends on
/// when the test ran.
fn base_instant() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// Record a one-version catalog entry and answer its unqualified ARN.
///
/// No modelled operation mints a policy — the catalog is AWS's — so a table that needs
/// more entries than a fresh store seeds writes them directly, as `resource_policies.rs`
/// does.
fn record_policy(harness: &Harness, name: &str, policy_type: state::PolicyType) -> String {
    let policy = state::CatalogPolicy::with_first_version(
        SEEDED_POLICY.0,
        name,
        policy_type,
        base_instant(),
        &recorded_policy_document(name, 1),
    );
    let arn = policy.arn.clone();
    let store = state::service_state(&*harness.engine).expect("the service store");
    store.lock().insert_policy(policy);
    arn
}

/// A policy name for row `n`, inside `PolicyName`'s `length(max: 64)` and the character
/// set `UnqualifiedPolicyArn`'s pattern allows.
fn policy_name(n: usize) -> String {
    format!("MPATestPolicy{n:03}")
}

/// Record `count` catalog entries and answer the attachment references a team carries
/// for them — half qualified with `$DEFAULT`, half with an explicit version id, the two
/// forms `QualifiedPolicyArn` allows, and alternating `PolicyType` so no table below
/// tests a single value of it.
fn record_attachments(harness: &Harness, count: usize) -> Vec<String> {
    (1..=count)
        .map(|n| {
            let policy_type = if n.is_multiple_of(2) {
                state::PolicyType::AwsRam
            } else {
                state::PolicyType::AwsManaged
            };
            let arn = record_policy(harness, &policy_name(n), policy_type);
            if n.is_multiple_of(2) {
                format!("{arn}/$DEFAULT")
            } else {
                state::policy_version_arn(&arn, state::FIRST_POLICY_VERSION)
            }
        })
        .collect()
}

/// Create a team attaching `policies` and answer its ARN.
async fn create_team(harness: &Harness, name: &str, policies: &[String]) -> String {
    let (status, created) = harness.create(&create_body(name, policies)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateApprovalTeam returned {status} with {created:?}"
    );
    let arn = text(&created, "Arn");
    assert!(
        !arn.is_empty(),
        "case {name:?}: CreateApprovalTeam returned no Arn: {created:?}"
    );
    arn
}

/// The resource policies the STORE resolves for `arn` — what a listed page must be
/// computed from.
fn stored_resource_policies(harness: &Harness, arn: &str) -> Vec<state::ResourcePolicy> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .resource_policies(arn)
        .expect("a stored resource to resolve policies for")
}

/// Every `ResourceArn` FORM that is not a well-formed ARN.
///
/// The grammar is `arn:<partition>:<service>:<region>:<account>:<resource>` — six
/// colon-separated fields with a non-empty partition, service and resource, the shape
/// `state::is_well_formed_arn` pins — so each row below breaks exactly one part of it.
/// The first row is the value the recorded check sends.
fn malformed_resource_arns() -> Vec<(&'static str, String)> {
    let own = engine::emulator_account_id();
    vec![
        ("not an ARN at all", "not-an-arn-at-all".to_string()),
        ("the prefix alone", "arn".to_string()),
        ("a prefix and a partition", "arn:aws".to_string()),
        ("five fields", format!("arn:aws:mpa:{REGION}:{own}")),
        ("an empty resource field", format!("arn:aws:mpa:{REGION}:{own}:")),
        (
            "an empty partition",
            format!("arn::mpa:{REGION}:{own}:approval-team/team"),
        ),
        (
            "an empty service",
            format!("arn:aws::{REGION}:{own}:approval-team/team"),
        ),
        (
            "the prefix in the wrong case",
            format!("ARN:aws:mpa:{REGION}:{own}:approval-team/team"),
        ),
        ("a bare resource id", "approval-team/team".to_string()),
        (
            // `ResourceArn` is `length(max: 1000)`: a value at that bound is still not
            // an ARN, so the length it satisfies decides nothing.
            "a value at ResourceArn's length bound that is not an ARN",
            "x".repeat(MAX_RESOURCE_ARN),
        ),
    ]
}

/// Well-formed ARNs, with the answer each earns: 403 outside the caller's account, 404
/// for an ARN this store holds no resource under. Rows span resource TYPE, REGION,
/// PARTITION and ACCOUNT — the fields a well-formed ARN varies along.
fn well_formed_resource_arns() -> Vec<(&'static str, String, u16)> {
    let own = engine::emulator_account_id();
    vec![
        (
            "an approval team that was never created",
            format!("arn:aws:mpa:{REGION}:{own}:approval-team/never-created"),
            404,
        ),
        (
            "a session, a resource type this service stores none of",
            format!("arn:aws:mpa:{REGION}:{own}:session/some-team/1a2b3c"),
            404,
        ),
        (
            "an identity source, under which nothing was created",
            format!("arn:aws:mpa:{REGION}:{own}:identity-source/ssoins-0001"),
            404,
        ),
        (
            "another region",
            format!("arn:aws:mpa:eu-central-1:{own}:approval-team/elsewhere"),
            404,
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:mpa:us-gov-west-1:{own}:approval-team/elsewhere"),
            404,
        ),
        (
            "another service's ARN in the caller's account",
            format!("arn:aws:iam:{REGION}:{own}:role/not-an-mpa-resource"),
            404,
        ),
        (
            "another account",
            format!("arn:aws:mpa:{REGION}:999999999999:approval-team/elsewhere"),
            403,
        ),
        (
            "an account-less, AWS-owned policy ARN",
            format!(
                "{}/$DEFAULT",
                state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1)
            ),
            403,
        ),
    ]
}

/// A `ResourceArn` that is not an ARN is refused with each operation's own 400 input
/// error — the same boundary across every implemented operation keyed by that member,
/// decided before the account boundary and before any lookup.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_refused_by_every_reader() {
    let harness = Harness::new();
    // A stored resource, so no refusal below can be an accident of an empty store.
    let stored = create_team(
        &harness,
        "arn-boundary-team",
        &record_attachments(&harness, 1),
    )
    .await;

    for (case, arn) in malformed_resource_arns() {
        // Every operation keyed by this member, with the answer it must give.
        let answers = [
            ("ListTagsForResource", list_tags(&harness, &arn).await),
            ("TagResource", tag_resource(&harness, &arn).await),
            (
                "ListResourcePolicies",
                list_resource_policies(&harness, &arn, "").await,
            ),
            (
                "GetResourcePolicy",
                get_resource_policy(&harness, &arn, SEEDED_POLICY.1, "AWS_MANAGED").await,
            ),
        ];
        for (operation, (status, body)) in answers {
            assert_eq!(
                status, 400,
                "case {case:?} ({arn:?}) / {operation}: expected 400, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} ({arn:?}) / {operation}: wrong error code in {body:?}"
            );
            // The malformed-ARN class is ONE service-level rule, so every operation
            // states it with the one sentence `state` owns for it.
            assert_eq!(
                text(&body, "Message"),
                state::INVALID_ARN_MESSAGE,
                "case {case:?} / {operation}: wrong refusal wording in {body:?}"
            );
        }
    }

    // The refusal is about the ARN's FORM, not about the operations: the stored
    // resource's own ARN is still answered.
    for (operation, (status, body)) in [
        ("ListTagsForResource", list_tags(&harness, &stored).await),
        (
            "ListResourcePolicies",
            list_resource_policies(&harness, &stored, "").await,
        ),
    ] {
        assert_eq!(
            status, 200,
            "{operation} must still answer a stored resource, got {status} with {body:?}"
        );
    }
}

/// A well-formed `ResourceArn` is NOT refused as malformed: it earns 403 outside the
/// caller's account and 404 when this store holds no resource under it — the two answers
/// a caller must be able to tell from the 400 above.
#[tokio::test]
async fn a_well_formed_resource_arn_earns_the_account_or_existence_answer() {
    let harness = Harness::new();

    for (case, arn, expected_status) in well_formed_resource_arns() {
        let expected_code = if expected_status == 403 {
            "AccessDeniedException"
        } else {
            "ResourceNotFoundException"
        };
        for (operation, (status, body)) in [
            ("ListTagsForResource", list_tags(&harness, &arn).await),
            (
                "ListResourcePolicies",
                list_resource_policies(&harness, &arn, "").await,
            ),
        ] {
            assert_eq!(
                status, expected_status,
                "case {case:?} ({arn:?}) / {operation}: expected {expected_status}, got \
                 {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} ({arn:?}) / {operation}: wrong error code in {body:?}"
            );
        }
    }
}

/// `ListResourcePolicies` reports the policies the resource carries, in attachment
/// order, in pages of at most `MaxResults`, carrying a `NextToken` exactly while more
/// remain — for every attached-policy count `Policies` allows and every page size
/// `MaxResults` allows.
#[tokio::test]
async fn resource_policies_are_paged_out_of_the_resource_record() {
    // `Policies` is `length(1..10)`: each published bound and counts between.
    for count in [1usize, 2, 3, 9, MAX_POLICIES] {
        let harness = Harness::new();
        let attachments = record_attachments(&harness, count);
        let arn = create_team(&harness, &format!("policy-team-{count:02}"), &attachments).await;
        let stored = stored_resource_policies(&harness, &arn);
        assert_eq!(
            stored.len(),
            count,
            "the store must resolve every attachment the create request listed: {stored:?}"
        );

        // Each published bound of `MaxResults` (1 and 20) and sizes between.
        for page_size in [MAX_RESULTS_MIN, 2, 3, 7, MAX_RESULTS_MAX] {
            let case = format!("{count} policies, MaxResults={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let query = match &token {
                    None => format!("MaxResults={page_size}"),
                    Some(t) => format!("MaxResults={page_size}&NextToken={t}"),
                };
                let (status, body) = list_resource_policies(&harness, &arn, &query).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: ListResourcePolicies returned {status} with {body:?}"
                );
                let page = body["ResourcePolicies"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() as i64 <= page_size,
                    "case {case:?}: a page of {} exceeds MaxResults: {body:?}",
                    page.len()
                );
                for listed in &page {
                    let position = seen.len();
                    assert_entry(&case, listed, &stored[position]);
                    seen.push(text(listed, "PolicyArn"));
                }
                pages += 1;
                assert!(
                    pages <= stored.len() + 1,
                    "case {case:?}: paging did not terminate"
                );
                match body.get("NextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            let expected: Vec<String> = stored
                .iter()
                .map(|policy| policy.policy_arn.clone())
                .collect();
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must yield every attached policy exactly once, in \
                 attachment order"
            );
            assert_eq!(
                seen.iter().collect::<BTreeSet<_>>().len(),
                seen.len(),
                "case {case:?}: paging reported a policy twice: {seen:?}"
            );
        }

        // `MaxResults` omitted takes the modelled default of 20, which covers every
        // count `Policies` allows.
        let (status, body) = list_resource_policies(&harness, &arn, "").await;
        assert_eq!(
            status, 200,
            "the default page returned {status} with {body:?}"
        );
        let page = body["ResourcePolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            page.len(),
            count,
            "the default MaxResults of {MAX_RESULTS_MAX} must hold {count} policies: {body:?}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "a complete page carries no NextToken: {body:?}"
        );

        // A read changes nothing a later caller sees.
        let (status, again) = list_resource_policies(&harness, &arn, "").await;
        assert_eq!(status, 200, "the second read returned {again:?}");
        assert_eq!(
            again, body,
            "ListResourcePolicies is a read: two calls must agree"
        );
        assert_eq!(
            stored_resource_policies(&harness, &arn),
            stored,
            "ListResourcePolicies must not change the policies it reports"
        );
    }
}

/// One listed entry, against the attachment the store resolved for that position.
fn assert_entry(case: &str, listed: &Value, stored: &state::ResourcePolicy) {
    assert_eq!(
        text(listed, "PolicyArn"),
        stored.policy_arn,
        "case {case:?}: expected PolicyArn {:?}, got {listed:?}",
        stored.policy_arn
    );
    assert_eq!(
        text(listed, "PolicyName"),
        stored.policy_name,
        "case {case:?}: expected PolicyName {:?}, got {listed:?}",
        stored.policy_name
    );
    let policy_type = text(listed, "PolicyType");
    assert_eq!(
        policy_type,
        stored.policy_type.as_wire(),
        "case {case:?}: expected PolicyType {:?}, got {listed:?}",
        stored.policy_type.as_wire()
    );
    assert!(
        model::POLICY_TYPE_VALUES.contains(&policy_type.as_str()),
        "case {case:?}: PolicyType {policy_type:?} is not a token the model declares \
         ({:?})",
        model::POLICY_TYPE_VALUES
    );
    // `PolicyArn` is the ARN "for policy": the policy's own unqualified ARN, not the
    // version-qualified reference the attachment pins, which is reported by
    // `GetResourcePolicy$PolicyVersionArn`.
    assert!(
        !text(listed, "PolicyArn").ends_with("/$DEFAULT"),
        "case {case:?}: PolicyArn carries a version qualifier: {listed:?}"
    );
}

/// Every member a listed entry carries is the one `GetResourcePolicy` answers for the
/// same attachment, so the two resource-policy reads cannot disagree.
#[tokio::test]
async fn a_listed_entry_agrees_with_the_single_policy_read() {
    let harness = Harness::new();
    let attachments = record_attachments(&harness, 3);
    let arn = create_team(&harness, "agreement-team", &attachments).await;

    let (status, body) = list_resource_policies(&harness, &arn, "").await;
    assert_eq!(
        status, 200,
        "ListResourcePolicies returned {status} with {body:?}"
    );
    let page = body["ResourcePolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        page.len(),
        attachments.len(),
        "every attachment must be reported: {body:?}"
    );

    for listed in &page {
        let name = text(listed, "PolicyName");
        let policy_type = text(listed, "PolicyType");
        let (status, single) = get_resource_policy(&harness, &arn, &name, &policy_type).await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetResourcePolicy returned {status} with {single:?}"
        );
        assert_eq!(
            text(&single, "PolicyName"),
            name,
            "case {name:?}: the two reads disagree about PolicyName: {single:?}"
        );
        assert_eq!(
            text(&single, "PolicyType"),
            policy_type,
            "case {name:?}: the two reads disagree about PolicyType: {single:?}"
        );
        // The single read reports the VERSION in effect; its ARN must be a version of
        // the policy the list reported.
        assert!(
            text(&single, "PolicyVersionArn").starts_with(&text(listed, "PolicyArn")),
            "case {name:?}: PolicyVersionArn {:?} is not a version of the listed policy \
             {:?}",
            text(&single, "PolicyVersionArn"),
            text(listed, "PolicyArn")
        );
    }
}

/// A `NextToken` `ListResourcePolicies` never issued is refused with
/// `ValidationException` (400), whatever its form; the token it DID issue is accepted.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let harness = Harness::new();
    let attachments = record_attachments(&harness, 3);
    let arn = create_team(&harness, "token-team", &attachments).await;

    // (case, the token). `%20` is a percent-encoded space.
    let rows: [(&str, String); 5] = [
        ("not a token at all", "not-a-token".to_string()),
        ("a space", "%20".to_string()),
        ("a negative number", "-1".to_string()),
        ("a decimal", "1.5".to_string()),
        ("a policy ARN", attachments[0].clone()),
    ];
    for (case, token) in rows {
        let (status, body) =
            list_resource_policies(&harness, &arn, &format!("NextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign NextToken must be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    let (status, body) = list_resource_policies(&harness, &arn, "MaxResults=1").await;
    assert_eq!(status, 200, "the first page returned {status} with {body:?}");
    let token = text(&body, "NextToken");
    assert!(
        !token.is_empty(),
        "a partial page must carry a NextToken: {body:?}"
    );
    let (status, body) =
        list_resource_policies(&harness, &arn, &format!("MaxResults=1&NextToken={token}")).await;
    assert_eq!(
        status, 200,
        "the service's own NextToken must be accepted, got {status} with {body:?}"
    );
}

/// `ListTagsForResource` reports the tags the resource's record holds, for a resource
/// created with tags and for one created without — the map comes from state, and an
/// untagged resource answers an empty map rather than an absent member.
#[tokio::test]
async fn tags_are_reported_out_of_the_resource_record() {
    let harness = Harness::new();
    let attachments = record_attachments(&harness, 1);

    // (case, the Tags the create request carries)
    let rows: Vec<(&str, BTreeMap<String, String>)> = vec![
        ("no tags at all", BTreeMap::new()),
        (
            "several tags, one with an empty value",
            BTreeMap::from([
                ("env".to_string(), "test".to_string()),
                ("owner".to_string(), "platform".to_string()),
                // `TagValue` is `length(min: 0, max: 256)`: the minimum is empty.
                ("blank".to_string(), String::new()),
            ]),
        ),
        (
            // `TagKey` is `length(1..128)`, `TagValue` `length(0..256)`: the maxima.
            "key and value at their length maxima",
            BTreeMap::from([("k".repeat(128), "v".repeat(256))]),
        ),
    ];

    for (index, (case, tags)) in rows.iter().enumerate() {
        let mut body = create_body(&format!("tag-read-{index:02}"), &attachments);
        if !tags.is_empty() {
            body["Tags"] = Value::Object(
                tags.iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            );
        }
        let (status, created) = harness.create(&body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApprovalTeam returned {status} with {created:?}"
        );
        let arn = text(&created, "Arn");

        let (status, listed) = list_tags(&harness, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource returned {status} with {listed:?}"
        );
        let reported: BTreeMap<String, String> = listed
            .get("Tags")
            .and_then(Value::as_object)
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListTagsForResource reported no Tags map: {listed:?}")
            })
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            &reported, tags,
            "case {case:?}: expected Tags {tags:?}, got {listed:?}"
        );
    }
}
