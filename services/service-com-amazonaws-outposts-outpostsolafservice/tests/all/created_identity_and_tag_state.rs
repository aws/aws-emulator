// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a SUCCESS response reports about the resource the call
//! just created or changed, driven over the wire through
//! `MultiServiceRouter::dispatch` exactly as a signed `aws outposts` call arrives
//! (reusing the harness of [`crate::site_and_outpost_lifecycle`]).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `CreateSite`'s 200 reports the new site on the `Site` its output declares, and
//!    nothing beside it — that projection is the only place a caller learns the id
//!    the service minted, and every later call addressed with it (`GetSite`,
//!    `CreateOutpost$SiteId`) reaches that site.
//! 2. A success response projects the resource AS STORED, so `UpdateSite`'s 200
//!    carries the tag map the site holds at that moment — including tags a
//!    `TagResource` applied after the create, and excluding ones an `UntagResource`
//!    removed — rather than a tagless projection, and a later read agrees with it.
//! 3. A tag mutation (`TagResource`, `UntagResource`) reports nothing back about the
//!    resource it changed — its output declares no member — so it answers
//!    [`TAG_MUTATION_SUCCESS_STATUS`] with no member in the body, while the change
//!    itself is visible to the next read.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::error_classification::{reported_tags, tag_map};
use crate::site_and_outpost_lifecycle::{
    address, is_minted_id, query_value, segment, text, Outposts, SITE_ARN_PREFIX,
    TAG_MUTATION_SUCCESS_STATUS,
};

/// The prefix the modelled `SiteId` (`^os-[a-f0-9]{17}$` in its bare form) carries.
const SITE_ID_PREFIX: &str = "os";

/// `POST /sites`, answering the WHOLE response body — the create's own members, not
/// just the `Site` the harness helper projects out of it.
async fn create_site(outposts: &Outposts, request: &Value) -> Value {
    outposts.ok("POST", "/sites", Some(request)).await
}

/// The members `CreateSiteOutput` declares, as
/// `smithy_query(query="operation", operation="CreateSite")` reports its output: the
/// created site alone.
const CREATE_SITE_OUTPUT_MEMBERS: &[&str] = &["Site"];

/// Rule 1, over everything `CreateSite` generalises over: the optional members the
/// request may carry (none, a description, notes, tags, each address, the rack
/// properties) and the number of tags (none, one, several).
///
/// Each row checks the response carries only the member the output declares, that the
/// id on the `Site` is of the modelled form, and that this id addresses the new site —
/// both to read it back (`GetSite`) and to place an Outpost on it (`CreateOutpost`). A
/// response that reported no site, or a member the model does not declare, fails here.
#[tokio::test]
async fn create_site_answers_the_minted_id_on_the_site_it_created() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Value)> = vec![
        ("the name alone", json!({ "Name": "identity: name only" })),
        (
            "a description and notes",
            json!({
                "Name": "identity: described",
                "Description": "the described site",
                "Notes": "badge required\nescort to the cage",
            }),
        ),
        (
            "one tag",
            json!({ "Name": "identity: one tag", "Tags": { "Env": "dev" } }),
        ),
        (
            "several tags",
            json!({
                "Name": "identity: several tags",
                "Tags": { "Env": "prod", "team": "platform", "cost.center": "4711" },
            }),
        ),
        (
            "both addresses",
            json!({
                "Name": "identity: addressed",
                "OperatingAddress": address("US", "WA", "Seattle"),
                "ShippingAddress": address("US", "WA", "Kirkland"),
            }),
        ),
        (
            "every optional member at once",
            json!({
                "Name": "identity: everything",
                "Description": "every member",
                "Notes": "escort required",
                "Tags": { "Env": "staging" },
                "OperatingAddress": address("DE", "BE", "Berlin"),
                "ShippingAddress": address("DE", "BE", "Berlin"),
                "RackPhysicalProperties": {
                    "PowerDrawKva": "POWER_10_KVA",
                    "PowerPhase": "THREE_PHASE",
                },
            }),
        ),
        (
            "a name at the modelled maximum length",
            json!({ "Name": "n".repeat(1000) }),
        ),
    ];

    for (case, request) in &cases {
        let created = create_site(&outposts, request).await;
        let reported: Vec<String> = created
            .as_object()
            .map(|members| members.keys().cloned().collect())
            .unwrap_or_default();
        assert_eq!(
            reported,
            CREATE_SITE_OUTPUT_MEMBERS
                .iter()
                .map(|member| (*member).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the create must answer exactly the members its output \
             declares, got {created:?}"
        );

        let answered = text(&created["Site"], "SiteId");
        assert!(
            is_minted_id(&answered, SITE_ID_PREFIX),
            "case {case:?}: the created site must carry the minted SiteId, got \
             {answered:?} in {created:?}"
        );
        let site_arn = text(&created["Site"], "SiteArn");
        assert_eq!(
            site_arn,
            format!("{SITE_ARN_PREFIX}{answered}"),
            "case {case:?}: the site's ARN must name the id the create answered, got \
             {site_arn:?}"
        );

        // The id read off the created site is what addresses the new site.
        let read = outposts.ok("GET", &format!("/sites/{answered}"), None).await;
        assert_eq!(
            read["Site"], created["Site"],
            "case {case:?}: GetSite on the answered id must answer the created site, got \
             {read:?}"
        );

        let placed = outposts
            .ok(
                "POST",
                "/outposts",
                Some(&json!({
                    "Name": format!("identity outpost: {case}"),
                    "SiteId": answered,
                })),
            )
            .await;
        assert_eq!(
            text(&placed["Outpost"], "SiteId"),
            answered,
            "case {case:?}: an Outpost placed with the answered id must stand on that \
             site, got {placed:?}"
        );
    }
}

