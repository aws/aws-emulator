// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the resource-share slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: each of these operations answers from the ONE persisted
//! resource-share record and the association records stored beside it, so anything a
//! create, an associate or an update stored is what a later `GetResourceShares` or
//! `GetResourceShareAssociations` reports, and anything deleted is gone.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    ASSOCIATION_READY_AFTER_MILLIS, MAX_ENTITIES_PER_CALL, MAX_PAGE_SIZE, MAX_TAGS_PER_SHARE,
};

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// A row of the create/read-back table.
struct ShareCase {
    /// The share name, which is also how the row is reported in a failure.
    name: &'static str,
    /// The create request body.
    request: Value,
    /// `allowExternalPrincipals` the stored share must carry.
    allow_external_principals: bool,
    /// The tags the stored share must carry, in key order. A share with no tags
    /// reports the empty list, never an absent member.
    tags: Value,
    /// The `resourceShareConfiguration` the stored share must report: the one the
    /// request named, with this service's default for every member it left out.
    configuration: Value,
    /// `(associationType, associatedEntity, external)` for every association the
    /// create must have materialised.
    associations: Vec<(&'static str, &'static str, bool)>,
}

/// The configuration a share created without an explicit one reports: the modelled
/// `ResourceShareConfiguration` with its member at this service's default.
fn default_configuration() -> Value {
    json!({
        "retainSharingOnAccountLeaveOrganization": false,
    })
}

/// The table. The rule is "a read reports what the create stored", so the rows differ
/// along what the rule generalises over: which optional members are present, how many
/// entities of each type are named, whether an entity is inside the owning account,
/// the `allowExternalPrincipals` value (absent, true, false) and the tag count.
fn share_cases() -> Vec<ShareCase> {
    vec![
        // Only the required member. `allowExternalPrincipals` "default value is
        // true" (that member's own documentation), and nothing is associated.
        ShareCase {
            name: "share-minimal",
            request: json!({ "name": "share-minimal" }),
            allow_external_principals: true,
            tags: json!([]),
            configuration: default_configuration(),
            associations: vec![],
        },
        // One resource inside the owning account: an internal association.
        ShareCase {
            name: "share-one-local-resource",
            request: json!({
                "name": "share-one-local-resource",
                "resourceArns": [LOCAL_RESOURCE],
            }),
            allow_external_principals: true,
            tags: json!([]),
            configuration: default_configuration(),
            associations: vec![("RESOURCE", LOCAL_RESOURCE, false)],
        },
        // Two resources — one of them owned elsewhere — two principals, external
        // principals refused, and two tags.
        ShareCase {
            name: "share-mixed",
            request: json!({
                "name": "share-mixed",
                "resourceArns": [LOCAL_RESOURCE, FOREIGN_RESOURCE],
                "principals": ["444455556666", ACCOUNT],
                "allowExternalPrincipals": false,
                "tags": [
                    { "key": "Team", "value": "platform" },
                    { "key": "Env", "value": "test" },
                ],
            }),
            allow_external_principals: false,
            tags: json!([
                { "key": "Env", "value": "test" },
                { "key": "Team", "value": "platform" },
            ]),
            configuration: default_configuration(),
            associations: vec![
                ("RESOURCE", LOCAL_RESOURCE, false),
                ("RESOURCE", FOREIGN_RESOURCE, true),
                ("PRINCIPAL", "444455556666", true),
                ("PRINCIPAL", ACCOUNT, false),
            ],
        },
        // Sources and a service principal, `allowExternalPrincipals` spelled out,
        // permission ARNs stored for the `permissionArn` filter, and a tag whose
        // value is the empty string ("The value can be an empty string").
        ShareCase {
            name: "share-sources",
            request: json!({
                "name": "share-sources",
                "principals": ["lambda.amazonaws.com"],
                "sources": ["111122223333", "arn:aws:organizations::000000000000:ou/o-abc/ou-def"],
                "allowExternalPrincipals": true,
                "permissionArns": ["arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSubnet"],
                "tags": [{ "key": "Owner", "value": "" }],
            }),
            allow_external_principals: true,
            tags: json!([{ "key": "Owner", "value": "" }]),
            configuration: default_configuration(),
            associations: vec![
                ("PRINCIPAL", "lambda.amazonaws.com", true),
                ("SOURCE", "111122223333", true),
                (
                    "SOURCE",
                    "arn:aws:organizations::000000000000:ou/o-abc/ou-def",
                    false,
                ),
            ],
        },
        // The resource-share configuration the model admits, carried through the
        // store untouched.
        ShareCase {
            name: "share-configured",
            request: json!({
                "name": "share-configured",
                "resourceArns": [LOCAL_RESOURCE_TWO],
                "resourceShareConfiguration": {
                    "retainSharingOnAccountLeaveOrganization": true
                },
            }),
            allow_external_principals: true,
            tags: json!([]),
            configuration: json!({
                "retainSharingOnAccountLeaveOrganization": true,
            }),
            associations: vec![("RESOURCE", LOCAL_RESOURCE_TWO, false)],
        },
    ]
}

