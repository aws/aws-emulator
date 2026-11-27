// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for DataZone subscription requests: `CreateSubscriptionRequest`
//! and the reader it round-trips through, `GetSubscriptionRequestDetails`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A subscription request is RECORDED in its domain: the create answers the stored
//!    record — the status the ask resolves to (here `ACCEPTED`, because the calling identity
//!    owns the project that published the listing; the asks that await a decision, and the
//!    decisions taken on them, are [`crate::subscription_request_decisions`], and the
//!    create-time rule itself is [`crate::subscription_request_owner_asks`]), the calling
//!    principal as `createdBy`, the reason asked for, the
//!    subscribed principal resolved against the domain, and the subscribed listing at the
//!    revision the catalog held — and `GetSubscriptionRequestDetails` reads back exactly
//!    that record, as often as it is asked.
//! 2. The preconditions are refused with the operation's own modelled errors: an
//!    identifier the store serves no domain for with `AccessDeniedException`, an unsettled
//!    domain with `ConflictException`, a listing or a principal the domain does not hold
//!    with `ResourceNotFoundException`, and a request whose shape no modelled constraint
//!    covers with `ValidationException`.
//! 3. A repeated `clientToken` records ONE request, and a different one records another.
//! 4. An `assetScopes` entry is reported back with the verdict of the filters it names,
//!    and an `assetPermissions` entry with the permissions asked for — both on the asset
//!    they name.

use crate::harness::Emulator;
use serde_json::{json, Value};
// The enum members as the service stores them, so a drift between these constants and
// the values on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    FILTER_STATUS_INVALID, FILTER_STATUS_VALID, SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
};

/// The DataZone user every call here is attributed to: the user this emulator
/// authenticates every caller as, read from the service's own constant so a drift between
/// it and the value on the wire fails here.
const CALLER: &str = service_com_amazonaws_datazone_datazone::state::CALLER_USER_PROFILE_ID;

/// How long a case waits for a step the clock owes (a domain finishing `CREATING`).
///
/// Deliberately generous: the case is decided by the CONDITION it polls for, never by
/// how fast the host runs, so a loaded machine takes longer to pass rather than failing.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// How long the poll loop pauses between looks.
const POLL_PAUSE: std::time::Duration = std::time::Duration::from_millis(20);

/// `SubscriptionRequestId`'s modelled pattern, `^[a-zA-Z0-9_-]{1,36}$` (smithy_query
/// shape="SubscriptionRequestId").
fn is_subscription_request_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `ListingName`'s modelled maximum length, 1..64 (smithy_query shape="ListingName") —
/// shorter than `AssetName`'s 1..256, so a longer entity name is reported clipped to it.
const LISTING_NAME_MAX_LEN: usize = 64;

/// `RequestReason`'s modelled length, 1..4096 (smithy_query shape="RequestReason").
const REQUEST_REASON_LEN: std::ops::RangeInclusive<usize> = 1..=4096;

