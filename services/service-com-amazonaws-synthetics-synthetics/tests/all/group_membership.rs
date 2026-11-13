// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the CloudWatch Synthetics group-MEMBERSHIP slice:
//! `AssociateResource` and the read that proves what it wrote, `ListGroupResources`.
//! (`CreateGroup` and `GetGroup`'s own rules are pinned by `group_lifecycle.rs`; they are
//! used here only to have a group to associate with and to read its identity back.)
//!
//! The rules asserted here, each stated once and then exercised over a SET of inputs
//! rather than over one recorded request:
//!
//! * An association is addressed by any of the three forms the model documents for every
//!   group-addressing member — "You can specify the group name, the ARN, or the group ID as
//!   the `GroupIdentifier`" — and all three reach the ONE stored group.
//! * `AssociateResource` makes one canary a member of one group: both sides must already
//!   exist (404 `ResourceNotFoundException` otherwise), membership is a set that is written
//!   ONCE (so a canary already in the group is refused with 409 `ConflictException`), and
//!   the documented group quotas — "Each group can contain as
//!   many as 10 canaries … Any single canary can be a member of up to 10 groups"
//!   (`smithy_query(query="doc",shape="CreateGroup")`) — are refused with
//!   `ServiceQuotaExceededException` (HTTP 402).
//! * `ListGroupResources` reports the stored membership set, in one total order, paged.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, through the harness the lifecycle suite defines.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::canary_lifecycle::{create_body, poll_until, Answer, Synthetics};

/// The sentence a group lookup whose wording is RECORDED answers with for a
/// `GroupIdentifier` it cannot resolve, naming the identifier the request supplied:
/// `Group does not exist: <the identifier as supplied>`. That is what the recorded service
/// answers for `AssociateResource`, and — per the recorded unit
/// `synthetics.delete-group-nonexistent-resource-not-found`, whose later step reads a
/// just-deleted group back — for `DeleteGroup` and `GetGroup` too. The group operations
/// with no recorded wording (`ListGroupResources`) word the same miss with the service's
/// fixed, identifier-free sentence instead, so what is asserted per operation is what that
/// operation's wording is known to be.
///
/// One construction for the whole target, defined beside the group lifecycle suite.
use crate::group_lifecycle::group_not_found_message as recorded_group_not_found;

/// How `AssociateResource` words a canary lookup that misses
/// (`state::canary_not_found`): the recorded service builds the message FROM THE REQUEST,
/// naming the `ResourceArn` it could not resolve, the same construction it uses for an
/// unresolvable `GroupIdentifier` on that same call.
///
/// `pub(crate)` because the suite that asserts this wording as a rule
/// (`client_error_message_wording`) asserts this one construction.
pub(crate) fn canary_not_found_message(resource_arn: &str) -> String {
    format!("Canary does not exist: {resource_arn}")
}

/// The fixed sentence a canary lookup whose wording is NOT recorded answers with: the
/// operations addressed by canary `Name` (`tests/all/missing_resource_errors.rs`) and
/// `ListAssociatedGroups` (`tests/all/associated_groups.rs`).
pub(crate) const CANARY_NOT_FOUND: &str = "Canary not found.";

/// "Each group can contain as many as 10 canaries." — `CreateGroup`'s documentation.
pub(crate) const MAX_CANARIES_PER_GROUP: usize = 10;

/// "Any single canary can be a member of up to 10 groups." — same source.
pub(crate) const MAX_GROUPS_PER_CANARY: usize = 10;

/// A canary's ARN, written out from `CanaryArn`'s modelled pattern
/// (`^arn:(aws[a-zA-Z-]*)?:synthetics:…:\d{12}:canary:[0-9a-z_\-]{1,255}$`) rather than
/// read back from a response: the `Canary` shape carries no ARN member, so the ARN a
/// caller passes to `AssociateResource` is one it composes itself.
pub(crate) fn canary_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:synthetics:{region}:{}:canary:{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

pub(crate) async fn create_group(aws: &Synthetics, body: Value) -> Answer {
    aws.call("POST", "/group", Some(body)).await
}

async fn get_group(aws: &Synthetics, identifier: &str) -> Answer {
    aws.call("GET", &format!("/group/{identifier}"), None).await
}

pub(crate) async fn associate(aws: &Synthetics, identifier: &str, resource_arn: &str) -> Answer {
    aws.call(
        "PATCH",
        &format!("/group/{identifier}/associate"),
        Some(json!({ "ResourceArn": resource_arn })),
    )
    .await
}

pub(crate) async fn list_resources(aws: &Synthetics, identifier: &str, body: Value) -> Answer {
    aws.call(
        "POST",
        &format!("/group/{identifier}/resources"),
        Some(body),
    )
    .await
}

/// Assert a 200 and hand the answer back.
pub(crate) fn ok(case: &str, answer: Answer) -> Answer {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} ({:?})",
        answer.status, answer.body
    );
    answer
}

