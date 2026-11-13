// SPDX-License-Identifier: CC0-1.0

//! `ListPrivacyBudgetTemplates`: the privacy budget templates a membership's
//! collaboration holds.
//!
//! The rules under test:
//!
//! * the listing reports every privacy budget template of the collaboration the
//!   membership in the path belongs to — one summary per stored template, in creation
//!   order, projected from the SAME record `GetPrivacyBudgetTemplate` returns, and
//!   nothing of another collaboration;
//! * it pages on `maxResults`/`nextToken`, so a page never carries more rows than asked
//!   and resuming on the returned token yields every template exactly once;
//! * it follows state: a collaboration with no template reports an empty list, a create
//!   adds exactly its own row, and a delete removes it;
//! * a membership the store does not hold, a `maxResults` outside the declared range and
//!   a `nextToken` the service never handed out are refused with the modelled error for
//!   that failure, and a read changes nothing a later caller sees.
//!
//! The tables vary what those rules generalise over: both `PrivacyBudgetType` values and
//! both orders of creating them, the page sizes that divide the set evenly and unevenly
//! plus each published bound of `maxResults`, and collaborations holding zero, one and
//! two templates. Every request goes over the wire through the same `MultiServiceRouter`
//! the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, error_message, is_model_uuid, minimal_collaboration, Emulator,
    ALL_LOG_STATUSES,
};
// The create/read calls, the budget-type catalog and the never-minted membership id are
// the ones `cleanrooms_privacy_budget_template` already defines; this suite imports them
// rather than keeping a second copy.
use super::cleanrooms_privacy_budget_template::{
    create_template, get_template, ABSENT_UUID, ALL_PRIVACY_BUDGET_TYPES,
};

/// The published bounds of `ListPrivacyBudgetTemplatesInput$maxResults` (model shape
/// `MaxResults`, range 1..100).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The members `PrivacyBudgetTemplateSummary` declares (model shape
/// `PrivacyBudgetTemplateSummary`), each of which the full `PrivacyBudgetTemplate`
/// carries under the same name.
const SUMMARY_MEMBERS: [&str; 9] = [
    "id",
    "arn",
    "membershipId",
    "membershipArn",
    "collaborationId",
    "collaborationArn",
    "privacyBudgetType",
    "createTime",
    "updateTime",
];

/// The members of the full `PrivacyBudgetTemplate` that the summary shape does NOT
/// declare.
const MEMBERS_NOT_IN_SUMMARY: [&str; 2] = ["autoRefresh", "parameters"];

/// A collaboration plus the caller's membership in it:
/// `(collaboration id, membership id, membership arn)`.
async fn membership_in_new_collaboration(emulator: &Emulator, name: &str) -> (String, String, String) {
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration(name))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no collaboration.id in {created}"))
        .to_string();

    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": ALL_LOG_STATUSES[0],
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateMembership 200, got {status} with {joined}"
    );
    let membership_id = joined["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no membership.id in {joined}"))
        .to_string();
    let membership_arn = joined["membership"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no membership.arn in {joined}"))
        .to_string();
    (collaboration_id, membership_id, membership_arn)
}

/// The create request for one `PrivacyBudgetType`: each type's `parameters` union member
/// carries the parameters of exactly that type.
fn template_body(privacy_budget_type: &str, membership_arn: &str) -> Value {
    let parameters = if privacy_budget_type == ALL_PRIVACY_BUDGET_TYPES[0] {
        json!({ "differentialPrivacy": { "epsilon": 5, "usersNoisePerQuery": 20 } })
    } else {
        json!({
            "accessBudget": {
                "budgetParameters": [{ "type": "CALENDAR_DAY", "budget": 100 }],
                "resourceArn": format!("{membership_arn}/configuredtableassociation/0a1b2c3d"),
            }
        })
    };
    json!({
        "privacyBudgetType": privacy_budget_type,
        "parameters": parameters,
    })
}

async fn delete_template(
    emulator: &Emulator,
    membership_id: &str,
    template_id: &str,
) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!("/memberships/{membership_id}/privacybudgettemplates/{template_id}"),
            None,
        )
        .await
}