/// Create a domain and poll `GetDomain` until it reports `AVAILABLE`, which is the
/// precondition every case below needs.
async fn settled_domain(dz: &Emulator, name: &str) -> String {
    let created = dz.create_domain(json!({ "name": name })).await;
    assert_eq!(
        created.status, 201,
        "creating domain {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    let id = created.s("id");
    let uri = format!("/v2/domains/{id}");
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let got = dz.call("GET", &uri, None).await;
        if got.s("status") == "AVAILABLE" {
            return id;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "domain {name:?} never settled: last status {:?}, body {:?}",
            got.s("status"),
            got.body
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// Create a project, answering its id.
async fn project(dz: &Emulator, domain: &str, name: &str) -> String {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(json!({ "name": name })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating project {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// Create an asset owned by `owner`, answering the whole response so a case can read the
/// revision and the type back off it.
async fn asset(dz: &Emulator, domain: &str, owner: &str, mut body: Value) -> Value {
    body["owningProjectIdentifier"] = json!(owner);
    let created = dz
        .call("POST", &format!("/v2/domains/{domain}/assets"), Some(body))
        .await;
    assert_eq!(
        created.status, 201,
        "creating the asset: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.body
}

/// Publish an asset into the domain's catalog, answering the `ListingId` the change set
/// recorded it under.
async fn publish(dz: &Emulator, domain: &str, asset_id: &str) -> String {
    let answer = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/listings/change-set"),
            Some(json!({
                "entityIdentifier": asset_id,
                "entityType": "ASSET",
                "action": "PUBLISH",
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "publishing asset {asset_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.s("listingId")
}

/// Create a user profile, answering the `UserProfileId` a subscribed user principal is
/// named by.
async fn user_profile(dz: &Emulator, domain: &str, identifier: &str) -> String {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/user-profiles"),
            Some(json!({ "userIdentifier": identifier, "userType": "IAM_USER" })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating user profile {identifier:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// The member of a response body at `path`, or `Value::Null` so an assertion message
/// says which member was missing.
fn at(body: &Value, path: &[&str]) -> Value {
    let mut cursor = body;
    for step in path {
        match cursor.get(step) {
            Some(next) => cursor = next,
            None => return Value::Null,
        }
    }
    cursor.clone()
}

/// Which arm of the modelled `SubscribedPrincipalInput` union a case names its principal
/// with (smithy_query shape="SubscribedPrincipalInput": project | user | group | iam).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Principal {
    Project,
    User,
    IamRole,
}

/// One `CreateSubscriptionRequest` and the record the rules say it must produce.
struct RequestCase {
    label: &'static str,
    /// The `CreateAsset` body, minus `owningProjectIdentifier` (filled by the test).
    asset: Value,
    /// Which principal arm the request names.
    principal: Principal,
    /// The `metadataForms` the request carries, absent when it carries none.
    metadata_forms: Option<Value>,
    request_reason: String,
    /// The `name` the subscribed listing must be reported with.
    expect_listing_name: String,
    /// The `description` it must be reported with — a REQUIRED member, so an entity with
    /// no description is reported with the empty one.
    expect_listing_description: String,
    /// The listing item's `forms`, parsed back from the modelled `Forms` string; absent
    /// when the published entity carries no form.
    expect_item_forms: Option<Value>,
    /// The `metadataForms` the response must report back.
    expect_metadata_forms: Option<Value>,
}

/// The rows differ along what the rule generalises over: which principal arm asks for
/// the access, how many metadata forms the request carries and whether each names a
/// type or content, whether the published entity has a description or a form, how long
/// its name is against the listing's own bound, and how long the reason is against each
/// published bound of `RequestReason`.
fn request_cases() -> Vec<RequestCase> {
    let long_asset_name = "orders-fact-table-".repeat(5);
    vec![
        RequestCase {
            label: "a project principal, an asset with no form and no description, the \
                    shortest reason the model admits",
            asset: json!({
                "name": "minimal-asset",
                "typeIdentifier": "amazon.datazone.RelationalTable",
            }),
            principal: Principal::Project,
            metadata_forms: None,
            request_reason: "r".repeat(*REQUEST_REASON_LEN.start()),
            expect_listing_name: "minimal-asset".to_string(),
            expect_listing_description: String::new(),
            expect_item_forms: None,
            expect_metadata_forms: None,
        },
        RequestCase {
            label: "a user-profile principal, a described asset carrying one JSON form, \
                    three metadata forms",
            asset: json!({
                "name": "orders table",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "description": "the orders fact table",
                "formsInput": [{
                    "formName": "RelationalTableForm",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "content": "{\"columns\":[{\"columnName\":\"order_id\"}]}",
                }],
            }),
            principal: Principal::User,
            metadata_forms: Some(json!([
                {
                    "formName": "Justification",
                    "typeIdentifier": "amazon.datazone.JustificationFormType",
                    "typeRevision": "1",
                    "content": "{\"ticket\":\"DZ-42\"}",
                },
                { "formName": "ReviewNote", "content": "needed for the quarterly report" },
                { "formName": "NoContent" },
            ])),
            request_reason: "quarterly reporting needs the orders table".to_string(),
            expect_listing_name: "orders table".to_string(),
            expect_listing_description: "the orders fact table".to_string(),
            expect_item_forms: Some(json!({
                "RelationalTableForm": { "columns": [{ "columnName": "order_id" }] },
            })),
            expect_metadata_forms: Some(json!([
                {
                    "formName": "Justification",
                    "typeName": "amazon.datazone.JustificationFormType",
                    "typeRevision": "1",
                    "content": "{\"ticket\":\"DZ-42\"}",
                },
                // A form naming no type has no type revision either, by the rule
                // `crate::state::stored_forms` states.
                {
                    "formName": "ReviewNote",
                    "content": "needed for the quarterly report",
                },
                { "formName": "NoContent" },
            ])),
        },
        RequestCase {
            label: "an IAM role principal, an asset whose name is longer than a \
                    listing's bound, the longest reason the model admits",
            asset: json!({
                "name": long_asset_name.clone(),
                "typeIdentifier": "CustomTable",
                "description": "d".repeat(2048),
                "formsInput": [
                    { "formName": "OwnershipForm", "content": "{\"owner\":\"data-eng\"}" },
                    { "formName": "Plain", "content": "not json" },
                ],
            }),
            principal: Principal::IamRole,
            metadata_forms: None,
            request_reason: "r".repeat(*REQUEST_REASON_LEN.end()),
            expect_listing_name: long_asset_name
                .chars()
                .take(LISTING_NAME_MAX_LEN)
                .collect(),
            expect_listing_description: "d".repeat(2048),
            expect_item_forms: Some(json!({
                "OwnershipForm": { "owner": "data-eng" },
                "Plain": "not json",
            })),
            expect_metadata_forms: None,
        },
    ]
}

#[tokio::test]
async fn a_subscription_request_is_recorded_and_read_back() {
    let dz = Emulator::new();
    let domain = settled_domain(&dz, "subscription-request-domain").await;
    let project_name = "subscription owner";
    let owner = project(&dz, &domain, project_name).await;
    let profile = user_profile(&dz, &domain, "subscriber.analyst").await;
    let role_arn = format!(
        "arn:aws:iam::{}:role/DataZoneSubscriber",
        engine::emulator_account_id()
    );
    let requests_uri = format!("/v2/domains/{domain}/subscription-requests");
    let mut ids: Vec<String> = Vec::new();

    for case in request_cases() {
        let published = asset(&dz, &domain, &owner, case.asset.clone()).await;
        let asset_id = published
            .get("id")
            .and_then(Value::as_str)
            .expect("the created asset reports an id")
            .to_string();
        let asset_type = published
            .get("typeIdentifier")
            .and_then(Value::as_str)
            .expect("the created asset reports its type")
            .to_string();
        let asset_revision = published
            .get("revision")
            .and_then(Value::as_str)
            .expect("the created asset reports its revision")
            .to_string();
        let listing = publish(&dz, &domain, &asset_id).await;

        let principal = match case.principal {
            Principal::Project => json!({ "project": { "identifier": owner } }),
            Principal::User => json!({ "user": { "identifier": profile } }),
            Principal::IamRole => json!({ "iam": { "identifier": role_arn } }),
        };
        let mut body = json!({
            "subscribedPrincipals": [principal],
            "subscribedListings": [{ "identifier": listing }],
            "requestReason": case.request_reason,
        });
        if let Some(forms) = &case.metadata_forms {
            body["metadataForms"] = forms.clone();
        }
        let created = dz.call("POST", &requests_uri, Some(body)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.label, created.status, created.body
        );

        let id = created.s("id");
        assert!(
            is_subscription_request_id(&id),
            "case {:?}: the id must satisfy SubscriptionRequestId's pattern, got {id:?}",
            case.label
        );
        assert!(
            !ids.contains(&id),
            "case {:?}: every request gets its own id, got {id:?} again after {ids:?}",
            case.label
        );
        ids.push(id.clone());

        assert_eq!(
            created.s("status"),
            SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
            "case {:?}: the asker owns the project that published the listing, so its own \
             ask is resolved when it is made, got {:?}",
            case.label,
            created.body
        );
        assert_eq!(
            created.s("domainId"),
            domain,
            "case {:?}: the request belongs to the domain it was made in, got {:?}",
            case.label,
            created.body
        );
        assert_eq!(
            created.s("createdBy"),
            CALLER,
            "case {:?}: createdBy must name the calling principal, got {:?}",
            case.label,
            created.s("createdBy")
        );
        assert_eq!(
            created.s("requestReason"),
            case.request_reason,
            "case {:?}: the reason asked for must be reported back",
            case.label
        );
        // Nothing has updated or decided the request yet, and no subscription exists for
        // the access it asks for.
        for absent in [
            "updatedBy",
            "reviewerId",
            "decisionComment",
            "existingSubscriptionId",
        ] {
            assert_eq!(
                created.body.get(absent),
                None,
                "case {:?}: {absent} must be absent on a fresh request, got {:?}",
                case.label,
                created.body.get(absent)
            );
        }
        let created_at = created.body.get("createdAt").cloned();
        assert!(
            created_at.is_some(),
            "case {:?}: createdAt is required, got {:?}",
            case.label,
            created.body
        );
        assert_eq!(
            created.body.get("updatedAt").cloned(),
            created_at,
            "case {:?}: nothing has updated the request, so updatedAt is when it was \
             made, got {:?}",
            case.label,
            created.body
        );

        // The principal, resolved against the domain rather than echoed.
        let principals = created
            .body
            .get("subscribedPrincipals")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            principals.len(),
            1,
            "case {:?}: exactly the principal asked for, got {:?}",
            case.label,
            created.body.get("subscribedPrincipals")
        );
        let reported = &principals[0];
        match case.principal {
            Principal::Project => {
                assert_eq!(
                    at(reported, &["project", "id"]),
                    json!(owner),
                    "case {:?}: the project principal's id, got {reported:?}",
                    case.label
                );
                assert_eq!(
                    at(reported, &["project", "name"]),
                    json!(project_name),
                    "case {:?}: the project's name comes from its stored record, got \
                     {reported:?}",
                    case.label
                );
            }
            Principal::User => {
                assert_eq!(
                    at(reported, &["user", "id"]),
                    json!(profile),
                    "case {:?}: the user principal's profile id, got {reported:?}",
                    case.label
                );
                assert_ne!(
                    at(reported, &["user", "details"]),
                    Value::Null,
                    "case {:?}: the user's details come from its stored profile, got \
                     {reported:?}",
                    case.label
                );
            }
            Principal::IamRole => {
                assert_eq!(
                    at(reported, &["iam", "principalArn"]),
                    json!(role_arn),
                    "case {:?}: the IAM principal's ARN, got {reported:?}",
                    case.label
                );
            }
        }

        // The subscribed listing: the catalog record as it stands, at the revision the
        // catalog held when the request was made.
        let listings = created
            .body
            .get("subscribedListings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listings.len(),
            1,
            "case {:?}: exactly the listing asked for, got {:?}",
            case.label,
            created.body.get("subscribedListings")
        );
        let reported = &listings[0];
        for (member, expected) in [
            ("id", json!(listing)),
            ("name", json!(case.expect_listing_name)),
            ("description", json!(case.expect_listing_description)),
            ("revision", json!("1")),
            ("ownerProjectId", json!(owner)),
            ("ownerProjectName", json!(project_name)),
        ] {
            assert_eq!(
                reported.get(member).cloned().unwrap_or(Value::Null),
                expected,
                "case {:?}: subscribedListings[0].{member}, got {reported:?}",
                case.label
            );
        }
        assert_eq!(
            at(reported, &["item", "assetListing", "entityId"]),
            json!(asset_id),
            "case {:?}: the listing item records the published asset, got {reported:?}",
            case.label
        );
        assert_eq!(
            at(reported, &["item", "assetListing", "entityRevision"]),
            json!(asset_revision),
            "case {:?}: at the revision that was published, got {reported:?}",
            case.label
        );
        assert_eq!(
            at(reported, &["item", "assetListing", "entityType"]),
            json!(asset_type),
            "case {:?}: and with the entity's own type, got {reported:?}",
            case.label
        );
        let item_forms = at(reported, &["item", "assetListing", "forms"])
            .as_str()
            .map(|forms| {
                serde_json::from_str::<Value>(forms).expect("the forms are a JSON document")
            });
        assert_eq!(
            item_forms, case.expect_item_forms,
            "case {:?}: the published entity's forms, got {reported:?}",
            case.label
        );
        // Nothing narrowed the access asked for, so neither member is reported.
        for absent in ["assetScope", "permissions"] {
            assert_eq!(
                at(reported, &["item", "assetListing", absent]),
                Value::Null,
                "case {:?}: {absent} must be absent when the request named none, got \
                 {reported:?}",
                case.label
            );
        }
        assert_eq!(
            created.body.get("metadataForms").cloned(),
            case.expect_metadata_forms,
            "case {:?}: the metadata forms the request carried, got {:?}",
            case.label,
            created.body.get("metadataForms")
        );

        // Rule 1, the round trip: the reader reports the stored record, and a read
        // changes nothing a later caller sees.
        let request_uri = format!("{requests_uri}/{id}");
        let read = dz.call("GET", &request_uri, None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: reading the request back: expected 200, got {} {:?}",
            case.label, read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {:?}: the reader must report the record the create answered",
            case.label
        );
        let again = dz.call("GET", &request_uri, None).await;
        assert_eq!(
            again.body, read.body,
            "case {:?}: a read must change nothing a later read sees",
            case.label
        );
    }
}

#[tokio::test]
async fn a_subscription_request_needs_a_settled_domain_a_listing_and_a_known_principal() {
    let dz = Emulator::new();
    let domain = settled_domain(&dz, "subscription-precondition-domain").await;
    let owner = project(&dz, &domain, "precondition owner").await;
    let published = asset(
        &dz,
        &domain,
        &owner,
        json!({ "name": "precondition-asset", "typeIdentifier": "CustomTable" }),
    )
    .await;
    let asset_id = published.s_id();
    let listing = publish(&dz, &domain, &asset_id).await;
    // Another asset, published nowhere: an `assetScopes` entry naming it asks for
    // something about an asset the subscribed listing does not publish.
    let other = asset(
        &dz,
        &domain,
        &owner,
        json!({ "name": "unsubscribed-asset", "typeIdentifier": "CustomTable" }),
    )
    .await;
    let other_asset = other.s_id();
    // A domain that has only just been created is still CREATING, so it cannot take a
    // request yet. Created last so the window has not elapsed.
    let creating = dz
        .create_domain(json!({ "name": "subscription-unsettled-domain" }))
        .await;
    assert_eq!(
        creating.status, 201,
        "creating the unsettled domain: got {:?}",
        creating.body
    );
    let creating_domain = creating.s("id");

    let ask = |principal: Value| {
        json!({
            "subscribedPrincipals": [principal],
            "subscribedListings": [{ "identifier": listing }],
            "requestReason": "because",
        })
    };
    // (label, domain, body, expected status, expected error code)
    let rows: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            // A permission is held ON a domain, so the caller holds none on an identifier
            // the store serves no domain for: authorization decides the call before any
            // lookup.
            "a domain nothing created",
            "dzd_doesnotexist".to_string(),
            ask(json!({ "project": { "identifier": owner } })),
            403,
            "AccessDeniedException",
        ),
        (
            "a domain that is still CREATING",
            creating_domain.clone(),
            json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "because",
            }),
            409,
            "ConflictException",
        ),
        (
            "a listing id nothing published",
            domain.clone(),
            json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": "nothingpublished" }],
                "requestReason": "because",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a project the domain does not hold",
            domain.clone(),
            ask(json!({ "project": { "identifier": "nosuchproject" } })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a user profile the domain does not hold",
            domain.clone(),
            ask(json!({
                "user": { "identifier": "4a2e8f31-7c0d-4b19-9f52-6e1d3a8b4c77" },
            })),
            404,
            "ResourceNotFoundException",
        ),
        (
            // Group profiles are created by `CreateGroupProfile`, still a scaffold, so no
            // domain holds one for a group a request names.
            "a group principal, which needs a group profile",
            domain.clone(),
            ask(json!({
                "group": { "identifier": "7c0d4b199f-4a2e8f31-7c0d-4b19-9f52-6e1d3a8b4c77" },
            })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a principal arm naming no identifier",
            domain.clone(),
            ask(json!({ "project": {} })),
            400,
            "ValidationException",
        ),
        (
            "a reason shorter than the modelled minimum",
            domain.clone(),
            json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "",
            }),
            400,
            "ValidationException",
        ),
        (
            "a reason one character past the modelled maximum",
            domain.clone(),
            json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "r".repeat(*REQUEST_REASON_LEN.end() + 1),
            }),
            400,
            "ValidationException",
        ),
        (
            "two metadata forms sharing one formName",
            domain.clone(),
            json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "because",
                "metadataForms": [
                    { "formName": "Twice", "content": "first" },
                    { "formName": "Twice", "content": "second" },
                ],
            }),
            400,
            "ValidationException",
        ),
        (
            "an assetScopes entry naming an asset the listing does not publish",
            domain.clone(),
            json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "because",
                "assetScopes": [{ "assetId": other_asset, "filterIds": [] }],
            }),
            400,
            "ValidationException",
        ),
        (
            "an assetPermissions entry naming an asset the listing does not publish",
            domain.clone(),
            json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "because",
                "assetPermissions": [{
                    "assetId": other_asset,
                    "permissions": { "s3": ["READ"] },
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            "an assetScopes entry naming a filter the asset does not have",
            domain.clone(),
            json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "because",
                "assetScopes": [{ "assetId": asset_id, "filterIds": ["nosuchfilter"] }],
            }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (label, scope, body, status, code) in rows {
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{scope}/subscription-requests"),
                Some(body),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }

    // Reading one: a request id nothing created is a miss, and an identifier the store
    // serves no domain for is denied before any lookup.
    let missing = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/subscription-requests/nosuchrequest"),
            None,
        )
        .await;
    assert_eq!(
        (missing.status, missing.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "reading a request nothing created: got {} {:?}",
        missing.status,
        missing.body
    );
    let denied = dz
        .call(
            "GET",
            "/v2/domains/dzd_doesnotexist/subscription-requests/anything",
            None,
        )
        .await;
    assert_eq!(
        (denied.status, denied.error_code.as_str()),
        (403, "AccessDeniedException"),
        "reading in a domain nothing created: got {} {:?}",
        denied.status,
        denied.body
    );

    // Every refusal above left nothing behind: the domain still takes a request.
    let accepted = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/subscription-requests"),
            Some(json!({
                "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "after the refusals",
            })),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "a well-formed request after the refusals: got {} {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn a_repeated_client_token_records_one_subscription_request() {
    let dz = Emulator::new();
    let domain = settled_domain(&dz, "subscription-idempotency-domain").await;
    let owner = project(&dz, &domain, "idempotency owner").await;
    let requests_uri = format!("/v2/domains/{domain}/subscription-requests");
    let published = asset(
        &dz,
        &domain,
        &owner,
        json!({ "name": "token-asset", "typeIdentifier": "CustomTable" }),
    )
    .await;
    let listing = publish(&dz, &domain, &published.s_id()).await;

    // The rows differ along the token itself — the only thing the rule turns on.
    let mut seen: Vec<String> = Vec::new();
    for token in ["t", "a-second-token", &"t".repeat(64)] {
        let ask = json!({
            "subscribedPrincipals": [{ "project": { "identifier": owner } }],
            "subscribedListings": [{ "identifier": listing }],
            "requestReason": "idempotent ask",
            "clientToken": token,
        });
        let first = dz.call("POST", &requests_uri, Some(ask.clone())).await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: expected 200, got {} {:?}",
            first.status, first.body
        );
        let replay = dz.call("POST", &requests_uri, Some(ask)).await;
        assert_eq!(
            replay.status, 200,
            "case {token:?}: the replay: expected 200, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body, first.body,
            "case {token:?}: a replay must answer the request the first call recorded"
        );
        let id = first.s("id");
        assert!(
            !seen.contains(&id),
            "case {token:?}: a DIFFERENT token records another request, got {id:?} again \
             after {seen:?}"
        );
        seen.push(id.clone());

        let read = dz.call("GET", &format!("{requests_uri}/{id}"), None).await;
        assert_eq!(
            read.body, first.body,
            "case {token:?}: the one recorded request reads back"
        );
    }
}

/// The columns the filtered asset's schema declares, so a filter naming one of them is a
/// valid view of it and a filter naming anything else is not.
const SCHEMA_COLUMNS: [&str; 2] = ["order_id", "region"];

#[tokio::test]
async fn an_asset_scope_is_reported_with_the_verdict_of_the_filters_it_names() {
    let dz = Emulator::new();
    let domain = settled_domain(&dz, "subscription-scope-domain").await;
    let owner = project(&dz, &domain, "scope owner").await;
    let requests_uri = format!("/v2/domains/{domain}/subscription-requests");

    let columns: Vec<Value> = SCHEMA_COLUMNS
        .iter()
        .map(|name| json!({ "columnName": name, "dataType": "string" }))
        .collect();
    let schema =
        serde_json::to_string(&json!({ "columns": columns })).expect("the schema serializes");
    let published = asset(
        &dz,
        &domain,
        &owner,
        json!({
            "name": "scoped-orders",
            "typeIdentifier": "amazon.datazone.RelationalTable",
            "formsInput": [{
                "formName": "RelationalTableForm",
                "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                "content": schema,
            }],
        }),
    )
    .await;
    let asset_id = published.s_id();
    let listing = publish(&dz, &domain, &asset_id).await;

    // Two filters on the one asset: one over a column its schema declares, one over a
    // column it does not have.
    let filters_uri = format!("/v2/domains/{domain}/assets/{asset_id}/filters");
    let mut filter_ids: Vec<String> = Vec::new();
    for (name, column) in [("valid view", SCHEMA_COLUMNS[0]), ("invalid view", "ssn")] {
        let created = dz
            .call(
                "POST",
                &filters_uri,
                Some(json!({
                    "name": name,
                    "configuration": {
                        "columnConfiguration": { "includedColumnNames": [column] },
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "creating filter {name:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        filter_ids.push(created.s("id"));
    }
    let (valid_filter, invalid_filter) = (filter_ids[0].clone(), filter_ids[1].clone());

    // One row per scope the rule generalises over: how many filters it names, and
    // whether each is a valid view of the asset.
    let rows: Vec<(&str, Vec<String>, &str, bool)> = vec![
        ("no filter at all", vec![], FILTER_STATUS_VALID, false),
        (
            "one filter the asset's schema satisfies",
            vec![valid_filter.clone()],
            FILTER_STATUS_VALID,
            false,
        ),
        (
            "one filter naming a column the asset does not have",
            vec![invalid_filter.clone()],
            FILTER_STATUS_INVALID,
            true,
        ),
        (
            "a valid and an invalid filter together",
            vec![valid_filter.clone(), invalid_filter.clone()],
            FILTER_STATUS_INVALID,
            true,
        ),
    ];

    for (label, filters, expect_status, expect_error) in rows {
        let created = dz
            .call(
                "POST",
                &requests_uri,
                Some(json!({
                    "subscribedPrincipals": [{ "project": { "identifier": owner } }],
                    "subscribedListings": [{ "identifier": listing }],
                    "requestReason": "scoped ask",
                    "assetScopes": [{ "assetId": asset_id, "filterIds": filters }],
                    "assetPermissions": [{
                        "assetId": asset_id,
                        "permissions": { "s3": ["READ", "WRITE"] },
                    }],
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            created.status, created.body
        );
        let item = at(&created.body, &["subscribedListings"]);
        let item = item
            .as_array()
            .and_then(|listings| listings.first())
            .cloned()
            .unwrap_or(Value::Null);
        let scope = at(&item, &["item", "assetListing", "assetScope"]);
        assert_eq!(
            scope.get("assetId").cloned().unwrap_or(Value::Null),
            json!(asset_id),
            "case {label:?}: the scope names the asset it was asked for, got {scope:?}"
        );
        assert_eq!(
            scope.get("filterIds").cloned().unwrap_or(Value::Null),
            json!(filters),
            "case {label:?}: and the filters it was asked for, got {scope:?}"
        );
        assert_eq!(
            scope.get("status").and_then(Value::as_str),
            Some(expect_status),
            "case {label:?}: the scope's verdict, got {scope:?}"
        );
        assert_eq!(
            scope.get("errorMessage").is_some(),
            expect_error,
            "case {label:?}: an invalid scope says why, a valid one says nothing, got \
             {scope:?}"
        );
        assert_eq!(
            at(&item, &["item", "assetListing", "permissions", "s3"]),
            json!(["READ", "WRITE"]),
            "case {label:?}: the permissions asked for are reported on the asset, got \
             {item:?}"
        );

        // And the reader reports the same scope, derived from the same stored ask.
        let read = dz
            .call("GET", &format!("{requests_uri}/{}", created.s("id")), None)
            .await;
        assert_eq!(
            read.body, created.body,
            "case {label:?}: the reader must report the record the create answered"
        );
    }
}

/// The `id` of a created resource, for the fixture helpers above.
trait CreatedId {
    fn s_id(&self) -> String;
}

impl CreatedId for Value {
    fn s_id(&self) -> String {
        self.get("id")
            .and_then(Value::as_str)
            .expect("a created resource reports an id")
            .to_string()
    }
}