#[tokio::test]
async fn a_create_is_reported_by_every_later_read() {
    let router = emulator();

    for case in share_cases() {
        let label = case.name;
        let created = create_ok(&router, label, &case.request).await;
        let arn = arn_of(label, &created);
        assert_share_arn(label, &arn);

        assert_eq!(
            created.get("name").and_then(Value::as_str),
            Some(label),
            "case {label:?}: create reported the wrong name, got {created:?}"
        );
        assert_eq!(
            created.get("owningAccountId").and_then(Value::as_str),
            Some(ACCOUNT),
            "case {label:?}: create reported the wrong owningAccountId, got {created:?}"
        );
        assert_eq!(
            created.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {label:?}: a share RAM accepts is ACTIVE, got {created:?}"
        );
        assert_eq!(
            created.get("featureSet").and_then(Value::as_str),
            Some("STANDARD"),
            "case {label:?}: a share created through the API is STANDARD, got {created:?}"
        );
        assert_eq!(
            created.get("allowExternalPrincipals").and_then(Value::as_bool),
            Some(case.allow_external_principals),
            "case {label:?}: wrong allowExternalPrincipals, got {created:?}"
        );
        assert_eq!(
            created.get("tags").cloned().unwrap_or(Value::Null),
            case.tags,
            "case {label:?}: wrong tags, got {created:?}"
        );
        assert_eq!(
            created
                .get("resourceShareConfiguration")
                .cloned()
                .unwrap_or(Value::Null),
            case.configuration,
            "case {label:?}: wrong resourceShareConfiguration, got {created:?}"
        );
        let created_at = timestamp(label, &created, "creationTime");
        let updated_at = timestamp(label, &created, "lastUpdatedTime");
        assert!(
            (created_at - updated_at).abs() < f64::EPSILON,
            "case {label:?}: a share never updated reports creationTime {created_at} and \
             lastUpdatedTime {updated_at}"
        );

        // The same record, read back through GetResourceShares by ARN.
        let (status, listed) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetResourceShares expected 200, got {status} with {listed:?}"
        );
        assert_eq!(
            shares_of(label, &listed),
            vec![created.clone()],
            "case {label:?}: the read-back must be the record the create reported"
        );

        // And the associations the create materialised, by type.
        for association_type in ["RESOURCE", "PRINCIPAL", "SOURCE"] {
            let expected: Vec<(&str, bool)> = case
                .associations
                .iter()
                .filter(|(kind, _, _)| *kind == association_type)
                .map(|(_, entity, external)| (*entity, *external))
                .collect();
            let (status, found) = get_associations(
                &router,
                &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}/{association_type}: expected 200, got {status} with {found:?}"
            );
            let reported = associations_of(label, &found);
            let actual: Vec<(String, bool)> = reported
                .iter()
                .map(|association| {
                    (
                        association
                            .get("associatedEntity")
                            .and_then(Value::as_str)
                            .unwrap_or("<none>")
                            .to_string(),
                        association
                            .get("external")
                            .and_then(Value::as_bool)
                            .unwrap_or_default(),
                    )
                })
                .collect();
            let want: Vec<(String, bool)> = expected
                .iter()
                .map(|(entity, external)| ((*entity).to_string(), *external))
                .collect();
            let mut sorted_actual = actual.clone();
            sorted_actual.sort();
            let mut sorted_want = want.clone();
            sorted_want.sort();
            assert_eq!(
                sorted_actual, sorted_want,
                "case {label:?}/{association_type}: wrong associated entities"
            );
            for association in &reported {
                assert_eq!(
                    association.get("resourceShareArn").and_then(Value::as_str),
                    Some(arn.as_str()),
                    "case {label:?}/{association_type}: association names the wrong share"
                );
                assert_eq!(
                    association.get("resourceShareName").and_then(Value::as_str),
                    Some(label),
                    "case {label:?}/{association_type}: association names the wrong share name"
                );
                assert_eq!(
                    association.get("associationType").and_then(Value::as_str),
                    Some(association_type),
                    "case {label:?}/{association_type}: wrong associationType"
                );
                assert_eq!(
                    association.get("status").and_then(Value::as_str),
                    Some("ASSOCIATING"),
                    "case {label:?}/{association_type}: a new association is ASSOCIATING until \
                     the move RAM owes it falls due, got {association:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_association_reaches_associated_when_the_clock_makes_it_due() {
    let router = emulator();
    let case = "settles";
    let created = create_ok(
        &router,
        case,
        &json!({
            "name": "share-settles",
            "resourceArns": [LOCAL_RESOURCE],
            "principals": ["444455556666"],
        }),
    )
    .await;
    let arn = arn_of(case, &created);

    // The span is never zero, so a caller reading straight back still sees the
    // transient status.
    for association_type in ["RESOURCE", "PRINCIPAL"] {
        let (_, found) = get_associations(
            &router,
            &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
        )
        .await;
        for association in associations_of(case, &found) {
            assert_eq!(
                association.get("status").and_then(Value::as_str),
                Some("ASSOCIATING"),
                "case {case:?}/{association_type}: expected ASSOCIATING before the move falls \
                 due, got {association:?}"
            );
        }
    }

    // Nothing sleeps server-side: the move is materialised by the first call at or
    // after the instant it fell due.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(ASSOCIATION_READY_AFTER_MILLIS).unwrap_or_default() + 100,
    ));

    for association_type in ["RESOURCE", "PRINCIPAL"] {
        let (status, found) = get_associations(
            &router,
            &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}/{association_type}: expected 200, got {status} with {found:?}"
        );
        let reported = associations_of(case, &found);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}/{association_type}: expected one association, got {reported:?}"
        );
        for association in &reported {
            assert_eq!(
                association.get("status").and_then(Value::as_str),
                Some("ASSOCIATED"),
                "case {case:?}/{association_type}: expected ASSOCIATED once due, got \
                 {association:?}"
            );
            let created_at = timestamp(case, association, "creationTime");
            let updated_at = timestamp(case, association, "lastUpdatedTime");
            assert!(
                updated_at > created_at,
                "case {case:?}/{association_type}: the move must be stamped after creation, got \
                 creationTime {created_at} and lastUpdatedTime {updated_at}"
            );
        }
    }

    // Asking for the status it no longer has reports nothing; asking for the one it
    // now has reports it.
    for (wanted, expected) in [("ASSOCIATING", 0usize), ("ASSOCIATED", 1usize)] {
        let (_, found) = get_associations(
            &router,
            &json!({
                "associationType": "PRINCIPAL",
                "resourceShareArns": [arn],
                "associationStatus": wanted,
            }),
        )
        .await;
        assert_eq!(
            associations_of(case, &found).len(),
            expected,
            "case {case:?}: associationStatus={wanted} reported the wrong count, got {found:?}"
        );
    }
}