/// `ListPrivacyBudgetTemplates`, with `query` appended to the path as given.
async fn list_templates(emulator: &Emulator, membership_id: &str, query: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/memberships/{membership_id}/privacybudgettemplates{query}"),
            None,
        )
        .await
}

/// The ids the listing reports for `membership_id`, walking every page.
async fn listed_ids(emulator: &Emulator, membership_id: &str, max_results: Option<i64>) -> Vec<String> {
    let case = format!("membership={membership_id} maxResults={max_results:?}");
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let query = match (&token, max_results) {
            (Some(t), Some(max)) => format!("?maxResults={max}&nextToken={t}"),
            (Some(t), None) => format!("?nextToken={t}"),
            (None, Some(max)) => format!("?maxResults={max}"),
            (None, None) => String::new(),
        };
        let (status, body) = list_templates(emulator, membership_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected ListPrivacyBudgetTemplates 200, got {status} with {body}"
        );
        let page = body["privacyBudgetTemplateSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no privacyBudgetTemplateSummaries in {body}"));
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "case {case}: page carried {} rows, more than asked",
                page.len()
            );
        }
        for summary in page {
            seen.push(
                summary["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a summary carried no id: {summary}"))
                    .to_string(),
            );
        }
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        pages += 1;
        assert!(
            pages <= 20,
            "case {case}: pagination did not terminate, saw {seen:?}"
        );
        if token.is_none() {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn list_privacy_budget_templates_pages_the_collaborations_templates_exactly_once() {
    let emulator = Emulator::new();

    // One collaboration holds a template of EVERY budget type (the uniqueness key is
    // (collaboration, privacyBudgetType), so that is the largest set one collaboration
    // can hold), a second holds one, and a third holds none.
    let (_, full_membership, full_arn) =
        membership_in_new_collaboration(&emulator, "list-full").await;
    let mut created: Vec<String> = Vec::new();
    for budget_type in ALL_PRIVACY_BUDGET_TYPES {
        let (status, body) =
            create_template(&emulator, &full_membership, template_body(budget_type, &full_arn)).await;
        assert_eq!(
            status, 200,
            "setup {budget_type:?}: CreatePrivacyBudgetTemplate failed with {status}: {body}"
        );
        created.push(
            body["privacyBudgetTemplate"]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("setup {budget_type:?}: no template id in {body}"))
                .to_string(),
        );
    }

    let (_, one_membership, one_arn) = membership_in_new_collaboration(&emulator, "list-one").await;
    let (status, body) = create_template(
        &emulator,
        &one_membership,
        template_body(ALL_PRIVACY_BUDGET_TYPES[1], &one_arn),
    )
    .await;
    assert_eq!(status, 200, "setup list-one: create failed: {body}");
    let one_id = body["privacyBudgetTemplate"]["id"]
        .as_str()
        .expect("setup list-one: template id")
        .to_string();

    let (_, empty_membership, _) = membership_in_new_collaboration(&emulator, "list-empty").await;

    // Rows differ in page size: both published bounds of `maxResults`, a size that
    // divides the set evenly, one that does not, and unpaged.
    for max_results in [
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(MAX_RESULTS_MAX),
        None,
    ] {
        let seen = listed_ids(&emulator, &full_membership, max_results).await;
        assert_eq!(
            seen, created,
            "case maxResults={max_results:?}: paging must yield every template of the \
             collaboration once, in creation order"
        );

        // Scoping: another collaboration's listing reports only its own template, and a
        // collaboration with none reports an empty list.
        let seen = listed_ids(&emulator, &one_membership, max_results).await;
        assert_eq!(
            seen,
            vec![one_id.clone()],
            "case maxResults={max_results:?}: a second collaboration must report only its own \
             template"
        );
        let seen = listed_ids(&emulator, &empty_membership, max_results).await;
        assert!(
            seen.is_empty(),
            "case maxResults={max_results:?}: a collaboration with no template must report an \
             empty list, got {seen:?}"
        );
    }

    // A complete page carries no token, and the required member is present even when the
    // collaboration holds nothing.
    let (status, body) = list_templates(&emulator, &empty_membership, "").await;
    assert_eq!(status, 200, "case empty: expected 200, got {status}: {body}");
    assert_eq!(
        body["privacyBudgetTemplateSummaries"].as_array().map(Vec::len),
        Some(0),
        "case empty: expected an empty privacyBudgetTemplateSummaries array, got {body}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "case empty: a complete page must carry no nextToken, got {:?}",
        body.get("nextToken")
    );
}