/// Assert the modelled error envelope — status and `x-amzn-errortype` — and return the
/// `Message`, which is `None` when the body carries none.
pub(crate) fn error_of(case: &str, answer: &Answer, status: u16, code: &str) -> Option<String> {
    assert_eq!(
        answer.status, status,
        "case {case}: expected {status}, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some(code),
        "case {case}: expected {code}, got {:?} ({:?})",
        answer.error_code,
        answer.body
    );
    answer.at("Message").as_str().map(str::to_string)
}

/// The `Resources` member of a `ListGroupResources` answer.
pub(crate) fn resources_of(case: &str, answer: &Answer) -> Vec<String> {
    answer
        .at("Resources")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a Resources array, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected ARN strings, got {entry:?}"))
                .to_string()
        })
        .collect()
}

/// Create `name` as a canary in `aws`' Region and answer its ARN.
pub(crate) async fn canary(aws: &Synthetics, region: &str, name: &str) -> String {
    ok(
        &format!("create-canary/{name}"),
        aws.create(create_body(name)).await,
    );
    canary_arn(region, name)
}

/// A group round-trips through `CreateGroup` → `GetGroup`, and all three documented forms
/// of `GroupIdentifier` — name, id and ARN — address the ONE stored record, for reads and
/// for associations alike.
#[tokio::test]
async fn every_group_identifier_form_addresses_the_same_stored_group() {
    let region = "eu-west-1";
    let aws = Synthetics::new(region);

    let created = ok(
        "create-group",
        create_group(
            &aws,
            json!({ "Name": "web-probes", "Tags": { "team": "sre" } }),
        )
        .await,
    );
    let name = created.string_at("Group.Name");
    let id = created.string_at("Group.Id");
    let arn = created.string_at("Group.Arn");

    // `GroupArn`'s pattern ends `:group:[0-9a-z]+$`, so the id is lowercase alphanumeric
    // and is the ARN's resource part — the ARN is not built from the group's name, which
    // "can include any Unicode characters".
    assert!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case create-group: expected a lowercase alphanumeric Group.Id, got {id:?}"
    );
    assert_eq!(
        arn,
        format!(
            "arn:aws:synthetics:{region}:{}:group:{id}",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "case create-group: expected an ARN naming the signed Region and the group id, got {arn:?}"
    );

    // Three canaries, so each identifier form below associates a DIFFERENT resource and
    // the membership set proves all three calls reached the same group.
    let alpha = canary(&aws, region, "alpha-canary").await;
    let beta = canary(&aws, region, "beta-canary").await;
    let gamma = canary(&aws, region, "gamma_canary").await;

    // The rows differ along the form of the identifier, which is exactly what the rule
    // generalises over.
    let forms = [
        ("by-name", name.clone(), alpha.clone()),
        ("by-id", id.clone(), beta.clone()),
        ("by-arn", arn.clone(), gamma.clone()),
    ];

    for (case, identifier, _) in &forms {
        let read = ok(&format!("get-group/{case}"), get_group(&aws, identifier).await);
        for field in ["Group.Id", "Group.Name", "Group.Arn", "Group.Tags"] {
            assert_eq!(
                read.at(field),
                created.at(field),
                "case get-group/{case}: expected {field} to match what CreateGroup stored, \
                 got {:?} vs {:?}",
                read.at(field),
                created.at(field)
            );
        }
        assert_eq!(
            read.at("Group.CreatedTime"),
            created.at("Group.CreatedTime"),
            "case get-group/{case}: expected a stable CreatedTime, got {:?}",
            read.at("Group.CreatedTime")
        );
    }

    for (case, identifier, resource) in &forms {
        ok(
            &format!("associate/{case}"),
            associate(&aws, identifier, resource).await,
        );
    }

    let expected: BTreeSet<String> = forms
        .iter()
        .map(|(_, _, resource)| resource.clone())
        .collect();
    for (case, identifier, _) in &forms {
        let listed = ok(
            &format!("list/{case}"),
            list_resources(&aws, identifier, json!({})).await,
        );
        let reported: BTreeSet<String> = resources_of(&format!("list/{case}"), &listed)
            .into_iter()
            .collect();
        assert_eq!(
            reported, expected,
            "case list/{case}: expected the one membership set every identifier form \
             addresses, got {reported:?}"
        );
        assert!(
            listed.at("NextToken").is_null(),
            "case list/{case}: expected no NextToken for a complete page, got {:?}",
            listed.at("NextToken")
        );
    }

    // The group's own identity did not move when its membership did.
    let after = ok("get-group/after-associate", get_group(&aws, &id).await);
    for field in ["Group.Id", "Group.Name", "Group.Arn", "Group.CreatedTime"] {
        assert_eq!(
            after.at(field),
            created.at(field),
            "case get-group/after-associate: expected {field} unchanged, got {:?} vs {:?}",
            after.at(field),
            created.at(field)
        );
    }
    let (created_time, last_modified) = (
        after.at("Group.CreatedTime").as_f64(),
        after.at("Group.LastModifiedTime").as_f64(),
    );
    assert!(
        created_time.is_some()
            && last_modified.is_some()
            && last_modified >= created_time,
        "case get-group/after-associate: expected LastModifiedTime at or after \
         CreatedTime, got {last_modified:?} vs {created_time:?}"
    );
}