/// A row of the `GetResourceShares` filter table: a request body and the share names
/// it must report, sorted.
struct FilterCase {
    case: &'static str,
    request: Value,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn get_resource_shares_filters_the_stored_shares() {
    let router = emulator();
    for case in share_cases() {
        create_ok(&router, case.name, &case.request).await;
    }
    let all = vec![
        "share-configured",
        "share-minimal",
        "share-mixed",
        "share-one-local-resource",
        "share-sources",
    ];

    // The rule is "a filter selects the stored shares that match it", so the rows
    // differ along which filter is used, whether it matches none, one or several
    // shares, and — for `tagFilters` — the key-only, key-and-value, several-filters
    // (OR) and empty-value forms the modelled `TagFilter` documents.
    let cases = vec![
        FilterCase {
            case: "no filter",
            request: json!({ "resourceOwner": "SELF" }),
            expected: all.clone(),
        },
        FilterCase {
            case: "owned elsewhere",
            request: json!({ "resourceOwner": "OTHER-ACCOUNTS" }),
            expected: vec![],
        },
        FilterCase {
            case: "name that exists",
            request: json!({ "resourceOwner": "SELF", "name": "share-mixed" }),
            expected: vec!["share-mixed"],
        },
        FilterCase {
            case: "name that does not exist",
            request: json!({ "resourceOwner": "SELF", "name": "share-absent" }),
            expected: vec![],
        },
        FilterCase {
            case: "status ACTIVE",
            request: json!({ "resourceOwner": "SELF", "resourceShareStatus": "ACTIVE" }),
            expected: all.clone(),
        },
        FilterCase {
            case: "status DELETED",
            request: json!({ "resourceOwner": "SELF", "resourceShareStatus": "DELETED" }),
            expected: vec![],
        },
        FilterCase {
            case: "status PENDING",
            request: json!({ "resourceOwner": "SELF", "resourceShareStatus": "PENDING" }),
            expected: vec![],
        },
        FilterCase {
            case: "tag key only",
            request: json!({
                "resourceOwner": "SELF",
                "tagFilters": [{ "tagKey": "Team" }],
            }),
            expected: vec!["share-mixed"],
        },
        FilterCase {
            case: "tag key and matching value",
            request: json!({
                "resourceOwner": "SELF",
                "tagFilters": [{ "tagKey": "Team", "tagValues": ["platform", "network"] }],
            }),
            expected: vec!["share-mixed"],
        },
        FilterCase {
            case: "tag key and non-matching value",
            request: json!({
                "resourceOwner": "SELF",
                "tagFilters": [{ "tagKey": "Team", "tagValues": ["network"] }],
            }),
            expected: vec![],
        },
        FilterCase {
            case: "two tag filters are an OR",
            request: json!({
                "resourceOwner": "SELF",
                "tagFilters": [{ "tagKey": "Team" }, { "tagKey": "Owner" }],
            }),
            expected: vec!["share-mixed", "share-sources"],
        },
        FilterCase {
            case: "tag value that is the empty string",
            request: json!({
                "resourceOwner": "SELF",
                "tagFilters": [{ "tagKey": "Owner", "tagValues": [""] }],
            }),
            expected: vec!["share-sources"],
        },
        FilterCase {
            case: "tag key nothing carries",
            request: json!({
                "resourceOwner": "SELF",
                "tagFilters": [{ "tagKey": "Absent" }],
            }),
            expected: vec![],
        },
        FilterCase {
            case: "permission the share was created with",
            request: json!({
                "resourceOwner": "SELF",
                "permissionArn": "arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSubnet",
            }),
            expected: vec!["share-sources"],
        },
        FilterCase {
            case: "permission no share was created with",
            request: json!({
                "resourceOwner": "SELF",
                "permissionArn": "arn:aws:ram::aws:permission/AWSRAMDefaultPermissionOther",
            }),
            expected: vec![],
        },
        FilterCase {
            case: "name and status together",
            request: json!({
                "resourceOwner": "SELF",
                "name": "share-sources",
                "resourceShareStatus": "ACTIVE",
            }),
            expected: vec!["share-sources"],
        },
    ];

    for FilterCase {
        case,
        request,
        expected,
    } in cases
    {
        let (status, listed) = get_shares(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {listed:?}"
        );
        assert_eq!(
            names_of(case, &listed),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: wrong shares for {request}"
        );
    }
}

#[tokio::test]
async fn pagination_walks_every_share_exactly_once() {
    let router = emulator();
    let total = 7usize;
    let mut expected: Vec<String> = Vec::with_capacity(total);
    for index in 0..total {
        let name = format!("share-page-{index}");
        create_ok(&router, &name, &json!({ "name": name })).await;
        expected.push(name);
    }
    expected.sort();

    // One page size per row: below, at and above the number of shares, plus the
    // published maximum of the modelled `MaxResults` range (1..500).
    for page in [1usize, 2, 3, total, MAX_PAGE_SIZE] {
        let case = format!("maxResults={page}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "resourceOwner": "SELF", "maxResults": page });
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, listed) = get_shares(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {listed:?}"
            );
            let names = names_of(&case, &listed);
            assert!(
                names.len() <= page,
                "case {case:?}: a page carried {} shares, more than the {page} asked for",
                names.len()
            );
            seen.extend(names);
            next_token = listed
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: a share was reported twice across pages, got {seen:?}"
        );
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages together must report every stored share"
        );
    }

    // Each published bound of the modelled range, and one step past each.
    for (max_results, expect_ok) in [(1, true), (500, true), (0, false), (501, false)] {
        let case = format!("bound maxResults={max_results}");
        let (status, body) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "maxResults": max_results }),
        )
        .await;
        if expect_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a value outside the modelled range must be refused, got {status} \
                 with {body:?}"
            );
        }
    }

    // A token this service never issued.
    for token in ["not-a-token", "6e6f70652d6e6f742d616e2d61726e"] {
        let case = format!("nextToken={token}");
        let (status, body) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "nextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNextTokenException",
            "case {case:?}: wrong error, got {body:?}"
        );
    }

    // And the same for the association pagination, over one share's associations.
    let entities: Vec<String> = (0..5).map(|index| format!("44445555666{index}")).collect();
    let created = create_ok(
        &router,
        "association pages",
        &json!({ "name": "share-association-pages", "principals": entities }),
    )
    .await;
    let arn = arn_of("association pages", &created);
    for page in [1usize, 2, 5] {
        let case = format!("associations maxResults={page}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({
                "associationType": "PRINCIPAL",
                "resourceShareArns": [arn],
                "maxResults": page,
            });
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, found) = get_associations(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {found:?}"
            );
            let reported = associations_of(&case, &found);
            assert!(
                reported.len() <= page,
                "case {case:?}: a page carried {} associations, more than the {page} asked for",
                reported.len()
            );
            seen.extend(reported.iter().map(|association| {
                association
                    .get("associatedEntity")
                    .and_then(Value::as_str)
                    .unwrap_or("<none>")
                    .to_string()
            }));
            next_token = found
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        seen.sort();
        let mut want = entities.clone();
        want.sort();
        assert_eq!(
            seen, want,
            "case {case:?}: the pages together must report every stored association"
        );
    }
    let (status, body) = get_associations(
        &router,
        &json!({
            "associationType": "PRINCIPAL",
            "nextToken": "not-a-token",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"association nextToken\": expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidNextTokenException",
        "case \"association nextToken\": wrong error, got {body:?}"
    );
}

/// A row of the update table.
struct UpdateCase {
    case: &'static str,
    /// The name the share is created with.
    initial_name: &'static str,
    /// `allowExternalPrincipals` the share is created with.
    initial_allow: bool,
    /// The members the update names, beside `resourceShareArn`.
    patch: Value,
    expected_name: &'static str,
    expected_allow: bool,
}

#[tokio::test]
async fn an_update_changes_only_what_it_names_and_is_visible_on_re_read() {
    let router = emulator();

    // The rule is "an update stores the members it names and nothing else", so the
    // rows differ along which of the two mutable members is named, and along the
    // direction the boolean moves.
    let cases = vec![
        UpdateCase {
            case: "name only",
            initial_name: "u-name",
            initial_allow: true,
            patch: json!({ "name": "u-name-renamed" }),
            expected_name: "u-name-renamed",
            expected_allow: true,
        },
        UpdateCase {
            case: "flag to false",
            initial_name: "u-flag-off",
            initial_allow: true,
            patch: json!({ "allowExternalPrincipals": false }),
            expected_name: "u-flag-off",
            expected_allow: false,
        },
        UpdateCase {
            case: "flag to true",
            initial_name: "u-flag-on",
            initial_allow: false,
            patch: json!({ "allowExternalPrincipals": true }),
            expected_name: "u-flag-on",
            expected_allow: true,
        },
        UpdateCase {
            case: "both members",
            initial_name: "u-both",
            initial_allow: true,
            patch: json!({ "name": "u-both-renamed", "allowExternalPrincipals": false }),
            expected_name: "u-both-renamed",
            expected_allow: false,
        },
    ];

    for case in cases {
        let label = case.case;
        let created = create_ok(
            &router,
            label,
            &json!({
                "name": case.initial_name,
                "allowExternalPrincipals": case.initial_allow,
                "principals": ["444455556666"],
            }),
        )
        .await;
        let arn = arn_of(label, &created);
        let created_at = timestamp(label, &created, "creationTime");

        let mut request = case.patch.clone();
        request["resourceShareArn"] = json!(arn);
        let (status, updated) = update(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: update expected 200, got {status} with {updated:?}"
        );
        let share = updated
            .get("resourceShare")
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: update reported no share, got {updated:?}"));
        assert_eq!(
            share.get("name").and_then(Value::as_str),
            Some(case.expected_name),
            "case {label:?}: wrong name after the update, got {share:?}"
        );
        assert_eq!(
            share.get("allowExternalPrincipals").and_then(Value::as_bool),
            Some(case.expected_allow),
            "case {label:?}: wrong allowExternalPrincipals after the update, got {share:?}"
        );
        let updated_at = timestamp(label, &share, "lastUpdatedTime");
        assert!(
            (timestamp(label, &share, "creationTime") - created_at).abs() < f64::EPSILON,
            "case {label:?}: an update must not move creationTime, got {share:?}"
        );
        assert!(
            updated_at > created_at,
            "case {label:?}: expected a lastUpdatedTime after the creationTime {created_at}, got \
             {updated_at}"
        );

        // The re-read reports exactly the updated record.
        let (_, listed) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            shares_of(label, &listed),
            vec![share.clone()],
            "case {label:?}: the re-read must report the updated record"
        );
        // And is found under its new name, never under the old one if it changed.
        let (_, by_name) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "name": case.expected_name }),
        )
        .await;
        assert_eq!(
            names_of(label, &by_name),
            vec![case.expected_name.to_string()],
            "case {label:?}: the share must be found under its stored name"
        );
        if case.expected_name != case.initial_name {
            let (_, by_old_name) = get_shares(
                &router,
                &json!({ "resourceOwner": "SELF", "name": case.initial_name }),
            )
            .await;
            assert!(
                names_of(label, &by_old_name).is_empty(),
                "case {label:?}: the share must no longer be found under its old name, got \
                 {by_old_name:?}"
            );
        }
        // Every association reports the share's name, so a rename reaches them.
        let (_, found) = get_associations(
            &router,
            &json!({ "associationType": "PRINCIPAL", "resourceShareArns": [arn] }),
        )
        .await;
        for association in associations_of(label, &found) {
            assert_eq!(
                association.get("resourceShareName").and_then(Value::as_str),
                Some(case.expected_name),
                "case {label:?}: an association must report the share's stored name, got \
                 {association:?}"
            );
        }
    }

    // An update that names neither mutable member asks for no change at all.
    let created = create_ok(&router, "empty update", &json!({ "name": "u-empty" })).await;
    let arn = arn_of("empty update", &created);
    let (status, body) = update(&router, &json!({ "resourceShareArn": arn })).await;
    assert_eq!(
        status, 400,
        "case \"empty update\": expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "MissingRequiredParameterException",
        "case \"empty update\": wrong error, got {body:?}"
    );
}