#[tokio::test]
async fn each_listed_summary_is_the_stored_template_narrowed_to_the_summary_shape() {
    let emulator = Emulator::new();
    let (collaboration_id, membership_id, membership_arn) =
        membership_in_new_collaboration(&emulator, "summaries").await;

    // Rows: every budget type, so neither type's projection is privileged.
    for budget_type in ALL_PRIVACY_BUDGET_TYPES {
        let (status, body) = create_template(
            &emulator,
            &membership_id,
            template_body(budget_type, &membership_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {budget_type:?}: create failed with {status}: {body}"
        );
    }

    let (status, listed) = list_templates(&emulator, &membership_id, "").await;
    assert_eq!(status, 200, "listing failed with {status}: {listed}");
    let summaries = listed["privacyBudgetTemplateSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no privacyBudgetTemplateSummaries in {listed}"))
        .clone();
    assert_eq!(
        summaries.len(),
        ALL_PRIVACY_BUDGET_TYPES.len(),
        "expected one summary per created template, got {listed}"
    );

    for summary in &summaries {
        let id = summary["id"]
            .as_str()
            .unwrap_or_else(|| panic!("a summary carried no id: {summary}"));
        assert!(
            is_model_uuid(id),
            "case {id:?}: the summary id must have the modelled form, got {id:?}"
        );
        let (status, read) = get_template(&emulator, &membership_id, id).await;
        assert_eq!(
            status, 200,
            "case {id:?}: the listed template must be readable, got {status} with {read}"
        );
        let stored = &read["privacyBudgetTemplate"];
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                summary.get(member),
                stored.get(member),
                "case {id:?}: summary {member} must be the stored template's, got {:?} vs {:?}",
                summary.get(member),
                stored.get(member)
            );
        }
        for member in MEMBERS_NOT_IN_SUMMARY {
            assert!(
                summary.get(member).is_none(),
                "case {id:?}: the summary shape declares no {member}, got {:?}",
                summary.get(member)
            );
        }
        assert_eq!(
            summary["collaborationId"].as_str(),
            Some(collaboration_id.as_str()),
            "case {id:?}: expected collaborationId {collaboration_id}, got {:?}",
            summary["collaborationId"]
        );
        assert_eq!(
            summary["membershipId"].as_str(),
            Some(membership_id.as_str()),
            "case {id:?}: expected membershipId {membership_id}, got {:?}",
            summary["membershipId"]
        );
    }
}

#[tokio::test]
async fn the_listing_follows_every_create_and_delete() {
    let emulator = Emulator::new();

    // Rows: both orders of creating the two budget types, so the reported order is the
    // order the creates happened in rather than a fixed order of the catalog.
    let orders: [[&str; 2]; 2] = [
        [ALL_PRIVACY_BUDGET_TYPES[0], ALL_PRIVACY_BUDGET_TYPES[1]],
        [ALL_PRIVACY_BUDGET_TYPES[1], ALL_PRIVACY_BUDGET_TYPES[0]],
    ];
    for (row, order) in orders.into_iter().enumerate() {
        let (_, membership_id, membership_arn) =
            membership_in_new_collaboration(&emulator, &format!("follows-{row}")).await;

        let seen = listed_ids(&emulator, &membership_id, None).await;
        assert!(
            seen.is_empty(),
            "case {order:?}: before any create the listing must be empty, got {seen:?}"
        );

        let mut expected: Vec<String> = Vec::new();
        for budget_type in order {
            let (status, body) = create_template(
                &emulator,
                &membership_id,
                template_body(budget_type, &membership_arn),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {order:?}/{budget_type:?}: create failed with {status}: {body}"
            );
            expected.push(
                body["privacyBudgetTemplate"]["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {order:?}: no template id in {body}"))
                    .to_string(),
            );
            let seen = listed_ids(&emulator, &membership_id, None).await;
            assert_eq!(
                seen, expected,
                "case {order:?}/{budget_type:?}: the listing must report exactly the created \
                 templates"
            );
        }

        // A delete removes exactly its own row, in both directions.
        while let Some(gone) = expected.first().cloned() {
            let (status, body) = delete_template(&emulator, &membership_id, &gone).await;
            assert_eq!(
                status, 204,
                "case {order:?}/{gone:?}: delete failed with {status}: {body}"
            );
            expected.remove(0);
            let seen = listed_ids(&emulator, &membership_id, None).await;
            assert_eq!(
                seen, expected,
                "case {order:?}/{gone:?}: a deleted template must leave the listing"
            );
        }
    }
}

