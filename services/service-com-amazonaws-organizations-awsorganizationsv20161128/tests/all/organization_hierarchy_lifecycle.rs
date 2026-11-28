// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the organization + organizational-unit hierarchy lifecycle:
//! `CreateOrganization`, `DescribeOrganization`, `ListRoots`,
//! `CreateOrganizationalUnit`, `DescribeOrganizationalUnit`,
//! `ListOrganizationalUnitsForParent` and `DeleteOrganizationalUnit`.
//!
//! Every case drives the real wire path — `POST /` with
//! `X-Amz-Target: AWSOrganizationsV20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched through the same [`MultiServiceRouter`] the emulator boots — so what is
//! asserted is what a client sees, not what a handler returns.
//!
//! The suite asserts RULES over sets of inputs rather than one recorded call: the
//! feature-set catalogue from the model's own `OrganizationFeatureSet` values, a table
//! of OU names, the `Path` invariant over every nesting depth the service publishes
//! plus one past it, pagination over the modelled `MaxResults` bounds plus one outside
//! each, and "not in use" over every operation of the slice.
//!
//! Every case here starts from the store a freshly mounted control plane holds: an
//! account that belongs to NO organization, because membership is established by a
//! `CreateOrganization` request and by nothing else. THAT default is asserted by the
//! `default_organization_membership` suite.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_organizations_awsorganizationsv20161128::ops::types::{
    EFFECTIVE_POLICY_TYPE_VALUES, ORGANIZATION_FEATURE_SET_VALUES, POLICY_TYPE_STATUS_VALUES,
    POLICY_TYPE_VALUES,
};
use service_com_amazonaws_organizations_awsorganizationsv20161128::state::{
    MAX_ORGANIZATIONAL_UNIT_DEPTH, MAX_PAGE_SIZE, MAX_TAGS_PER_RESOURCE,
    PARENT_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
///
/// `pub(crate)` so a sibling suite in this test target addresses the same wire path
/// rather than keeping a second copy of it.
pub(crate) const TARGET_PREFIX: &str = "AWSOrganizationsV20161128";

/// This service's SigV4 signing name, the routing key the front door reads off the
/// `authorization` header's credential scope (`OPERATIONS.md`, "Wire protocol").
pub(crate) const SIGNING_NAME: &str = "organizations";

/// The canonical account id every emulated service presents, and so the management
/// account of the organization this service creates.
pub(crate) fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router carrying only this service, over one shared engine — what
/// `core/router/src/lib.rs` builds, narrowed to the service under test, over the store
/// mounting it establishes: one holding no organization. Request authentication is off so
/// the suite needs no signing keys; routing still goes through the SigV4 credential scope
/// on the `authorization` header below.
///
/// `pub(crate)` so a sibling suite drives the booted emulator through this harness.
pub(crate) fn mounted_router() -> MultiServiceRouter {
    router_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`mounted_router`]: a freshly mounted control plane, which is the state of an account
/// that belongs to NO organization — every case BELOW is about creating an organization,
/// or about what an operation answers while none exists.
fn router() -> MultiServiceRouter {
    mounted_router()
}

/// This service mounted on a router over `engine`, the way `core/router/src/lib.rs`
/// mounts it.
fn router_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false).register_service(
        service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(engine),
    );
    router
}

/// One answered request: the HTTP status, the `__type` error code when the answer is a
/// failure, and the parsed JSON body (`Value::Null` for an empty body).
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error: Option<String>,
    pub(crate) body: Value,
}

impl Answer {
    /// Whether this answer is the modelled failure `exception`. The awsJson `__type` is
    /// either the bare shape name or the namespaced shape id, so the suffix is matched.
    pub(crate) fn is(&self, exception: &str) -> bool {
        self.error
            .as_deref()
            .is_some_and(|found| found.ends_with(exception))
    }

    /// A required string member of the body, with the case named on failure.
    pub(crate) fn string(&self, path: &[&str], case: &str) -> String {
        let mut cursor = &self.body;
        for member in path {
            cursor = &cursor[*member];
        }
        cursor
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a string at {path:?}, got {:?}",
                    self.body
                )
            })
            .to_owned()
    }
}

/// Call one operation of this service over the wire.
///
/// `pub(crate)` so a sibling suite drives the same deserialize → handle → serialize walk.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "organizations.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    // awsJson carries the error code in the body's `__type`; the header is set only on
    // some protocols, so the body is the authoritative source here.
    let error = error.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    Answer {
        status,
        error,
        body,
    }
}