#[tokio::test]
async fn a_delete_removes_the_share_and_everything_it_shared() {
    let router = emulator();
    let case = "delete";
    let created = create_ok(
        &router,
        case,
        &json!({
            "name": "share-deleted",
            "resourceArns": [LOCAL_RESOURCE],
            "principals": ["444455556666"],
        }),
    )
    .await;
    let arn = arn_of(case, &created);

    let (status, deleted) = delete(&router, &arn, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: delete expected 200, got {status} with {deleted:?}"
    );
    assert_eq!(
        deleted.get("returnValue").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: a delete that succeeded reports returnValue true, got {deleted:?}"
    );
    assert!(
        deleted
            .get("clientToken")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.is_empty()),
        "case {case:?}: RAM generates a clientToken when the request omits one, got {deleted:?}"
    );

    // The record survives the delete: a read that names no status reports it in its
    // terminal DELETED state, so a caller can still observe the share it deleted.
    let (_, listed) = get_shares(&router, &json!({ "resourceOwner": "SELF" })).await;
    assert_eq!(
        names_of(case, &listed),
        vec!["share-deleted".to_string()],
        "case {case:?}: a deleted share stays retrievable, got {listed:?}"
    );
    assert_eq!(
        shares_of(case, &listed)[0].get("status").and_then(Value::as_str),
        Some("DELETED"),
        "case {case:?}: the retained record must report DELETED, got {listed:?}"
    );
    // And to a read that asks for deleted shares by status.
    let (_, tombstoned) = get_shares(
        &router,
        &json!({ "resourceOwner": "SELF", "resourceShareStatus": "DELETED" }),
    )
    .await;
    let reported = shares_of(case, &tombstoned);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the tombstone, got {tombstoned:?}"
    );
    assert_eq!(
        reported[0].get("status").and_then(Value::as_str),
        Some("DELETED"),
        "case {case:?}: the tombstone must report DELETED, got {tombstoned:?}"
    );
    // Its associations are gone with it.
    for association_type in ["RESOURCE", "PRINCIPAL", "SOURCE"] {
        let (_, found) = get_associations(
            &router,
            &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
        )
        .await;
        assert!(
            associations_of(case, &found).is_empty(),
            "case {case:?}/{association_type}: a deleted share has no associations, got {found:?}"
        );
    }

    // And nothing can act on it any more — but deleting it again is the same
    // success, because the share RAM is asked to delete is already deleted.
    let (status, body) = delete(&router, &arn, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a repeat delete of a deleted share expected 200, got {status} with \
         {body:?}"
    );
    assert_eq!(
        body.get("returnValue").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: a repeat delete reports returnValue true, got {body:?}"
    );
    let (status, body) = update(
        &router,
        &json!({ "resourceShareArn": arn, "name": "u-gone" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "UnknownResourceException".to_string()),
        "case {case:?}: an update of a deleted share must be refused, got {body:?}"
    );
    let (status, body) = associate(
        &router,
        &json!({ "resourceShareArn": arn, "principals": ["777788889999"] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "UnknownResourceException".to_string()),
        "case {case:?}: an associate on a deleted share must be refused, got {body:?}"
    );
}

#[tokio::test]
async fn an_associate_appends_to_the_stored_share() {
    let router = emulator();
    let case = "associate";
    let created = create_ok(
        &router,
        case,
        &json!({ "name": "share-associate", "principals": ["444455556666"] }),
    )
    .await;
    let arn = arn_of(case, &created);

    let (status, associated) = associate(
        &router,
        &json!({
            "resourceShareArn": arn,
            "principals": ["777788889999"],
            "resourceArns": [LOCAL_RESOURCE],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: associate expected 200, got {status} with {associated:?}"
    );
    // The response describes exactly the entities just associated.
    let reported = associations_of(case, &associated);
    let described: Vec<(String, String)> = reported
        .iter()
        .map(|association| {
            (
                association
                    .get("associationType")
                    .and_then(Value::as_str)
                    .unwrap_or("<none>")
                    .to_string(),
                association
                    .get("associatedEntity")
                    .and_then(Value::as_str)
                    .unwrap_or("<none>")
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        described,
        vec![
            ("RESOURCE".to_string(), LOCAL_RESOURCE.to_string()),
            ("PRINCIPAL".to_string(), "777788889999".to_string()),
        ],
        "case {case:?}: the response must describe the entities just associated"
    );

    // Both principals — the one from the create and the one just associated — are
    // now stored against the share.
    let (_, found) = get_associations(
        &router,
        &json!({ "associationType": "PRINCIPAL", "resourceShareArns": [arn] }),
    )
    .await;
    let mut principals: Vec<String> = associations_of(case, &found)
        .iter()
        .map(|association| {
            association
                .get("associatedEntity")
                .and_then(Value::as_str)
                .unwrap_or("<none>")
                .to_string()
        })
        .collect();
    principals.sort();
    assert_eq!(
        principals,
        vec!["444455556666".to_string(), "777788889999".to_string()],
        "case {case:?}: every associated principal must be reported"
    );

    // Associating an entity that is already associated stores nothing new.
    let (status, again) = associate(
        &router,
        &json!({ "resourceShareArn": arn, "principals": ["777788889999"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a repeat associate expected 200, got {status} with {again:?}"
    );
    let (_, after) = get_associations(
        &router,
        &json!({ "associationType": "PRINCIPAL", "resourceShareArns": [arn] }),
    )
    .await;
    assert_eq!(
        associations_of(case, &after).len(),
        2,
        "case {case:?}: re-associating an entity must not duplicate it, got {after:?}"
    );
    // A filter on one entity reports only that one.
    let (_, one) = get_associations(
        &router,
        &json!({
            "associationType": "PRINCIPAL",
            "resourceShareArns": [arn],
            "principal": "777788889999",
        }),
    )
    .await;
    assert_eq!(
        associations_of(case, &one).len(),
        1,
        "case {case:?}: the principal filter must select one association, got {one:?}"
    );
    let (_, none) = get_associations(
        &router,
        &json!({
            "associationType": "PRINCIPAL",
            "resourceShareArns": [arn],
            "principal": "999988887777",
        }),
    )
    .await;
    assert!(
        associations_of(case, &none).is_empty(),
        "case {case:?}: a principal nothing is associated under reports nothing, got {none:?}"
    );
    let (_, by_resource) = get_associations(
        &router,
        &json!({
            "associationType": "RESOURCE",
            "resourceShareArns": [arn],
            "resourceArn": LOCAL_RESOURCE,
        }),
    )
    .await;
    assert_eq!(
        associations_of(case, &by_resource).len(),
        1,
        "case {case:?}: the resourceArn filter must select one association, got {by_resource:?}"
    );
}

#[tokio::test]
async fn a_client_token_makes_a_call_idempotent() {
    let router = emulator();

    // Create: the same token and the same parameters create one share.
    let body = json!({
        "name": "share-token",
        "principals": ["444455556666"],
        "clientToken": "token-create",
    });
    let first = create_ok(&router, "create retry", &body).await;
    let second = create_ok(&router, "create retry", &body).await;
    assert_eq!(
        first, second,
        "case \"create retry\": a retry under the same clientToken must report the same share"
    );
    let arn = arn_of("create retry", &first);
    let (_, listed) = get_shares(
        &router,
        &json!({ "resourceOwner": "SELF", "name": "share-token" }),
    )
    .await;
    assert_eq!(
        shares_of("create retry", &listed).len(),
        1,
        "case \"create retry\": a retry must not create a second share, got {listed:?}"
    );
    assert_eq!(
        first.get("resourceShareArn").and_then(Value::as_str),
        Some(arn.as_str()),
        "case \"create retry\": the retry must report the stored share"
    );

    // Create: the same token with different parameters is the mismatch case.
    let (status, mismatch) = create(
        &router,
        &json!({ "name": "share-token-other", "clientToken": "token-create" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&mismatch)),
        (400, "IdempotentParameterMismatchException".to_string()),
        "case \"create mismatch\": wrong answer, got {mismatch:?}"
    );

    // Associate: the same token re-reports the stored associations and stores no
    // more.
    let associate_body = json!({
        "resourceShareArn": arn,
        "principals": ["777788889999"],
        "clientToken": "token-associate",
    });
    let (_, once) = associate(&router, &associate_body).await;
    let (_, twice) = associate(&router, &associate_body).await;
    assert_eq!(
        once, twice,
        "case \"associate retry\": a retry must report the same associations"
    );
    let (_, found) = get_associations(
        &router,
        &json!({ "associationType": "PRINCIPAL", "resourceShareArns": [arn] }),
    )
    .await;
    assert_eq!(
        associations_of("associate retry", &found).len(),
        2,
        "case \"associate retry\": a retry must not store a third association, got {found:?}"
    );
    let (status, mismatch) = associate(
        &router,
        &json!({
            "resourceShareArn": arn,
            "principals": ["555566667777"],
            "clientToken": "token-associate",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&mismatch)),
        (400, "IdempotentParameterMismatchException".to_string()),
        "case \"associate mismatch\": wrong answer, got {mismatch:?}"
    );

    // Update: the same token keeps the lastUpdatedTime the first call stored.
    let update_body = json!({
        "resourceShareArn": arn,
        "name": "share-token-renamed",
        "clientToken": "token-update",
    });
    let (_, once) = update(&router, &update_body).await;
    let (_, twice) = update(&router, &update_body).await;
    assert_eq!(
        once, twice,
        "case \"update retry\": a retry must report the same share, including its \
         lastUpdatedTime"
    );
    let (status, mismatch) = update(
        &router,
        &json!({
            "resourceShareArn": arn,
            "name": "share-token-renamed-again",
            "clientToken": "token-update",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&mismatch)),
        (400, "IdempotentParameterMismatchException".to_string()),
        "case \"update mismatch\": wrong answer, got {mismatch:?}"
    );

    // Delete: the same token reports the same success rather than a second,
    // failing delete.
    let (status, once) = delete(&router, &arn, Some("token-delete")).await;
    assert_eq!(
        status, 200,
        "case \"delete retry\": the first delete expected 200, got {once:?}"
    );
    let (status, twice) = delete(&router, &arn, Some("token-delete")).await;
    assert_eq!(
        (status, twice.get("returnValue").and_then(Value::as_bool)),
        (200, Some(true)),
        "case \"delete retry\": a retry under the same clientToken must report the same success, \
         got {twice:?}"
    );
}

/// A row of the refusal table: a request that must be refused, and how.
struct RefusalCase {
    case: String,
    uri: &'static str,
    request: Value,
    expected: &'static str,
}

#[tokio::test]
async fn a_request_the_service_cannot_store_is_refused_as_ram_words_it() {
    let router = emulator();
    let created = create_ok(
        &router,
        "refusals",
        &json!({ "name": "share-refusals", "principals": ["444455556666"] }),
    )
    .await;
    let arn = arn_of("refusals", &created);
    // Well-formed, and no share has it.
    let absent_arn =
        format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/00000000-0000-4000-8000-000000000000");

    let many_tags: Vec<Value> = (0..=MAX_TAGS_PER_SHARE)
        .map(|index| json!({ "key": format!("key-{index}"), "value": "v" }))
        .collect();
    let many_principals: Vec<Value> = (0..=MAX_ENTITIES_PER_CALL)
        .map(|index| json!(format!("{index:012}")))
        .collect();

    let cases = vec![
        RefusalCase {
            case: "create with an empty name".to_string(),
            uri: "/createresourceshare",
            request: json!({ "name": "" }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "create with a resourceArn that is not an ARN".to_string(),
            uri: "/createresourceshare",
            request: json!({ "name": "share-bad-arn", "resourceArns": ["subnet-1111"] }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "create with a reserved tag key".to_string(),
            uri: "/createresourceshare",
            request: json!({
                "name": "share-reserved-tag",
                "tags": [{ "key": "aws:created-by", "value": "x" }],
            }),
            expected: "TagPolicyViolationException",
        },
        RefusalCase {
            case: format!("create with {} tags", MAX_TAGS_PER_SHARE + 1),
            uri: "/createresourceshare",
            request: json!({ "name": "share-too-many-tags", "tags": many_tags }),
            expected: "TagLimitExceededException",
        },
        RefusalCase {
            case: "create with a tag that has no key".to_string(),
            uri: "/createresourceshare",
            request: json!({ "name": "share-keyless-tag", "tags": [{ "value": "x" }] }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: format!("create with {} principals", MAX_ENTITIES_PER_CALL + 1),
            uri: "/createresourceshare",
            request: json!({ "name": "share-too-many-principals", "principals": many_principals }),
            expected: "ResourceShareLimitExceededException",
        },
        RefusalCase {
            case: "create with an empty clientToken".to_string(),
            uri: "/createresourceshare",
            request: json!({ "name": "share-empty-token", "clientToken": "" }),
            expected: "InvalidClientTokenException",
        },
        RefusalCase {
            case: "associate nothing".to_string(),
            uri: "/associateresourceshare",
            request: json!({ "resourceShareArn": arn }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "associate with a share ARN nothing has".to_string(),
            uri: "/associateresourceshare",
            request: json!({ "resourceShareArn": absent_arn, "principals": ["444455556666"] }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "associate with an ARN of another service".to_string(),
            uri: "/associateresourceshare",
            request: json!({
                "resourceShareArn": LOCAL_RESOURCE,
                "principals": ["444455556666"],
            }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "associate with a resourceArn that is not an ARN".to_string(),
            uri: "/associateresourceshare",
            request: json!({ "resourceShareArn": arn, "resourceArns": ["subnet-1111"] }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "update a share ARN nothing has".to_string(),
            uri: "/updateresourceshare",
            request: json!({ "resourceShareArn": absent_arn, "name": "u-absent" }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "update with an ARN that is not a share ARN".to_string(),
            uri: "/updateresourceshare",
            request: json!({ "resourceShareArn": "ram/share", "name": "u-bad" }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "update to an empty name".to_string(),
            uri: "/updateresourceshare",
            request: json!({ "resourceShareArn": arn, "name": "" }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "read shares by a share ARN nothing has".to_string(),
            uri: "/getresourceshares",
            request: json!({ "resourceOwner": "SELF", "resourceShareArns": [absent_arn] }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "read shares by an ARN that is not a share ARN".to_string(),
            uri: "/getresourceshares",
            request: json!({ "resourceOwner": "SELF", "resourceShareArns": [LOCAL_RESOURCE] }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "read associations of a share ARN nothing has".to_string(),
            uri: "/getresourceshareassociations",
            request: json!({ "associationType": "PRINCIPAL", "resourceShareArns": [absent_arn] }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "read associations with principal on a RESOURCE query".to_string(),
            uri: "/getresourceshareassociations",
            request: json!({ "associationType": "RESOURCE", "principal": "444455556666" }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "read associations with resourceArn on a PRINCIPAL query".to_string(),
            uri: "/getresourceshareassociations",
            request: json!({ "associationType": "PRINCIPAL", "resourceArn": LOCAL_RESOURCE }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "read associations with a resourceArn that is not an ARN".to_string(),
            uri: "/getresourceshareassociations",
            request: json!({ "associationType": "RESOURCE", "resourceArn": "subnet-1111" }),
            expected: "MalformedArnException",
        },
    ];

    for RefusalCase {
        case,
        uri,
        request,
        expected,
    } in cases
    {
        let (status, body) = call(&router, "POST", uri, Some(&request)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: wrong error for {request}, got {body:?}"
        );
    }

    // The same two refusals on the one operation that takes its input in the query
    // string.
    let (status, body) = delete(&router, "ram/share", None).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "MalformedArnException".to_string()),
        "case \"delete with an ARN that is not a share ARN\": got {body:?}"
    );
    let (status, body) = delete(&router, &absent_arn, None).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "UnknownResourceException".to_string()),
        "case \"delete a share ARN nothing has\": got {body:?}"
    );

    // Nothing above stored anything: the one share created at the top is still the
    // only one.
    let (_, listed) = get_shares(&router, &json!({ "resourceOwner": "SELF" })).await;
    assert_eq!(
        names_of("refusals", &listed),
        vec!["share-refusals".to_string()],
        "case \"refusals\": a refused request must store nothing, got {listed:?}"
    );
}