#[tokio::test]
async fn the_listing_refuses_input_the_state_or_the_model_cannot_satisfy() {
    let emulator = Emulator::new();
    let (_, membership_id, membership_arn) =
        membership_in_new_collaboration(&emulator, "list-refusals").await;
    let (status, body) = create_template(
        &emulator,
        &membership_id,
        template_body(ALL_PRIVACY_BUDGET_TYPES[0], &membership_arn),
    )
    .await;
    assert_eq!(status, 200, "setup: create failed with {status}: {body}");

    // A membership the store does not hold: the collaboration the templates would be
    // retrieved from cannot be resolved.
    let (status, refused) = list_templates(&emulator, ABSENT_UUID, "").await;
    assert_eq!(
        status, 404,
        "case absent membership: expected 404, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case absent membership: expected ResourceNotFoundException, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        format!("No such membership: {ABSENT_UUID} for account {}", account_id()),
        "case absent membership: unexpected message in {refused}"
    );
    assert_eq!(
        refused["resourceId"].as_str(),
        Some(ABSENT_UUID),
        "case absent membership: expected resourceId {ABSENT_UUID}, got {refused}"
    );
    assert_eq!(
        refused["resourceType"].as_str(),
        Some("MEMBERSHIP"),
        "case absent membership: expected resourceType MEMBERSHIP, got {refused}"
    );

    // One past each published bound of `maxResults`.
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let (status, refused) =
            list_templates(&emulator, &membership_id, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={max_results}: expected a 400 refusal, got {status} with {refused}"
        );
    }
    // And each bound itself is accepted.
    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let (status, body) =
            list_templates(&emulator, &membership_id, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 200,
            "case maxResults={max_results}: a published bound must be accepted, got {status} \
             with {body}"
        );
    }

    // A token the service never handed out.
    for token in ["not-hex", "6e6f742d612d6b6e6f776e2d6964", ""] {
        let (status, refused) =
            list_templates(&emulator, &membership_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case nextToken={token:?}: expected a 400 refusal, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case nextToken={token:?}: expected ValidationException, got {refused}"
        );
    }
}

#[tokio::test]
async fn listing_privacy_budget_templates_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let (_, membership_id, membership_arn) =
        membership_in_new_collaboration(&emulator, "list-is-a-read").await;
    for budget_type in ALL_PRIVACY_BUDGET_TYPES {
        let (status, body) = create_template(
            &emulator,
            &membership_id,
            template_body(budget_type, &membership_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup {budget_type:?}: create failed with {status}: {body}"
        );
    }

    // Rows: the same request repeated at each page size, including a partial first page.
    for max_results in [Some(MAX_RESULTS_MIN), Some(MAX_RESULTS_MAX), None] {
        let query = match max_results {
            Some(max) => format!("?maxResults={max}"),
            None => String::new(),
        };
        let (_, first) = list_templates(&emulator, &membership_id, &query).await;
        let (_, again) = list_templates(&emulator, &membership_id, &query).await;
        assert_eq!(
            first, again,
            "case maxResults={max_results:?}: a read must answer the same twice"
        );
        let whole = listed_ids(&emulator, &membership_id, max_results).await;
        let after = listed_ids(&emulator, &membership_id, max_results).await;
        assert_eq!(
            whole, after,
            "case maxResults={max_results:?}: paging through the listing must not consume it"
        );
    }
}