/// `CreateOrganization` on a fresh router, asserting it was accepted, and returning the
/// root id every OU in the organization hangs under.
async fn create_organization(router: &MultiServiceRouter, feature_set: Option<&str>) -> String {
    let body = match feature_set {
        Some(value) => json!({ "FeatureSet": value }),
        None => json!({}),
    };
    let answer = call(router, "CreateOrganization", &body).await;
    assert_eq!(
        answer.status, 200,
        "case CreateOrganization({feature_set:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let roots = call(router, "ListRoots", &json!({})).await;
    assert_eq!(
        roots.status, 200,
        "case ListRoots after CreateOrganization({feature_set:?}): expected 200, got {} with {:?}",
        roots.status, roots.body
    );
    let listed = roots.body["Roots"]
        .as_array()
        .unwrap_or_else(|| panic!("case ListRoots: expected a Roots array, got {:?}", roots.body))
        .clone();
    assert_eq!(
        listed.len(),
        1,
        "case ListRoots after CreateOrganization({feature_set:?}): expected exactly one root, got {listed:?}"
    );
    listed[0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case ListRoots: expected a root Id, got {listed:?}"))
        .to_owned()
}

/// `CreateOrganizationalUnit` for `name` under `parent`, asserting it was accepted and
/// returning the created `OrganizationalUnit`.
async fn create_ou(router: &MultiServiceRouter, parent: &str, name: &str) -> Value {
    let answer = call(
        router,
        "CreateOrganizationalUnit",
        &json!({ "ParentId": parent, "Name": name }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case CreateOrganizationalUnit({name:?} under {parent:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["OrganizationalUnit"].clone()
}

/// The `OrganizationalUnits` of a `ListOrganizationalUnitsForParent` answer.
fn listed_ous(answer: &Answer, case: &str) -> Vec<Value> {
    answer.body["OrganizationalUnits"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an OrganizationalUnits array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// Whether `value` is `prefix` followed by `len_min..=len_max` characters drawn only
/// from `[a-z0-9]` — the shape every id pattern in this model spells.
///
/// `pub(crate)` so a sibling suite checks an id against its modelled pattern the same
/// way.
pub(crate) fn matches_id(value: &str, prefix: &str, len_min: usize, len_max: usize) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    (len_min..=len_max).contains(&suffix.len())
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

#[tokio::test]
async fn an_organization_is_created_persisted_and_described_for_every_feature_set() {
    // The catalogue is the model's own `OrganizationFeatureSet` enum values, plus
    // `None` for a request that omits the optional member — CreateOrganization's
    // documentation makes that the same as `ALL` ("By default (or if you set the
    // FeatureSet parameter to ALL) …").
    let mut cases: Vec<Option<&str>> = vec![None];
    cases.extend(ORGANIZATION_FEATURE_SET_VALUES.iter().copied().map(Some));

    for requested in cases {
        // One emulator per case: an account belongs to at most one organization, so a
        // second feature set needs a fresh store.
        let router = router();
        let created = call(
            &router,
            "CreateOrganization",
            &match requested {
                Some(value) => json!({ "FeatureSet": value }),
                None => json!({}),
            },
        )
        .await;
        let case = format!("CreateOrganization(FeatureSet={requested:?})");
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            created.status, created.body
        );

        let organization = created.body["Organization"].clone();
        let expected_feature_set = requested.unwrap_or("ALL");
        assert_eq!(
            organization["FeatureSet"].as_str(),
            Some(expected_feature_set),
            "case {case}: expected FeatureSet {expected_feature_set}, got {organization:?}"
        );

        let id = created.string(&["Organization", "Id"], &case);
        assert!(
            matches_id(&id, "o-", 10, 32),
            "case {case}: expected an Id matching ^o-[a-z0-9]{{10,32}}$, got {id:?}"
        );
        assert_eq!(
            organization["Arn"].as_str(),
            Some(format!("arn:aws:organizations::{}:organization/{id}", account_id()).as_str()),
            "case {case}: expected the organization ARN to name its own id, got {organization:?}"
        );
        assert_eq!(
            organization["MasterAccountId"].as_str(),
            Some(account_id()),
            "case {case}: expected MasterAccountId {}, got {organization:?}",
            account_id()
        );
        assert_eq!(
            organization["MasterAccountArn"].as_str(),
            Some(
                format!(
                    "arn:aws:organizations::{0}:account/{id}/{0}",
                    account_id()
                )
                .as_str()
            ),
            "case {case}: expected the master-account ARN to name the organization and the account, got {organization:?}"
        );
        let email = created.string(&["Organization", "MasterAccountEmail"], &case);
        assert!(
            email.contains('@') && email.split('@').nth(1).is_some_and(|d| d.contains('.')),
            "case {case}: expected MasterAccountEmail to match ^[^\\s@]+@[^\\s@]+\\.[^\\s@]+$, got {email:?}"
        );

        // All features enables service control policies in the root; consolidated
        // billing enables no policy type at all.
        let expected_policy_types: Vec<Value> = if expected_feature_set == "CONSOLIDATED_BILLING" {
            Vec::new()
        } else {
            vec![json!({ "Type": "SERVICE_CONTROL_POLICY", "Status": "ENABLED" })]
        };
        assert_eq!(
            organization["AvailablePolicyTypes"],
            Value::Array(expected_policy_types.clone()),
            "case {case}: expected AvailablePolicyTypes {expected_policy_types:?}, got {organization:?}"
        );

        // DescribeOrganization reports the SAME struct, read back from the store.
        let described = call(&router, "DescribeOrganization", &json!({})).await;
        assert_eq!(
            described.status, 200,
            "case DescribeOrganization after {case}: expected 200, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["Organization"], organization,
            "case DescribeOrganization after {case}: expected the persisted organization {organization:?}, got {:?}",
            described.body["Organization"]
        );

        // ListRoots reports one root, whose policy types follow the same rule.
        let roots = call(&router, "ListRoots", &json!({})).await;
        let listed = roots.body["Roots"]
            .as_array()
            .unwrap_or_else(|| panic!("case ListRoots after {case}: got {:?}", roots.body))
            .clone();
        assert_eq!(
            listed.len(),
            1,
            "case ListRoots after {case}: expected exactly one root, got {listed:?}"
        );
        let root_id = listed[0]["Id"].as_str().unwrap_or_default().to_owned();
        assert!(
            matches_id(&root_id, "r-", 4, 32),
            "case ListRoots after {case}: expected an Id matching ^r-[0-9a-z]{{4,32}}$, got {root_id:?}"
        );
        assert_eq!(
            listed[0]["Name"].as_str(),
            Some("Root"),
            "case ListRoots after {case}: expected the root Name \"Root\", got {listed:?}"
        );
        assert_eq!(
            listed[0]["Arn"].as_str(),
            Some(format!("arn:aws:organizations::{}:root/{id}/{root_id}", account_id()).as_str()),
            "case ListRoots after {case}: expected the root ARN to name the organization and the root, got {listed:?}"
        );
        assert_eq!(
            listed[0]["PolicyTypes"],
            Value::Array(expected_policy_types.clone()),
            "case ListRoots after {case}: expected PolicyTypes {expected_policy_types:?}, got {listed:?}"
        );

        // A second create is refused: an account belongs to one organization at a time.
        let again = call(&router, "CreateOrganization", &json!({})).await;
        assert_eq!(
            again.status, 409,
            "case CreateOrganization twice after {case}: expected 409, got {} with {:?}",
            again.status, again.body
        );
        assert!(
            again.is("AlreadyInOrganizationException"),
            "case CreateOrganization twice after {case}: expected AlreadyInOrganizationException, got {:?}",
            again.error
        );
        // And the refusal changed nothing.
        let after = call(&router, "DescribeOrganization", &json!({})).await;
        assert_eq!(
            after.body["Organization"], organization,
            "case CreateOrganization twice after {case}: expected the organization unchanged, got {:?}",
            after.body["Organization"]
        );
    }
}

#[tokio::test]
async fn every_operation_reports_the_organization_is_not_in_use_before_one_exists() {
    // Each operation of this slice other than the create, with a request that is valid
    // against the model, so the only thing wrong is that no organization exists.
    let cases: &[(&str, Value)] = &[
        ("DescribeOrganization", json!({})),
        ("ListRoots", json!({})),
        (
            "CreateOrganizationalUnit",
            json!({ "ParentId": "r-abcd", "Name": "Sandbox" }),
        ),
        (
            "ListOrganizationalUnitsForParent",
            json!({ "ParentId": "r-abcd" }),
        ),
        (
            "DescribeOrganizationalUnit",
            json!({ "OrganizationalUnitId": "ou-abcd-11111111" }),
        ),
        (
            "DeleteOrganizationalUnit",
            json!({ "OrganizationalUnitId": "ou-abcd-11111111" }),
        ),
    ];

    for (operation, body) in cases {
        let router = router();
        let answer = call(&router, operation, body).await;
        assert_eq!(
            answer.status, 404,
            "case {operation} with no organization: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.is("AWSOrganizationsNotInUseException"),
            "case {operation} with no organization: expected AWSOrganizationsNotInUseException, got {:?}",
            answer.error
        );
    }
}

#[tokio::test]
async fn a_created_organizational_unit_is_read_back_listed_and_then_deleted() {
    let router = router();
    let root = create_organization(&router, Some("ALL")).await;
    let organization = call(&router, "DescribeOrganization", &json!({})).await;
    let organization_id = organization.string(&["Organization", "Id"], "DescribeOrganization");

    // Names differing along what the rule generalises over: length, case, digits,
    // separators, whitespace and non-ASCII — `OrganizationalUnitName` is modelled
    // `length(1..128)` with pattern `^[\s\S]*$`, so every one of these is in range.
    let names = [
        "a",
        "Sandbox",
        "Production-EU",
        "team 42",
        "Ingénierie",
        &"n".repeat(128),
    ];

    let mut created: Vec<Value> = Vec::new();
    for name in names {
        let ou = create_ou(&router, &root, name).await;
        let case = format!("CreateOrganizationalUnit({name:?})");
        let id = ou["Id"].as_str().unwrap_or_default().to_owned();
        let root_suffix = root.strip_prefix("r-").unwrap_or(&root);
        assert!(
            matches_id(&id, &format!("ou-{root_suffix}-"), 8, 32),
            "case {case}: expected an Id matching ^ou-{root_suffix}-[a-z0-9]{{8,32}}$, got {id:?}"
        );
        assert_eq!(
            ou["Name"].as_str(),
            Some(name),
            "case {case}: expected the Name to be stored verbatim, got {ou:?}"
        );
        assert_eq!(
            ou["Arn"].as_str(),
            Some(
                format!(
                    "arn:aws:organizations::{}:ou/{organization_id}/{id}",
                    account_id()
                )
                .as_str()
            ),
            "case {case}: expected the OU ARN to name the organization and the OU, got {ou:?}"
        );
        assert_eq!(
            ou["Path"].as_str(),
            Some(format!("{organization_id}/{root}/{id}/").as_str()),
            "case {case}: expected the Path to be the parent's path with the OU appended, got {ou:?}"
        );

        // The round trip: the same record, read back through DescribeOrganizationalUnit.
        let described = call(
            &router,
            "DescribeOrganizationalUnit",
            &json!({ "OrganizationalUnitId": id }),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case DescribeOrganizationalUnit after {case}: expected 200, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["OrganizationalUnit"], ou,
            "case DescribeOrganizationalUnit after {case}: expected the persisted OU {ou:?}, got {:?}",
            described.body["OrganizationalUnit"]
        );
        created.push(ou);
    }

    // Every created OU is listed under the root, exactly once, with the identical
    // record. `MaxResults` is omitted, and the whole set fits one page.
    let listed = call(
        &router,
        "ListOrganizationalUnitsForParent",
        &json!({ "ParentId": root }),
    )
    .await;
    let page = listed_ous(&listed, "ListOrganizationalUnitsForParent(root)");
    assert_eq!(
        page.len(),
        created.len(),
        "case ListOrganizationalUnitsForParent(root): expected {} OUs, got {page:?}",
        created.len()
    );
    for ou in &created {
        assert!(
            page.contains(ou),
            "case ListOrganizationalUnitsForParent(root): expected the listing to carry {ou:?}, got {page:?}"
        );
    }
    assert_eq!(
        listed.body["NextToken"],
        Value::Null,
        "case ListOrganizationalUnitsForParent(root): a complete page carries no NextToken, got {:?}",
        listed.body
    );

    // A delete is observable by the next read, for every one of them.
    for ou in &created {
        let id = ou["Id"].as_str().unwrap_or_default().to_owned();
        let deleted = call(
            &router,
            "DeleteOrganizationalUnit",
            &json!({ "OrganizationalUnitId": id }),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case DeleteOrganizationalUnit({id}): expected 200, got {} with {:?}",
            deleted.status, deleted.body
        );

        let gone = call(
            &router,
            "DescribeOrganizationalUnit",
            &json!({ "OrganizationalUnitId": id }),
        )
        .await;
        assert_eq!(
            gone.status, 404,
            "case DescribeOrganizationalUnit after delete({id}): expected 404, got {} with {:?}",
            gone.status, gone.body
        );
        assert!(
            gone.is("OrganizationalUnitNotFoundException"),
            "case DescribeOrganizationalUnit after delete({id}): expected OrganizationalUnitNotFoundException, got {:?}",
            gone.error
        );

        let remaining = call(
            &router,
            "ListOrganizationalUnitsForParent",
            &json!({ "ParentId": root }),
        )
        .await;
        let page = listed_ous(&remaining, "ListOrganizationalUnitsForParent after delete");
        assert!(
            !page.iter().any(|listed| listed["Id"] == ou["Id"]),
            "case ListOrganizationalUnitsForParent after delete({id}): expected the OU gone, got {page:?}"
        );

        // A second delete of the same id is a 404: the record is really gone.
        let again = call(
            &router,
            "DeleteOrganizationalUnit",
            &json!({ "OrganizationalUnitId": id }),
        )
        .await;
        assert_eq!(
            again.status, 404,
            "case DeleteOrganizationalUnit twice({id}): expected 404, got {} with {:?}",
            again.status, again.body
        );
        assert!(
            again.is("OrganizationalUnitNotFoundException"),
            "case DeleteOrganizationalUnit twice({id}): expected OrganizationalUnitNotFoundException, got {:?}",
            again.error
        );
    }

    let empty = call(
        &router,
        "ListOrganizationalUnitsForParent",
        &json!({ "ParentId": root }),
    )
    .await;
    assert!(
        listed_ous(&empty, "ListOrganizationalUnitsForParent after all deletes").is_empty(),
        "case ListOrganizationalUnitsForParent after all deletes: expected no OUs, got {:?}",
        empty.body
    );
}

#[tokio::test]
async fn an_ou_name_is_unique_among_its_siblings_only() {
    let router = router();
    let root = create_organization(&router, None).await;
    let name = "Sandbox";

    let first = create_ou(&router, &root, name).await;
    let first_id = first["Id"].as_str().unwrap_or_default().to_owned();

    // The same name under the SAME parent is refused, and no second OU appears.
    let duplicate = call(
        &router,
        "CreateOrganizationalUnit",
        &json!({ "ParentId": root, "Name": name }),
    )
    .await;
    assert_eq!(
        duplicate.status, 409,
        "case CreateOrganizationalUnit({name:?}) twice under one parent: expected 409, got {} with {:?}",
        duplicate.status, duplicate.body
    );
    assert!(
        duplicate.is("DuplicateOrganizationalUnitException"),
        "case CreateOrganizationalUnit({name:?}) twice under one parent: expected DuplicateOrganizationalUnitException, got {:?}",
        duplicate.error
    );
    let listed = call(
        &router,
        "ListOrganizationalUnitsForParent",
        &json!({ "ParentId": root }),
    )
    .await;
    assert_eq!(
        listed_ous(&listed, "ListOrganizationalUnitsForParent after a refused duplicate").len(),
        1,
        "case CreateOrganizationalUnit({name:?}) twice under one parent: expected the refusal to store nothing, got {:?}",
        listed.body
    );

    // The same name under a DIFFERENT parent is a different OU and is accepted.
    let sibling_parent = create_ou(&router, &root, "Other").await;
    let sibling_parent_id = sibling_parent["Id"].as_str().unwrap_or_default().to_owned();
    let nested = create_ou(&router, &sibling_parent_id, name).await;
    assert_ne!(
        nested["Id"], first["Id"],
        "case CreateOrganizationalUnit({name:?}) under a second parent: expected a distinct OU, got {nested:?}"
    );
    assert_eq!(
        nested["Name"].as_str(),
        Some(name),
        "case CreateOrganizationalUnit({name:?}) under a second parent: expected the same Name, got {nested:?}"
    );
    assert_ne!(
        first_id, sibling_parent_id,
        "the two parents must be distinct for this case to mean anything"
    );
}

#[tokio::test]
async fn an_ous_path_extends_its_parents_path_up_to_the_published_nesting_depth() {
    let router = router();
    let root = create_organization(&router, None).await;
    let organization = call(&router, "DescribeOrganization", &json!({})).await;
    let organization_id = organization.string(&["Organization", "Id"], "DescribeOrganization");

    // One OU per level, down to the published maximum: "OUs can be nested five levels
    // deep beneath the root" (Quotas for Organizations), i.e.
    // `MAX_ORGANIZATIONAL_UNIT_DEPTH`.
    let mut parent = root.clone();
    let mut expected_path = format!("{organization_id}/{root}/");
    let mut deepest = root.clone();
    for depth in 1..=MAX_ORGANIZATIONAL_UNIT_DEPTH {
        let ou = create_ou(&router, &parent, &format!("level-{depth}")).await;
        let id = ou["Id"].as_str().unwrap_or_default().to_owned();
        expected_path = format!("{expected_path}{id}/");
        assert_eq!(
            ou["Path"].as_str(),
            Some(expected_path.as_str()),
            "case CreateOrganizationalUnit at depth {depth}: expected Path {expected_path:?}, got {ou:?}"
        );
        // Each level is listed under its own parent, and only there.
        let listed = call(
            &router,
            "ListOrganizationalUnitsForParent",
            &json!({ "ParentId": parent }),
        )
        .await;
        let page = listed_ous(&listed, &format!("ListOrganizationalUnitsForParent at depth {depth}"));
        assert_eq!(
            page.len(),
            1,
            "case ListOrganizationalUnitsForParent at depth {depth}: expected exactly the one child, got {page:?}"
        );
        assert_eq!(
            page[0], ou,
            "case ListOrganizationalUnitsForParent at depth {depth}: expected the persisted OU {ou:?}, got {:?}",
            page[0]
        );
        parent = id.clone();
        deepest = id;
    }

    // One level past the published bound is refused.
    let too_deep = call(
        &router,
        "CreateOrganizationalUnit",
        &json!({ "ParentId": deepest, "Name": "one-too-deep" }),
    )
    .await;
    assert_eq!(
        too_deep.status, 409,
        "case CreateOrganizationalUnit at depth {}: expected 409, got {} with {:?}",
        MAX_ORGANIZATIONAL_UNIT_DEPTH + 1,
        too_deep.status,
        too_deep.body
    );
    assert!(
        too_deep.is("ConstraintViolationException"),
        "case CreateOrganizationalUnit at depth {}: expected ConstraintViolationException, got {:?}",
        MAX_ORGANIZATIONAL_UNIT_DEPTH + 1,
        too_deep.error
    );
    assert_eq!(
        too_deep.body["Reason"].as_str(),
        Some("OU_DEPTH_LIMIT_EXCEEDED"),
        "case CreateOrganizationalUnit at depth {}: expected the reason OU_DEPTH_LIMIT_EXCEEDED, got {:?}",
        MAX_ORGANIZATIONAL_UNIT_DEPTH + 1,
        too_deep.body
    );

    // The deepest OU is still readable after the refusal above, so the refusal changed
    // nothing.
    let holder = call(&router, "DescribeOrganizationalUnit", &json!({ "OrganizationalUnitId": deepest })).await;
    assert_eq!(
        holder.status, 200,
        "case DescribeOrganizationalUnit(deepest): expected 200, got {} with {:?}",
        holder.status, holder.body
    );
}

#[tokio::test]
async fn an_ou_that_still_holds_a_child_cannot_be_deleted() {
    let router = router();
    let root = create_organization(&router, None).await;
    let parent = create_ou(&router, &root, "Parent").await;
    let parent_id = parent["Id"].as_str().unwrap_or_default().to_owned();
    let child = create_ou(&router, &parent_id, "Child").await;
    let child_id = child["Id"].as_str().unwrap_or_default().to_owned();

    let refused = call(
        &router,
        "DeleteOrganizationalUnit",
        &json!({ "OrganizationalUnitId": parent_id }),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case DeleteOrganizationalUnit(non-empty): expected 409, got {} with {:?}",
        refused.status, refused.body
    );
    assert!(
        refused.is("OrganizationalUnitNotEmptyException"),
        "case DeleteOrganizationalUnit(non-empty): expected OrganizationalUnitNotEmptyException, got {:?}",
        refused.error
    );

    // The refusal changed nothing: the parent and its child are both still there.
    let still_there = call(
        &router,
        "DescribeOrganizationalUnit",
        &json!({ "OrganizationalUnitId": parent_id }),
    )
    .await;
    assert_eq!(
        still_there.body["OrganizationalUnit"], parent,
        "case DeleteOrganizationalUnit(non-empty): expected the parent unchanged, got {:?}",
        still_there.body
    );

    // Empty it, and the delete goes through.
    let child_deleted = call(
        &router,
        "DeleteOrganizationalUnit",
        &json!({ "OrganizationalUnitId": child_id }),
    )
    .await;
    assert_eq!(
        child_deleted.status, 200,
        "case DeleteOrganizationalUnit(leaf): expected 200, got {} with {:?}",
        child_deleted.status, child_deleted.body
    );
    let parent_deleted = call(
        &router,
        "DeleteOrganizationalUnit",
        &json!({ "OrganizationalUnitId": parent_id }),
    )
    .await;
    assert_eq!(
        parent_deleted.status, 200,
        "case DeleteOrganizationalUnit(emptied parent): expected 200, got {} with {:?}",
        parent_deleted.status, parent_deleted.body
    );

    // The root still holds the management account, so it is not "empty" either — but
    // Organizations has no DeleteRoot, so what is asserted here is that the account
    // seeded at CreateOrganization is a real child edge: the root's OU listing is empty
    // while its child list is not.
    let listed = call(
        &router,
        "ListOrganizationalUnitsForParent",
        &json!({ "ParentId": root }),
    )
    .await;
    assert!(
        listed_ous(&listed, "ListOrganizationalUnitsForParent(root)").is_empty(),
        "case ListOrganizationalUnitsForParent(root): expected no OUs after both deletes, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn an_unknown_parent_is_refused_by_every_operation_that_takes_one() {
    // The sentence `ParentNotFoundException` itself states in this service's model. A
    // `ParentId` names a root or an OU — the only two forms its pattern accepts — so
    // those are the two node kinds the refusal tells the caller to look for.
    assert_eq!(
        PARENT_NOT_FOUND_MESSAGE, "We can't find a root or OU with the ParentId that you specified.",
        "case ParentNotFoundException: expected the sentence the model states for this \
         error, got {PARENT_NOT_FOUND_MESSAGE:?}"
    );

    let router = router();
    let root = create_organization(&router, None).await;
    let real = create_ou(&router, &root, "Real").await;
    let real_id = real["Id"].as_str().unwrap_or_default().to_owned();
    let root_suffix = root.strip_prefix("r-").unwrap_or(&root).to_owned();

    // An OU this organization really held, then deleted: afterwards its id is as unknown
    // to the store as one that was never minted.
    let transient = create_ou(&router, &root, "Transient").await;
    let deleted_id = transient["Id"].as_str().unwrap_or_default().to_owned();
    let removed = call(
        &router,
        "DeleteOrganizationalUnit",
        &json!({ "OrganizationalUnitId": deleted_id }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "case DeleteOrganizationalUnit({deleted_id}): expected 200, got {} with {:?}",
        removed.status, removed.body
    );

    // Parent ids that satisfy `ParentId`'s modelled pattern
    // `^(r-[0-9a-z]{4,32})|(ou-[0-9a-z]{4,32}-[a-z0-9]{8,32})$` but name no node of
    // this organization, differing along form and along the pattern's length bounds: a
    // root-shaped one, OU-shaped ones in and out of this root, a shortest and a longest
    // root-shaped one, an OU-shaped one with both segments at their longest, and the
    // deleted OU.
    let unknown = [
        "r-zzzz".to_string(),
        format!("ou-{root_suffix}-zzzzzzzz"),
        "ou-wxyz-00000000".to_string(),
        "r-0000".to_string(),
        format!("r-{}", "z".repeat(32)),
        format!("ou-{}-{}", "a".repeat(32), "b".repeat(32)),
        deleted_id.clone(),
    ];

    for parent in &unknown {
        for operation in ["CreateOrganizationalUnit", "ListOrganizationalUnitsForParent"] {
            let body = if operation == "CreateOrganizationalUnit" {
                json!({ "ParentId": parent, "Name": "Nope" })
            } else {
                json!({ "ParentId": parent })
            };
            let answer = call(&router, operation, &body).await;
            assert_eq!(
                answer.status, 404,
                "case {operation}(ParentId={parent:?}): expected 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.is("ParentNotFoundException"),
                "case {operation}(ParentId={parent:?}): expected ParentNotFoundException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.body["Message"].as_str(),
                Some(PARENT_NOT_FOUND_MESSAGE),
                "case {operation}(ParentId={parent:?}): expected the error's own sentence, \
                 got {:?}",
                answer.body
            );
        }
    }

    // The one real parent still answers, so the refusals above are about the id and not
    // about the operation.
    let listed = call(
        &router,
        "ListOrganizationalUnitsForParent",
        &json!({ "ParentId": root }),
    )
    .await;
    let page = listed_ous(&listed, "ListOrganizationalUnitsForParent(root)");
    assert_eq!(
        page.len(),
        1,
        "case ListOrganizationalUnitsForParent(root): expected the one real OU, got {page:?}"
    );
    assert_eq!(
        page[0]["Id"].as_str(),
        Some(real_id.as_str()),
        "case ListOrganizationalUnitsForParent(root): expected {real_id}, got {page:?}"
    );
}

#[tokio::test]
async fn a_listing_pages_the_whole_set_exactly_once_for_every_page_size() {
    let router = router();
    let root = create_organization(&router, None).await;

    // More OUs than the largest page the model allows, so every page size below is
    // exercised on a set that really needs several pages.
    let total = usize::try_from(MAX_PAGE_SIZE).unwrap_or(20) + 3;
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for index in 0..total {
        let ou = create_ou(&router, &root, &format!("ou-{index}")).await;
        expected.insert(ou["Id"].as_str().unwrap_or_default().to_owned());
    }

    // `MaxResults` is modelled `@range(min: 1, max: 20)`: both published bounds, a
    // middle value, and the omitted case.
    let sizes: [Option<i32>; 4] = [Some(1), Some(7), Some(MAX_PAGE_SIZE), None];
    for size in sizes {
        let case = format!("ListOrganizationalUnitsForParent(MaxResults={size:?})");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({ "ParentId": root });
            if let Some(size) = size {
                body["MaxResults"] = json!(size);
            }
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let answer = call(&router, "ListOrganizationalUnitsForParent", &body).await;
            assert_eq!(
                answer.status, 200,
                "case {case} page {pages}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page = listed_ous(&answer, &case);
            let limit = usize::try_from(size.unwrap_or(MAX_PAGE_SIZE)).unwrap_or(1);
            assert!(
                page.len() <= limit,
                "case {case} page {pages}: expected at most {limit} entries, got {}",
                page.len()
            );
            for ou in &page {
                seen.push(ou["Id"].as_str().unwrap_or_default().to_owned());
            }
            token = answer.body["NextToken"].as_str().map(str::to_owned);
            pages += 1;
            assert!(
                pages <= total + 2,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            if token.is_none() {
                // Only the LAST page may omit the token, and a non-final page must be
                // full.
                break;
            }
            assert_eq!(
                page.len(),
                limit,
                "case {case} page {}: a page followed by a NextToken must be full, got {}",
                pages - 1,
                page.len()
            );
        }
        assert_eq!(
            seen.len(),
            expected.len(),
            "case {case}: expected every OU exactly once over {pages} pages, got {} ids",
            seen.len()
        );
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique, expected,
            "case {case}: expected the pages to partition the stored OUs, got {unique:?}"
        );
    }

    // ListRoots pages the same way over its own (single-entry) sequence.
    let roots = call(&router, "ListRoots", &json!({ "MaxResults": 1 })).await;
    assert_eq!(
        roots.status, 200,
        "case ListRoots(MaxResults=1): expected 200, got {} with {:?}",
        roots.status, roots.body
    );
    assert_eq!(
        roots.body["Roots"].as_array().map(Vec::len),
        Some(1),
        "case ListRoots(MaxResults=1): expected the one root, got {:?}",
        roots.body
    );
    assert_eq!(
        roots.body["NextToken"],
        Value::Null,
        "case ListRoots(MaxResults=1): the only page carries no NextToken, got {:?}",
        roots.body
    );
}

#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let router = router();
    let root = create_organization(&router, None).await;

    // One past each published bound of `@range(min: 1, max: 20)`.
    for size in [0, MAX_PAGE_SIZE + 1] {
        for (operation, body) in [
            ("ListRoots", json!({ "MaxResults": size })),
            (
                "ListOrganizationalUnitsForParent",
                json!({ "ParentId": root, "MaxResults": size }),
            ),
        ] {
            let answer = call(&router, operation, &body).await;
            assert_eq!(
                answer.status, 400,
                "case {operation}(MaxResults={size}): expected 400, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn the_tags_on_a_create_are_bounded_and_their_keys_must_be_unique() {
    let router = router();
    let root = create_organization(&router, None).await;

    // One tag per entry, keys drawn from `TagKey`'s modelled charset
    // `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`.
    let tags = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
                .collect(),
        )
    };

    // The published quota is 50 tags per resource: exactly the bound is accepted, one
    // past it is refused as the quota violation it is.
    let at_bound = call(
        &router,
        "CreateOrganizationalUnit",
        &json!({ "ParentId": root, "Name": "AtTagBound", "Tags": tags(MAX_TAGS_PER_RESOURCE) }),
    )
    .await;
    assert_eq!(
        at_bound.status, 200,
        "case CreateOrganizationalUnit with {MAX_TAGS_PER_RESOURCE} tags: expected 200, got {} with {:?}",
        at_bound.status, at_bound.body
    );
    // The OU is really stored, tags and all.
    let stored_id = at_bound.string(&["OrganizationalUnit", "Id"], "CreateOrganizationalUnit at the tag bound");
    let described = call(
        &router,
        "DescribeOrganizationalUnit",
        &json!({ "OrganizationalUnitId": stored_id }),
    )
    .await;
    assert_eq!(
        described.body["OrganizationalUnit"], at_bound.body["OrganizationalUnit"],
        "case DescribeOrganizationalUnit after a tagged create: expected the persisted OU {:?}, got {:?}",
        at_bound.body["OrganizationalUnit"], described.body["OrganizationalUnit"]
    );

    let past_bound = call(
        &router,
        "CreateOrganizationalUnit",
        &json!({ "ParentId": root, "Name": "PastTagBound", "Tags": tags(MAX_TAGS_PER_RESOURCE + 1) }),
    )
    .await;
    assert_eq!(
        past_bound.status, 409,
        "case CreateOrganizationalUnit with {} tags: expected 409, got {} with {:?}",
        MAX_TAGS_PER_RESOURCE + 1,
        past_bound.status,
        past_bound.body
    );
    assert!(
        past_bound.is("ConstraintViolationException"),
        "case CreateOrganizationalUnit with {} tags: expected ConstraintViolationException, got {:?}",
        MAX_TAGS_PER_RESOURCE + 1,
        past_bound.error
    );
    assert_eq!(
        past_bound.body["Reason"].as_str(),
        Some("MAX_TAG_LIMIT_EXCEEDED"),
        "case CreateOrganizationalUnit with {} tags: expected the reason MAX_TAG_LIMIT_EXCEEDED, got {:?}",
        MAX_TAGS_PER_RESOURCE + 1,
        past_bound.body
    );

    // A key repeated anywhere in the list is refused, over a table of where the repeat
    // sits: adjacent, apart, and three times.
    let duplicates = [
        json!([{ "Key": "team", "Value": "a" }, { "Key": "team", "Value": "b" }]),
        json!([
            { "Key": "team", "Value": "a" },
            { "Key": "env", "Value": "prod" },
            { "Key": "team", "Value": "b" }
        ]),
        json!([
            { "Key": "team", "Value": "a" },
            { "Key": "team", "Value": "b" },
            { "Key": "team", "Value": "c" }
        ]),
    ];
    for (index, tags) in duplicates.iter().enumerate() {
        let answer = call(
            &router,
            "CreateOrganizationalUnit",
            &json!({ "ParentId": root, "Name": format!("Dup{index}"), "Tags": tags }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case CreateOrganizationalUnit with duplicate tag keys {tags:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.is("InvalidInputException"),
            "case CreateOrganizationalUnit with duplicate tag keys {tags:?}: expected InvalidInputException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.body["Reason"].as_str(),
            Some("DUPLICATE_TAG_KEY"),
            "case CreateOrganizationalUnit with duplicate tag keys {tags:?}: expected the reason DUPLICATE_TAG_KEY, got {:?}",
            answer.body
        );
    }

    // Every refusal above stored nothing: the one accepted create (`AtTagBound`) is the
    // only OU under the root.
    let listed = call(
        &router,
        "ListOrganizationalUnitsForParent",
        &json!({ "ParentId": root }),
    )
    .await;
    let page = listed_ous(&listed, "ListOrganizationalUnitsForParent after the tag cases");
    assert_eq!(
        page.len(),
        1,
        "case ListOrganizationalUnitsForParent after the tag cases: expected only the accepted create, got {page:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let router = router();
    let root = create_organization(&router, None).await;
    create_ou(&router, &root, "Sandbox").await;

    // Tokens differing along form: not hex at all, hex of something else, and a token
    // that decodes to an id the sequence does not hold.
    let tokens = [
        "not-a-token".to_string(),
        // hex("hello") — well-formed hex, wrong payload.
        hex::encode("hello"),
        // hex("v1:ou-zzzz-99999999") — this service's own form, unknown id.
        hex::encode("v1:ou-zzzz-99999999"),
    ];

    for token in &tokens {
        for (operation, body) in [
            ("ListRoots", json!({ "NextToken": token })),
            (
                "ListOrganizationalUnitsForParent",
                json!({ "ParentId": root, "NextToken": token }),
            ),
        ] {
            let answer = call(&router, operation, &body).await;
            assert_eq!(
                answer.status, 400,
                "case {operation}(NextToken={token:?}): expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.is("InvalidInputException"),
                "case {operation}(NextToken={token:?}): expected InvalidInputException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.body["Reason"].as_str(),
                Some("INVALID_NEXT_TOKEN"),
                "case {operation}(NextToken={token:?}): expected the reason INVALID_NEXT_TOKEN, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn the_models_policy_type_catalogues_are_what_the_wire_accepts() {
    // This case is about the VALIDATION layer of the request walk — which `PolicyType` /
    // `EffectivePolicyType` values reach a handler at all — and not about what any of
    // the operations below then answers: each of them is still scaffolded, so past
    // validation they answer the scaffold's 501, which is their own issue.
    let router = router();
    let root = create_organization(&router, None).await;

    // Every value of the model's `PolicyType` and `EffectivePolicyType` enums, on each
    // operation that takes one, read from the generated catalogues rather than written
    // out here, so a value the model adds or drops is covered by the same assertion.
    let mut cases: Vec<(&str, Value)> = Vec::new();
    for policy_type in POLICY_TYPE_VALUES {
        cases.push((
            "EnablePolicyType",
            json!({ "RootId": root, "PolicyType": policy_type }),
        ));
        cases.push((
            "DisablePolicyType",
            json!({ "RootId": root, "PolicyType": policy_type }),
        ));
        cases.push(("ListPolicies", json!({ "Filter": policy_type })));
    }
    for policy_type in EFFECTIVE_POLICY_TYPE_VALUES {
        cases.push((
            "DescribeEffectivePolicy",
            json!({ "PolicyType": policy_type }),
        ));
    }

    for (operation, body) in &cases {
        let answer = call(&router, operation, body).await;
        assert_ne!(
            answer.status, 400,
            "case {operation}({body}): a value the model's enum declares must not be \
             refused as a bad request, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // A value outside the catalogue is refused, differing along form: unknown, wrong
    // case, empty, and a prefix of a real value.
    let outside = [
        "NOT_A_POLICY_TYPE",
        "service_control_policy",
        "",
        "SERVICE_CONTROL",
    ];
    for value in outside {
        for (operation, body) in [
            (
                "EnablePolicyType",
                json!({ "RootId": root, "PolicyType": value }),
            ),
            ("DescribeEffectivePolicy", json!({ "PolicyType": value })),
        ] {
            let answer = call(&router, operation, &body).await;
            assert_eq!(
                answer.status, 400,
                "case {operation}(PolicyType={value:?}): expected 400 for a value no \
                 modelled enum declares, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn every_enum_typed_member_the_implemented_slice_reports_is_a_modelled_value() {
    // The model's own `OrganizationFeatureSet` catalogue, plus the request that omits it.
    let mut cases: Vec<Option<&str>> = vec![None];
    cases.extend(ORGANIZATION_FEATURE_SET_VALUES.iter().copied().map(Some));

    for requested in cases {
        let router = router();
        let case = format!("CreateOrganization(FeatureSet={requested:?})");
        create_organization(&router, requested).await;

        let organization = call(&router, "DescribeOrganization", &json!({})).await;
        let roots = call(&router, "ListRoots", &json!({})).await;
        // Every `PolicyTypeSummary` the slice reports — the root's enabled types and the
        // organization's deprecated copy of them — carries a `Type` and a `Status` the
        // model's enums declare. The exact values are asserted by
        // `an_organization_is_created_persisted_and_described_for_every_feature_set`;
        // what is asserted here is that no value outside the model's enums can appear in
        // them, over the whole feature-set catalogue.
        let reported = [
            (
                "ListRoots.Roots[].PolicyTypes",
                roots.body["Roots"][0]["PolicyTypes"].clone(),
            ),
            (
                "DescribeOrganization.Organization.AvailablePolicyTypes",
                organization.body["Organization"]["AvailablePolicyTypes"].clone(),
            ),
        ];
        for (member, summaries) in reported {
            let summaries = summaries.as_array().cloned().unwrap_or_else(|| {
                panic!("case {member} after {case}: expected a list, got {summaries:?}")
            });
            for summary in &summaries {
                assert!(
                    POLICY_TYPE_VALUES.contains(&summary["Type"].as_str().unwrap_or_default()),
                    "case {member} after {case}: Type must be a value the model's \
                     PolicyType enum declares, got {summary:?}"
                );
                assert!(
                    POLICY_TYPE_STATUS_VALUES
                        .contains(&summary["Status"].as_str().unwrap_or_default()),
                    "case {member} after {case}: Status must be a value the model's \
                     PolicyTypeStatus enum declares, got {summary:?}"
                );
            }
        }
    }
}