/// Both sides of an association have to exist: an identifier no group answers to, and a
/// canary ARN the store does not hold, are each answered with the modelled 404.
///
/// (`CreateGroup`'s own naming, tagging and quota rules are pinned by `group_lifecycle.rs`;
/// this suite only needs a group to associate canaries with.)
#[tokio::test]
async fn associate_resource_reports_a_missing_group_or_canary_as_not_found() {
    let region = "us-east-1";
    let aws = Synthetics::new(region);

    let live_arn = canary(&aws, region, "live-canary").await;
    let live_group = ok(
        "create-group",
        create_group(&aws, json!({ "Name": "live-group" })).await,
    );
    let live_id = live_group.string_at("Group.Id");

    // Group identifiers no group answers to, spanning the three forms the member accepts,
    // the modelled length bounds (`GroupIdentifier` is `len 1..128`) and resemblance to
    // the group that DOES exist.
    let missing_groups = [
        ("single-character", "x".to_string()),
        ("name-like", "no-such-group".to_string()),
        ("id-like", "0123456789ab".to_string()),
        (
            "arn-like",
            format!(
                "arn:aws:synthetics:{region}:{}:group:deadbeefcafe",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        ("max-length", "g".repeat(128)),
        ("prefix-of-existing", "live".to_string()),
        ("suffix-of-existing", "live-group-2".to_string()),
    ];

    let mut associate_sentences: BTreeSet<String> = BTreeSet::new();
    let mut reader_messages: BTreeSet<String> = BTreeSet::new();
    for (row, identifier) in &missing_groups {
        // `AssociateResource`'s wording is the recorded one, which names the identifier as
        // supplied.
        let case = format!("AssociateResource/{row}");
        let answer = associate(&aws, identifier, &live_arn).await;
        let message = error_of(&case, &answer, 404, "ResourceNotFoundException")
            .unwrap_or_else(|| panic!("case {case}: expected a Message, got {:?}", answer.body));
        assert_eq!(
            message,
            recorded_group_not_found(identifier),
            "case {case}: expected the missing-group sentence naming the identifier as \
             supplied, got {message:?}"
        );
        // The identifier is the ONLY thing that message varies by: it is the message's
        // suffix, and what precedes it is the same sentence for every row.
        associate_sentences.insert(
            message
                .strip_suffix(identifier.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: expected the message to end with the supplied \
                         identifier {identifier:?}, got {message:?}"
                    )
                })
                .to_string(),
        );

        // `GetGroup` words the same miss the recorded way too: the recorded unit
        // `synthetics.delete-group-nonexistent-resource-not-found` reads a group back
        // through `GetGroup` right after deleting it and is answered
        // `Group does not exist: <identifier>`, so that message echoes the identifier.
        let case = format!("GetGroup/{row}");
        let answer = get_group(&aws, identifier).await;
        let message = error_of(&case, &answer, 404, "ResourceNotFoundException")
            .unwrap_or_else(|| panic!("case {case}: expected a Message, got {:?}", answer.body));
        assert_eq!(
            message,
            recorded_group_not_found(identifier),
            "case {case}: expected the recorded missing-group sentence naming the \
             identifier as supplied, got {message:?}"
        );

        // `ListGroupResources` has no recorded wording, so it keeps this service's fixed,
        // identifier-free sentence — pinned as an invariant rather than as a literal,
        // because nothing documents its text.
        let case = format!("ListGroupResources/{row}");
        let answer = list_resources(&aws, identifier, json!({})).await;
        let message = error_of(&case, &answer, 404, "ResourceNotFoundException")
            .unwrap_or_else(|| panic!("case {case}: expected a Message, got {:?}", answer.body));
        assert!(
            !message.to_lowercase().contains(&identifier.to_lowercase()),
            "case {case}: expected a Message that does not name the requested group \
             {identifier:?}, got {message:?}"
        );
        reader_messages.insert(message);
    }
    assert_eq!(
        associate_sentences.len(),
        1,
        "case request-independence: expected one sentence around the echoed identifier \
         across every identifier, got {associate_sentences:?}"
    );
    assert_eq!(
        reader_messages.len(),
        1,
        "case reader-request-independence: expected one fixed message across every \
         identifier for the reader whose wording is not recorded, got {reader_messages:?}"
    );

    // Canary ARNs this store does not hold: another name, another Region (a canary is
    // associated "in the Region where the canary exists"), another account. Every row is
    // a legal `CanaryArn`, so each reaches the handler's lookup.
    let missing_canaries = [
        ("other-name", canary_arn(region, "no-such-canary")),
        ("other-region", canary_arn("eu-west-3", "live-canary")),
        (
            "other-account",
            "arn:aws:synthetics:us-east-1:999999999999:canary:live-canary".to_string(),
        ),
    ];
    for (row, resource_arn) in &missing_canaries {
        let case = format!("AssociateResource/{row}");
        let answer = associate(&aws, &live_id, resource_arn).await;
        let message = error_of(&case, &answer, 404, "ResourceNotFoundException")
            .unwrap_or_else(|| panic!("case {case}: expected a Message, got {:?}", answer.body));
        assert_eq!(
            message,
            canary_not_found_message(resource_arn),
            "case {case}: expected the missing-canary sentence naming the ARN as supplied, \
             got {message:?}"
        );
    }

    // Nothing above was persisted: the group is still empty, and the pair that DOES exist
    // still associates.
    let empty = ok("list/before", list_resources(&aws, &live_id, json!({})).await);
    assert!(
        resources_of("list/before", &empty).is_empty(),
        "case list/before: expected no members after refused associations, got {:?}",
        empty.at("Resources")
    );
    ok("associate/live", associate(&aws, &live_id, &live_arn).await);
    assert_eq!(
        resources_of(
            "list/after",
            &ok("list/after", list_resources(&aws, &live_id, json!({})).await)
        ),
        vec![live_arn.clone()],
        "case list/after: expected the one association that was accepted"
    );
}