/// One site's tag history and the update applied after it: a row of rule 2.
struct Row {
    case: &'static str,
    /// The `Tags` the create carried, or `null` for a create that carried none.
    at_create: Value,
    /// The tags a `TagResource` applies after the create, or `null` for none.
    applied: Value,
    /// Whether one applied tag's VALUE is the site's OWN ARN — a value no caller can
    /// know before the create, which is why it can only arrive through `TagResource`.
    own_arn: bool,
    /// The keys an `UntagResource` removes after that.
    removed: &'static [&'static str],
    /// The `UpdateSite` request body.
    update: Value,
    /// Whether the update addresses the site by its full ARN rather than its bare
    /// id: `SiteId` accepts both forms and both name the one stored site.
    by_arn: bool,
}

/// The tag key whose value is the site's own ARN. See [`Row::own_arn`].
const OWN_ARN_KEY: &str = "resourceArn";

/// Rule 2, over the ways a site's tag map can have come about and the members one
/// update can carry.
///
/// The rows differ along the tags the create carried (none, one, several), the tags a
/// later `TagResource` applied (none, one, several, a key that replaces a stored one,
/// and one whose value is the site's own ARN), whether an `UntagResource` then
/// removed some or all of them, which of `UpdateSite`'s three optional members the
/// request carries (each alone, all of them, and none at all), and which of the two
/// forms `SiteId` accepts addresses the update. Every row asserts the update's OWN
/// response, then re-reads the site, so a response built from a tagless projection —
/// or an update that dropped the stored tags — fails here.
#[tokio::test]
async fn update_site_answers_the_tags_the_site_carries_at_that_moment() {
    let outposts = Outposts::new();
    let rows: Vec<Row> = vec![
        Row {
            case: "no tags at create, one applied afterwards",
            at_create: json!(null),
            applied: json!({ "owner": "sre" }),
            own_arn: false,
            removed: &[],
            update: json!({ "Name": "tag state: renamed" }),
            by_arn: false,
        },
        Row {
            case: "one tag at create, another applied over it",
            at_create: json!({ "Env": "dev" }),
            applied: json!({ "team": "platform" }),
            own_arn: false,
            removed: &[],
            update: json!({ "Description": "re-described" }),
            by_arn: true,
        },
        Row {
            case: "a tag whose value is the site's own ARN",
            at_create: json!(null),
            applied: json!(null),
            own_arn: true,
            removed: &[],
            update: json!({ "Name": "tag state: self-referencing tag" }),
            by_arn: false,
        },
        Row {
            case: "several tags at create, several more applied",
            at_create: json!({ "Env": "prod", "team": "platform" }),
            applied: json!({ "tier": "gold", "window": "sat 02:00" }),
            own_arn: false,
            removed: &[],
            update: json!({ "Notes": "two people required" }),
            by_arn: true,
        },
        Row {
            case: "an applied key replacing the value stored at create",
            at_create: json!({ "owner": "sre" }),
            applied: json!({ "owner": "platform" }),
            own_arn: false,
            removed: &[],
            update: json!({
                "Name": "tag state: every member",
                "Description": "every member",
                "Notes": "every member",
            }),
            by_arn: false,
        },
        Row {
            case: "one of the applied keys removed again",
            at_create: json!({ "Env": "dev" }),
            applied: json!({ "team": "platform", "tier": "gold" }),
            own_arn: false,
            removed: &["team"],
            update: json!({ "Name": "tag state: partly untagged" }),
            by_arn: false,
        },
        Row {
            case: "every key removed again",
            at_create: json!({ "Env": "dev" }),
            applied: json!({ "team": "platform" }),
            own_arn: false,
            removed: &["Env", "team"],
            update: json!({ "Name": "tag state: fully untagged" }),
            by_arn: true,
        },
        Row {
            case: "a site that was never tagged, updated with no member at all",
            at_create: json!(null),
            applied: json!(null),
            own_arn: false,
            removed: &[],
            update: json!({}),
            by_arn: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut request = json!({ "Name": format!("tag state: {case}") });
        if row.at_create.is_object() {
            request["Tags"] = row.at_create.clone();
        }
        let created = create_site(&outposts, &request).await;
        let site_id = text(&created["Site"], "SiteId");
        let site_arn = text(&created["Site"], "SiteArn");
        let mut expected: BTreeMap<String, String> = tag_map(&row.at_create);

        // The tags that arrive through the tagging API, including the one whose value
        // is the ARN the create just minted.
        let mut applied = tag_map(&row.applied);
        if row.own_arn {
            applied.insert(OWN_ARN_KEY.to_string(), site_arn.clone());
        }
        if !applied.is_empty() {
            let tags: serde_json::Map<String, Value> = applied
                .iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect();
            outposts
                .no_content(
                    "POST",
                    &format!("/tags/{}", segment(&site_arn)),
                    Some(&json!({ "Tags": Value::Object(tags) })),
                )
                .await;
            expected.extend(applied);
        }

        if !row.removed.is_empty() {
            let keys: Vec<String> = row
                .removed
                .iter()
                .map(|key| format!("tagKeys={}", query_value(key)))
                .collect();
            outposts
                .no_content(
                    "DELETE",
                    &format!("/tags/{}?{}", segment(&site_arn), keys.join("&")),
                    None,
                )
                .await;
            for key in row.removed {
                expected.remove(*key);
            }
        }

        // `SiteId` takes the bare id or the full site ARN, so the row says which
        // form addresses the update; either way it is the one stored site.
        let addressed = if row.by_arn {
            segment(&site_arn)
        } else {
            site_id.clone()
        };
        let updated = outposts
            .ok("PATCH", &format!("/sites/{addressed}"), Some(&row.update))
            .await;
        assert_eq!(
            reported_tags(&updated["Site"]),
            expected,
            "case {case:?}: the update must answer the tags the site carries, got {:?}",
            updated["Site"]
        );

        // The update is an update: the members it carried landed, and the response
        // reports them beside those tags.
        for (member, value) in row.update.as_object().into_iter().flatten() {
            let reported = text(&updated["Site"], member);
            assert_eq!(
                reported,
                value.as_str().unwrap_or_default(),
                "case {case:?}: the update must report the {member} it was sent, got \
                 {reported:?}"
            );
        }

        // A later read agrees with the answer the update gave.
        let read = outposts.ok("GET", &format!("/sites/{site_id}"), None).await;
        assert_eq!(
            reported_tags(&read["Site"]),
            expected,
            "case {case:?}: the read-back must report the same tags the update answered, \
             got {:?}",
            read["Site"]
        );
    }
}

/// One tag mutation to call, and the tags the resource must carry afterwards: a row
/// of rule 3.
struct Mutation {
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    expected: BTreeMap<String, String>,
}

/// Rule 3, over both tag mutations, both taggable resource kinds and the shapes one
/// request can take: a map of one entry and of several, a request re-sending a pair
/// the resource already carries, a removal of a carried key, of a key the resource
/// never had, and of every key it has left.
///
/// Each row asserts the STATUS and that the answer carries no member — a body with
/// one would be a value the operation's output does not declare — and then reads the
/// resource back, so a no-content answer that wrote nothing fails here.
#[tokio::test]
async fn every_tag_mutation_answers_the_no_content_status() {
    let outposts = Outposts::new();
    let created = create_site(
        &outposts,
        &json!({ "Name": "no content site", "Tags": { "Env": "dev" } }),
    )
    .await;
    let site_arn = text(&created["Site"], "SiteArn");
    let outpost = outposts
        .create_outpost(&json!({
            "Name": "no content outpost",
            "SiteId": text(&created["Site"], "SiteId"),
            "Tags": { "Env": "dev" },
        }))
        .await;
    let outpost_arn = text(&outpost, "OutpostArn");

    let tag_path = |arn: &str| format!("/tags/{}", segment(arn));
    let untag_path = |arn: &str, keys: &[&str]| {
        let query: Vec<String> = keys
            .iter()
            .map(|key| format!("tagKeys={}", query_value(key)))
            .collect();
        format!("/tags/{}?{}", segment(arn), query.join("&"))
    };
    let expect = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    };

    let mutations: Vec<Mutation> = vec![
        Mutation {
            case: "TagResource on a site, one new key".to_string(),
            method: "POST",
            path: tag_path(&site_arn),
            body: Some(json!({ "Tags": { "owner": "sre" } })),
            expected: expect(&[("Env", "dev"), ("owner", "sre")]),
        },
        Mutation {
            case: "TagResource on a site, several keys at once".to_string(),
            method: "POST",
            path: tag_path(&site_arn),
            body: Some(json!({ "Tags": { "tier": "gold", "window": "sat 02:00" } })),
            expected: expect(&[
                ("Env", "dev"),
                ("owner", "sre"),
                ("tier", "gold"),
                ("window", "sat 02:00"),
            ]),
        },
        Mutation {
            case: "TagResource on a site, the same pair again".to_string(),
            method: "POST",
            path: tag_path(&site_arn),
            body: Some(json!({ "Tags": { "owner": "sre" } })),
            expected: expect(&[
                ("Env", "dev"),
                ("owner", "sre"),
                ("tier", "gold"),
                ("window", "sat 02:00"),
            ]),
        },
        Mutation {
            case: "UntagResource on a site, a key it carries".to_string(),
            method: "DELETE",
            path: untag_path(&site_arn, &["owner"]),
            body: None,
            expected: expect(&[("Env", "dev"), ("tier", "gold"), ("window", "sat 02:00")]),
        },
        Mutation {
            case: "UntagResource on a site, a key it does not carry".to_string(),
            method: "DELETE",
            path: untag_path(&site_arn, &["never-applied"]),
            body: None,
            expected: expect(&[("Env", "dev"), ("tier", "gold"), ("window", "sat 02:00")]),
        },
        Mutation {
            case: "UntagResource on a site, every key it has left".to_string(),
            method: "DELETE",
            path: untag_path(&site_arn, &["Env", "tier", "window"]),
            body: None,
            expected: BTreeMap::new(),
        },
        Mutation {
            case: "TagResource on an Outpost, one new key".to_string(),
            method: "POST",
            path: tag_path(&outpost_arn),
            body: Some(json!({ "Tags": { "owner": "network" } })),
            expected: expect(&[("Env", "dev"), ("owner", "network")]),
        },
        Mutation {
            case: "UntagResource on an Outpost, several keys at once".to_string(),
            method: "DELETE",
            path: untag_path(&outpost_arn, &["Env", "owner"]),
            body: None,
            expected: BTreeMap::new(),
        },
    ];

    for mutation in &mutations {
        let case = &mutation.case;
        let reply = outposts
            .call(mutation.method, &mutation.path, mutation.body.as_ref())
            .await;
        assert_eq!(
            reply.status, TAG_MUTATION_SUCCESS_STATUS,
            "case {case:?}: expected HTTP {TAG_MUTATION_SUCCESS_STATUS}, got {} {}",
            reply.status, reply.body
        );
        let members = match &reply.body {
            // No bytes at all, and the empty object the framework writes for an output
            // structure with no members: neither carries a member.
            Value::Null => 0,
            Value::Object(members) => members.len(),
            _ => usize::MAX,
        };
        assert_eq!(
            members, 0,
            "case {case:?}: the answer must carry no member, got {}",
            reply.body
        );

        // The change the mutation reported nothing about is what the next read answers.
        let arn = mutation
            .path
            .trim_start_matches("/tags/")
            .split('?')
            .next()
            .unwrap_or_default();
        let listed = outposts.ok("GET", &format!("/tags/{arn}"), None).await;
        assert_eq!(
            reported_tags(&listed),
            mutation.expected,
            "case {case:?}: the tags after the mutation must be what it wrote, got {listed:?}"
        );
    }
}