/// Membership is a SET and it names a resource that exists: a canary that is already a
/// member is REFUSED rather than associated twice, whichever identifier form the repeat
/// uses, and deleting the canary ends the association.
#[tokio::test]
async fn a_duplicate_membership_is_refused_and_does_not_outlive_the_canary() {
    let region = "ap-south-1";
    let aws = Synthetics::new(region);

    let arn = canary(&aws, region, "repeat-canary").await;
    let group = ok(
        "create-group",
        create_group(&aws, json!({ "Name": "repeats" })).await,
    );
    let id = group.string_at("Group.Id");
    let group_arn = group.string_at("Group.Arn");

    // The first association is made.
    ok("associate/first", associate(&aws, &id, &arn).await);

    // Every repeat is refused with the modelled conflict, whichever of the three
    // identifier forms addresses the group — the membership is one relation, so the form
    // the repeat uses cannot make it a second one.
    for (case, identifier) in [
        ("by-id", id.clone()),
        ("by-name", "repeats".to_string()),
        ("by-arn", group_arn.clone()),
    ] {
        let answer = associate(&aws, &identifier, &arn).await;
        let message = error_of(&format!("associate/{case}"), &answer, 409, "ConflictException")
            .unwrap_or_else(|| panic!("case associate/{case}: expected a Message, got {:?}", answer.body));
        assert_eq!(
            message,
            format!("Canary {arn} already exists in group repeats"),
            "case associate/{case}: expected the duplicate-membership sentence naming the \
             canary ARN and the group's name, got {message:?}"
        );
        let listed = ok(
            &format!("list/{case}"),
            list_resources(&aws, &id, json!({})).await,
        );
        assert_eq!(
            resources_of(&format!("list/{case}"), &listed),
            vec![arn.clone()],
            "case list/{case}: expected the one membership the refused repeats left alone"
        );
    }

    // The membership can be made again once it has been removed: what the refusal reports
    // is the membership that exists, not the canary.
    ok(
        "disassociate",
        aws.call(
            "PATCH",
            &format!("/group/{id}/disassociate"),
            Some(json!({ "ResourceArn": arn })),
        )
        .await,
    );
    ok("associate/again", associate(&aws, &id, &arn).await);
    assert_eq!(
        resources_of(
            "list/after-re-associate",
            &ok(
                "list/after-re-associate",
                list_resources(&aws, &id, json!({})).await
            )
        ),
        vec![arn.clone()],
        "case list/after-re-associate: expected the membership to be back"
    );

    // The canary goes away, and with it the association: a membership can only name a
    // canary that exists. A canary still being provisioned cannot be deleted, so the delete
    // waits for the CONDITION the clock owes — the canary out of `CREATING` — rather than
    // for a span of its own.
    poll_until("canary/provisioned", || aws.get("repeat-canary"), |answer| {
        answer.string_at("Canary.Status.State") != "CREATING"
    })
    .await;
    ok("delete-canary", aws.delete("repeat-canary").await);
    let listed = ok(
        "list/after-delete",
        list_resources(&aws, &id, json!({})).await,
    );
    assert!(
        resources_of("list/after-delete", &listed).is_empty(),
        "case list/after-delete: expected a deleted canary to leave no membership behind, \
         got {:?}",
        listed.at("Resources")
    );
    // The group itself survives its last member.
    ok("get-group/after-delete", get_group(&aws, &id).await);
}

/// `ListGroupResources` pages over the membership set: every page size covers the whole
/// set exactly once, in one total order, and a token is issued only while entries remain.
#[tokio::test]
async fn list_group_resources_pages_over_the_whole_membership_set() {
    let region = "eu-central-1";
    let aws = Synthetics::new(region);

    let group = ok(
        "create-group",
        create_group(&aws, json!({ "Name": "paged" })).await,
    );
    let id = group.string_at("Group.Id");

    // A full group: the documented maximum of 10 canaries, so the page sizes below
    // genuinely split it.
    let mut all: Vec<String> = Vec::new();
    for index in 0..MAX_CANARIES_PER_GROUP {
        let arn = canary(&aws, region, &format!("paged-canary-{index:02}")).await;
        ok(
            &format!("associate/{index}"),
            associate(&aws, &id, &arn).await,
        );
        all.push(arn);
    }
    all.sort();

    // `MaxGroupResults` is `@range 1..20`: both published bounds, plus sizes that divide
    // the set evenly and unevenly.
    for size in [1usize, 3, 4, 10, 20] {
        let case = format!("page-size/{size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({ "MaxResults": size });
            if let Some(next) = &token {
                body["NextToken"] = json!(next);
            }
            let answer = ok(&case, list_resources(&aws, &id, body).await);
            let page = resources_of(&case, &answer);
            assert!(
                page.len() <= size,
                "case {case}: expected at most {size} entries per page, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            token = answer.at("NextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= all.len() + 1,
                "case {case}: expected paging to terminate, got {pages} pages"
            );
        }
        assert_eq!(
            seen, all,
            "case {case}: expected every member exactly once in ascending ARN order, got \
             {seen:?}"
        );
        let expected_pages = all.len().div_ceil(size);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} page(s), got {pages}"
        );
    }

    // A token this operation never issued is a bad request, not an empty page. A KEY token
    // naming an ARN the group no longer holds is NOT forged: a key boundary stays exact
    // across associates and disassociates, which is the point of the key form, so it
    // resumes at the next member in order.
    for (row, forged) in [
        ("opaque", "not-a-token".to_string()),
        ("wrong-prefix", format!("zz#{}", all[0])),
        ("unparsable-offset", "ix#three".to_string()),
        // An offset past the end of the set: a stale or forged token resumes nowhere.
        ("offset-past-the-end", format!("ix#{}", all.len() + 1)),
    ] {
        let case = format!("forged-token/{row}");
        let answer = list_resources(&aws, &id, json!({ "NextToken": forged })).await;
        error_of(&case, &answer, 400, "ValidationException");
    }
}

/// The two membership quotas `CreateGroup` documents are enforced, and a refused
/// association leaves the store exactly as it was.
#[tokio::test]
async fn membership_quotas_follow_the_documented_bounds() {
    let region = "us-east-2";
    let aws = Synthetics::new(region);

    // "Each group can contain as many as 10 canaries."
    let full = ok(
        "create-group/full",
        create_group(&aws, json!({ "Name": "full-group" })).await,
    );
    let full_id = full.string_at("Group.Id");
    for index in 0..MAX_CANARIES_PER_GROUP {
        let arn = canary(&aws, region, &format!("member-canary-{index:02}")).await;
        ok(
            &format!("associate/member-{index}"),
            associate(&aws, &full_id, &arn).await,
        );
    }
    let one_too_many = canary(&aws, region, "overflow-canary").await;
    let refused = associate(&aws, &full_id, &one_too_many).await;
    error_of(
        "associate/past-group-bound",
        &refused,
        402,
        "ServiceQuotaExceededException",
    );
    let listed = ok(
        "list/full-group",
        list_resources(&aws, &full_id, json!({ "MaxResults": 20 })).await,
    );
    let members = resources_of("list/full-group", &listed);
    assert_eq!(
        members.len(),
        MAX_CANARIES_PER_GROUP,
        "case list/full-group: expected the refused association to change nothing, got \
         {members:?}"
    );
    assert!(
        !members.contains(&one_too_many),
        "case list/full-group: expected the refused canary to be no member, got {members:?}"
    );

    // "Any single canary can be a member of up to 10 groups." The canary already refused
    // above is a member of nothing, so it is the one used here.
    let mut group_ids: Vec<String> = Vec::new();
    for index in 0..=MAX_GROUPS_PER_CANARY {
        let created = ok(
            &format!("create-group/{index}"),
            create_group(&aws, json!({ "Name": format!("joined-group-{index:02}") })).await,
        );
        group_ids.push(created.string_at("Group.Id"));
    }
    for (index, group_id) in group_ids.iter().take(MAX_GROUPS_PER_CANARY).enumerate() {
        ok(
            &format!("associate/joined-{index}"),
            associate(&aws, group_id, &one_too_many).await,
        );
    }
    let past_bound = group_ids
        .last()
        .expect("the loop above created one group past the bound");
    let refused = associate(&aws, past_bound, &one_too_many).await;
    error_of(
        "associate/past-canary-bound",
        &refused,
        402,
        "ServiceQuotaExceededException",
    );
    let listed = ok(
        "list/past-bound-group",
        list_resources(&aws, past_bound, json!({})).await,
    );
    assert!(
        resources_of("list/past-bound-group", &listed).is_empty(),
        "case list/past-bound-group: expected the refused association to leave the group \
         empty, got {:?}",
        listed.at("Resources")
    );
    // And the memberships it already had are untouched.
    for (index, group_id) in group_ids.iter().take(MAX_GROUPS_PER_CANARY).enumerate() {
        let case = format!("list/joined-{index}");
        let answer = ok(&case, list_resources(&aws, group_id, json!({})).await);
        assert_eq!(
            resources_of(&case, &answer),
            vec![one_too_many.clone()],
            "case {case}: expected the membership the earlier association wrote"
        );
    }
}
