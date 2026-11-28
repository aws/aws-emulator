// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Opportunity lifecycle: `CreateOpportunity`,
//! `GetOpportunity`, `UpdateOpportunity`, `AssignOpportunity`, `SubmitOpportunity` and
//! `ListOpportunities` — and, in [`customer_accounts`], for the CustomerAccount
//! lifecycle: `CreateCustomerAccount`, `GetCustomerAccount`, `UpdateCustomerAccount`,
//! `DeleteCustomerAccount`, `SearchCustomerAccounts` and
//! `GetCustomerAccountStatistics`, which share this suite's wire harness.
//!
//! Every call goes through the same [`MultiServiceRouter`] the production front door
//! mounts, as an `awsJson1_0` `POST /` with an `X-Amz-Target` header, so what is
//! asserted is the wire behaviour and not a handler's return value.
//!
//! Each test is a TABLE over the class its rule covers — the rows differ in catalog,
//! in which optional members a request carries, in list sizes, in page size, in sort
//! key and in filter shape — because the rules under test ("whatever was created comes
//! back", "a page bound is honoured and resumable", "a stale version token is a
//! conflict") hold over that whole class and not just over one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralselling_awspartnercentralselling::state::{
    service_state, MAX_CREATE_REPLAYS, MAX_OPPORTUNITIES,
};
use service_com_amazonaws_partnercentralselling_awspartnercentralselling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every test signs for; it is also the region the ARNs must carry.
const REGION: &str = "us-east-1";

/// A body long enough for `Project.CustomerBusinessProblem` (`^(?s).{20,2000}$`).
const BUSINESS_PROBLEM: &str = "The customer needs to migrate a monolith off premises.";

/// The emulator under test: one shared engine, this service mounted on the router.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine handle, for a test that has to look at the
/// store itself rather than only at what the wire reports.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise operation behaviour, not SigV4: the signature a real client
    // sends is verified by the emulator, never by a service.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One answered call: the HTTP status and the parsed JSON body.
struct Call {
    status: u16,
    body: Value,
}

impl Call {
    /// The body of a call that must have succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {} with body {}",
            self.status, self.body
        );
        &self.body
    }

    /// The wire error code (`__type`), with any namespace prefix stripped.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Assert this call failed with `status` and `code`.
    fn expect_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code().as_str()),
            (status, code),
            "case {case}: expected HTTP {status} {code}, got HTTP {} {} with body {}",
            self.status,
            self.error_code(),
            self.body
        );
    }

    /// The record version a response reports: the `LastModifiedDate` member, as the
    /// JSON value a client echoes back into `UpdateOpportunity`. It is a modelled
    /// `DateTime`, so the test never assumes which wire form it takes.
    fn version(&self, case: &str) -> Value {
        let value = self
            .body
            .get("LastModifiedDate")
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            !value.is_null(),
            "case {case}: expected a LastModifiedDate in the response, got {}",
            self.body
        );
        value
    }

    /// A string member of the body.
    fn field(&self, case: &str, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a string `{member}` in the response, got {}",
                    self.body
                )
            })
            .to_string()
    }
}

/// Send one operation call.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Call {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/partnercentral-selling/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("https://partnercentral-selling.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSPartnerCentralSelling.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the test body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Call { status, body }
}

/// Wait out the millisecond the timestamps are stamped at.
///
/// `CreatedDate` and `LastModifiedDate` come from the real clock, so two calls inside
/// one millisecond carry the same instant and a test asserting a time ORDERING would
/// be deciding on the tie-break instead. Only the test that asserts a time order
/// calls this; nothing in the service sleeps.
async fn separate_instants() {
    tokio::time::sleep(std::time::Duration::from_millis(3)).await;
}

/// A `CreateOpportunity` request: `members` plus the two required members.
fn create_body(catalog: &str, client_token: &str, members: &Value) -> Value {
    let mut body = members.clone();
    body["Catalog"] = json!(catalog);
    body["ClientToken"] = json!(client_token);
    body
}

/// Create one opportunity and return its id.
async fn create_opportunity(
    router: &MultiServiceRouter,
    case: &str,
    catalog: &str,
    client_token: &str,
    members: &Value,
) -> String {
    let created = call(
        router,
        "CreateOpportunity",
        &create_body(catalog, client_token, members),
    )
    .await;
    created.ok(case);
    created.field(case, "Id")
}

/// Read one opportunity back.
async fn get_opportunity(router: &MultiServiceRouter, catalog: &str, id: &str) -> Call {
    call(
        router,
        "GetOpportunity",
        &json!({ "Catalog": catalog, "Identifier": id }),
    )
    .await
}

/// The modelled `OpportunityIdentifier` pattern, `^O[0-9]{1,19}$`.
fn is_opportunity_id(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('O') else {
        return false;
    };
    (1..=19).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Assert every member `sent` carries is present in `got` with the same value,
/// recursing into nested objects so a response that adds service-owned members (a
/// `ReviewStatus`, a history) still satisfies "whatever was created comes back".
fn assert_echoed(case: &str, path: &str, sent: &Value, got: &Value) {
    match sent {
        Value::Object(members) => {
            let Some(object) = got.as_object() else {
                panic!("case {case}: expected an object at {path}, got {got}");
            };
            for (member, value) in members {
                let child = object.get(member).unwrap_or(&Value::Null);
                assert_echoed(case, &format!("{path}.{member}"), value, child);
            }
        }
        _ => assert_eq!(
            sent, got,
            "case {case}: at {path} expected {sent}, got {got}"
        ),
    }
}

/// The members a create request carries that no `GetOpportunity` response declares
/// (`Origin` and `Tags` are not on `GetOpportunityResponse`; `ClientToken` is the
/// idempotency token), so they are excluded from the echo check.
const NOT_ON_THE_READ: [&str; 3] = ["ClientToken", "Origin", "Tags"];

/// The payload rows every create/read test spans.
fn create_payloads() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "only the required members, AWS catalog",
            "AWS",
            json!({}),
        ),
        (
            "customer, project and lifecycle, Sandbox catalog",
            "Sandbox",
            json!({
                "Customer": {
                    "Account": {
                        "CompanyName": "Acme",
                        "WebsiteUrl": "https://acme.test",
                        "Industry": "Software and Internet",
                        "Address": { "City": "Seattle", "CountryCode": "US", "PostalCode": "98101" }
                    }
                },
                "Project": { "Title": "Migration", "CustomerBusinessProblem": BUSINESS_PROBLEM },
                "LifeCycle": { "Stage": "Prospect", "TargetCloseDate": "2027-01-31" }
            }),
        ),
        (
            "every top-level member, lists with several entries",
            "AWS",
            json!({
                "Customer": {
                    "Account": { "CompanyName": "Globex", "WebsiteUrl": "https://globex.test" },
                    "Contacts": [
                        { "Email": "buyer@globex.test", "FirstName": "Bea", "LastName": "Buyer" }
                    ]
                },
                "Project": {
                    "Title": "Data lake",
                    "CustomerBusinessProblem": BUSINESS_PROBLEM,
                    "DeliveryModels": ["SaaS or PaaS", "Managed Services"]
                },
                "LifeCycle": {
                    "Stage": "Qualified",
                    "TargetCloseDate": "2027-06-30",
                    "NextSteps": "Schedule the architecture review"
                },
                "OpportunityType": "Net New Business",
                "Origin": "Partner Referral",
                "NationalSecurity": "No",
                "PartnerOpportunityIdentifier": "crm-4711",
                "PrimaryNeedsFromAws": [
                    "Co-Sell - Architectural Validation",
                    "Co-Sell - Pricing Assistance"
                ],
                "OpportunityTeam": [
                    {
                        "Email": "pam@partner.test",
                        "FirstName": "Pam",
                        "LastName": "Manager",
                        "BusinessTitle": "PartnerAccountManager"
                    },
                    {
                        "Email": "owen@partner.test",
                        "FirstName": "Owen",
                        "LastName": "Owner",
                        "BusinessTitle": "OpportunityOwner"
                    }
                ],
                "Marketing": { "CampaignName": "summer-cosell" },
                "SoftwareRevenue": {
                    "Value": { "Amount": "1200.50", "CurrencyCode": "USD" },
                    "EffectiveDate": "2027-01-01",
                    "ExpirationDate": "2027-12-31"
                },
                "Tags": [{ "Key": "team", "Value": "cosell" }]
            }),
        ),
    ]
}

#[tokio::test]
async fn create_then_get_returns_every_created_member() {
    let router = emulator();

    for (index, (case, catalog, members)) in create_payloads().into_iter().enumerate() {
        let token = format!("token-{index}");
        let created = call(
            &router,
            "CreateOpportunity",
            &create_body(catalog, &token, &members),
        )
        .await;
        created.ok(case);

        let id = created.field(case, "Id");
        assert!(
            is_opportunity_id(&id),
            "case {case}: Id must match ^O[0-9]{{1,19}}$, got {id:?}"
        );
        assert_eq!(
            created.body.get("PartnerOpportunityIdentifier"),
            members.get("PartnerOpportunityIdentifier"),
            "case {case}: the create response must echo the stored PartnerOpportunityIdentifier, got {}",
            created.body
        );

        let read = get_opportunity(&router, catalog, &id).await;
        let body = read.ok(case).clone();

        // Everything the request carried and the read declares comes back.
        let mut expected = members.clone();
        if let Some(object) = expected.as_object_mut() {
            for member in NOT_ON_THE_READ {
                object.remove(member);
            }
        }
        assert_echoed(case, "GetOpportunity", &expected, &body);

        assert_eq!(
            body.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: the read must report the created Id, got {body}"
        );
        assert_eq!(
            body.get("Catalog").and_then(Value::as_str),
            Some(catalog),
            "case {case}: the read must report the requested catalog, got {body}"
        );
        let arn = read.field(case, "Arn");
        assert_eq!(
            arn,
            format!("arn:aws:partnercentral:{REGION}:000000000000:catalog/{catalog}/opportunity/{id}"),
            "case {case}: unexpected Arn, got {arn:?}"
        );
        assert_eq!(
            body.get("CreatedDate"),
            body.get("LastModifiedDate"),
            "case {case}: a created opportunity's LastModifiedDate is its CreatedDate, got {body}"
        );
        assert_eq!(
            body.pointer("/LifeCycle/ReviewStatus").and_then(Value::as_str),
            Some("Pending Submission"),
            "case {case}: creating an opportunity sets LifeCycle.ReviewStatus to Pending Submission, got {body}"
        );
        // Required on the response, and empty while nothing is associated.
        let related = body
            .get("RelatedEntityIdentifiers")
            .unwrap_or_else(|| panic!("case {case}: RelatedEntityIdentifiers is required, got {body}"));
        for member in [
            "Solutions",
            "AwsProducts",
            "AwsMarketplaceOffers",
            "AwsMarketplaceOfferSets",
            "AwsMarketplaceSolutions",
            "AwsMarketplaceProducts",
        ] {
            assert_eq!(
                related.get(member).and_then(Value::as_array).map(Vec::len),
                Some(0),
                "case {case}: RelatedEntityIdentifiers.{member} must be an empty list, got {related}"
            );
        }
    }
}

#[tokio::test]
async fn next_steps_are_recorded_in_the_lifecycle_history() {
    let router = emulator();
    // The rows differ in whether the created opportunity already had NextSteps, which
    // is what decides how many entries the history holds after one update.
    let cases = [
        ("created without NextSteps", Value::Null, 1_usize),
        ("created with NextSteps", json!("Confirm the budget"), 2),
    ];

    for (index, (case, created_next_steps, expected_entries)) in cases.into_iter().enumerate() {
        let mut life_cycle = json!({ "Stage": "Prospect" });
        if let Some(value) = created_next_steps.as_str() {
            life_cycle["NextSteps"] = json!(value);
        }
        let id = create_opportunity(
            &router,
            case,
            "AWS",
            &format!("history-{index}"),
            &json!({ "LifeCycle": life_cycle }),
        )
        .await;

        let read = get_opportunity(&router, "AWS", &id).await;
        let version = read.version(case);
        let updated = call(
            &router,
            "UpdateOpportunity",
            &json!({
                "Catalog": "AWS",
                "Identifier": id,
                "LastModifiedDate": version,
                "LifeCycle": { "Stage": "Qualified", "NextSteps": "Book the technical validation" }
            }),
        )
        .await;
        updated.ok(case);

        let read = get_opportunity(&router, "AWS", &id).await;
        let body = read.ok(case).clone();
        let history = body
            .pointer("/LifeCycle/NextStepsHistory")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            history.len(),
            expected_entries,
            "case {case}: expected {expected_entries} NextStepsHistory entries, got {body}"
        );
        let last = history.last().cloned().unwrap_or(Value::Null);
        assert_eq!(
            last.get("Value").and_then(Value::as_str),
            Some("Book the technical validation"),
            "case {case}: the newest history entry must carry the new NextSteps, got {last}"
        );
        assert!(
            last.get("Time").is_some(),
            "case {case}: a history entry carries the time it was recorded, got {last}"
        );
    }
}

#[tokio::test]
async fn create_is_idempotent_on_its_client_token() {
    let router = emulator();
    let mut minted = Vec::new();

    for (index, (case, catalog, members)) in create_payloads().into_iter().enumerate() {
        let token = format!("replay-{index}");
        let body = create_body(catalog, &token, &members);

        let first = call(&router, "CreateOpportunity", &body).await;
        first.ok(case);
        let id = first.field(case, "Id");
        minted.push((catalog, id.clone()));

        // Same token, same body: the same record, not a second one.
        let replay = call(&router, "CreateOpportunity", &body).await;
        replay.ok(case);
        assert_eq!(
            replay.field(case, "Id"),
            id,
            "case {case}: replaying a ClientToken with the same body must return the same Id"
        );

        // Same token, different body: the documented conflict.
        let mut changed = body.clone();
        changed["PartnerOpportunityIdentifier"] = json!(format!("crm-changed-{index}"));
        call(&router, "CreateOpportunity", &changed)
            .await
            .expect_error(case, 409, "ConflictException");

        // A fresh token with the same body is a new opportunity.
        let again = call(
            &router,
            "CreateOpportunity",
            &create_body(catalog, &format!("fresh-{index}"), &members),
        )
        .await;
        again.ok(case);
        let second_id = again.field(case, "Id");
        assert_ne!(
            second_id, id,
            "case {case}: a different ClientToken must mint a new Id"
        );
        minted.push((catalog, second_id));
    }

    // The store holds exactly the records the tokens minted — a replay added none.
    for catalog in ["AWS", "Sandbox"] {
        let expected: Vec<String> = minted
            .iter()
            .filter(|(record_catalog, _)| *record_catalog == catalog)
            .map(|(_, id)| id.clone())
            .collect();
        let listed = list_ids(&router, catalog, 100).await;
        let mut sorted = listed.clone();
        sorted.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(
            sorted, want,
            "case {catalog}: the catalog must hold exactly the minted opportunities, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_not_found() {
    let router = emulator();
    let sandbox_id = create_opportunity(&router, "setup", "Sandbox", "nf-1", &json!({})).await;
    let version = get_opportunity(&router, "Sandbox", &sandbox_id)
        .await
        .version("setup");

    // The rows span the operations that resolve an opportunity, and both ways a
    // lookup can miss: an id nothing ever minted, and an id that exists in ANOTHER
    // catalog.
    let assignee = json!({
        "Email": "rep@partner.test",
        "FirstName": "Rep",
        "LastName": "One",
        "BusinessTitle": "OpportunityOwner"
    });
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "GetOpportunity, id never minted",
            "GetOpportunity",
            json!({ "Catalog": "Sandbox", "Identifier": "O999999999" }),
        ),
        (
            "GetOpportunity, id of another catalog",
            "GetOpportunity",
            json!({ "Catalog": "AWS", "Identifier": sandbox_id }),
        ),
        (
            "UpdateOpportunity, id never minted",
            "UpdateOpportunity",
            json!({ "Catalog": "Sandbox", "Identifier": "O999999999", "LastModifiedDate": version }),
        ),
        (
            "UpdateOpportunity, id of another catalog",
            "UpdateOpportunity",
            json!({ "Catalog": "AWS", "Identifier": sandbox_id, "LastModifiedDate": version }),
        ),
        (
            "AssignOpportunity, id never minted",
            "AssignOpportunity",
            json!({ "Catalog": "Sandbox", "Identifier": "O999999999", "Assignee": assignee }),
        ),
        (
            "SubmitOpportunity, id of another catalog",
            "SubmitOpportunity",
            json!({ "Catalog": "AWS", "Identifier": sandbox_id, "InvolvementType": "Co-Sell" }),
        ),
    ];

    for (case, operation, body) in cases {
        call(&router, operation, &body)
            .await
            .expect_error(case, 404, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn update_requires_the_current_last_modified_date() {
    let router = emulator();

    for (index, (case, catalog, members)) in create_payloads().into_iter().enumerate() {
        let id = create_opportunity(&router, case, catalog, &format!("cc-{index}"), &members).await;
        let read = get_opportunity(&router, catalog, &id).await;
        let first_version = read.version(case);

        let update = json!({
            "Catalog": catalog,
            "Identifier": id,
            "LastModifiedDate": first_version,
            "Project": { "Title": "Migration v2", "CustomerBusinessProblem": BUSINESS_PROBLEM }
        });
        let updated = call(&router, "UpdateOpportunity", &update).await;
        updated.ok(case);
        assert_eq!(
            updated.field(case, "Id"),
            id,
            "case {case}: the update response must report the updated Id"
        );
        let second_version = updated.version(case);
        assert_ne!(
            second_version, first_version,
            "case {case}: an update must advance LastModifiedDate"
        );

        // The stale token is refused, the new one is accepted: that is what "advanced"
        // means for a concurrency token.
        call(&router, "UpdateOpportunity", &update)
            .await
            .expect_error(case, 409, "ConflictException");
        let mut retry = update.clone();
        retry["LastModifiedDate"] = json!(second_version);
        call(&router, "UpdateOpportunity", &retry).await.ok(case);

        // The whole payload replaces the stored members: the update above carried no
        // Customer, so the read reports none, while the service-owned ReviewStatus
        // survives.
        let read = get_opportunity(&router, catalog, &id).await;
        let body = read.ok(case).clone();
        assert_eq!(
            body.pointer("/Project/Title").and_then(Value::as_str),
            Some("Migration v2"),
            "case {case}: the update must be visible on the read, got {body}"
        );
        assert!(
            body.get("Customer").is_none(),
            "case {case}: a member omitted from an update is set to null, got {body}"
        );
        assert_eq!(
            body.pointer("/LifeCycle/ReviewStatus").and_then(Value::as_str),
            Some("Pending Submission"),
            "case {case}: ReviewStatus is service-owned and survives an update, got {body}"
        );
    }
}

#[tokio::test]
async fn assigning_an_opportunity_records_the_owner_on_the_team() {
    let router = emulator();
    let created_team = json!([
        {
            "Email": "pam@partner.test",
            "FirstName": "Pam",
            "LastName": "Manager",
            "BusinessTitle": "PartnerAccountManager"
        }
    ]);

    // The rows differ in the assignee's members (with and without a phone) and run in
    // sequence, so the second row is a REASSIGNMENT of the same opportunity.
    let assignees = [
        json!({
            "Email": "first@partner.test",
            "FirstName": "First",
            "LastName": "Owner",
            "BusinessTitle": "OpportunityOwner"
        }),
        json!({
            "Email": "second@partner.test",
            "FirstName": "Second",
            "LastName": "Owner",
            "BusinessTitle": "OpportunityOwner",
            "Phone": "+14255550123"
        }),
    ];

    let id = create_opportunity(
        &router,
        "assign setup",
        "AWS",
        "assign-1",
        &json!({ "OpportunityTeam": created_team }),
    )
    .await;

    for (index, assignee) in assignees.iter().enumerate() {
        let case = format!("assignee {index}");
        let before = get_opportunity(&router, "AWS", &id).await.version(&case);

        call(
            &router,
            "AssignOpportunity",
            &json!({ "Catalog": "AWS", "Identifier": id, "Assignee": assignee }),
        )
        .await
        .ok(&case);

        let read = get_opportunity(&router, "AWS", &id).await;
        let body = read.ok(&case).clone();
        let team = body
            .get("OpportunityTeam")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            team.first().cloned().unwrap_or(Value::Null),
            *assignee,
            "case {case}: the assignee must be the opportunity's owner contact, got {body}"
        );
        assert!(
            team.iter()
                .any(|member| member.get("Email").and_then(Value::as_str)
                    == Some("pam@partner.test")),
            "case {case}: assigning must not drop the created team members, got {body}"
        );
        assert_eq!(
            team.len(),
            2,
            "case {case}: reassignment replaces the previous owner rather than appending, got {body}"
        );
        assert_ne!(
            body.get("LastModifiedDate").cloned().unwrap_or(Value::Null),
            before,
            "case {case}: assigning must advance LastModifiedDate, got {body}"
        );
    }
}

#[tokio::test]
async fn submitting_an_opportunity_moves_its_review_status() {
    let router = emulator();
    // The rows span the `SalesInvolvementType` enum and both `Visibility` values,
    // plus an absent `Visibility`.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("co-sell, full visibility", "Co-Sell", Some("Full")),
        (
            "visibility only, limited visibility",
            "For Visibility Only",
            Some("Limited"),
        ),
        ("co-sell, no visibility member", "Co-Sell", None),
    ];

    for (index, (case, involvement, visibility)) in cases.into_iter().enumerate() {
        let id = create_opportunity(
            &router,
            case,
            "Sandbox",
            &format!("submit-{index}"),
            &json!({ "LifeCycle": { "Stage": "Qualified", "TargetCloseDate": "2027-03-31" } }),
        )
        .await;
        let read = get_opportunity(&router, "Sandbox", &id).await;
        let before = read.version(case);
        assert_eq!(
            read.body.pointer("/LifeCycle/ReviewStatus").and_then(Value::as_str),
            Some("Pending Submission"),
            "case {case}: an unsubmitted opportunity is Pending Submission, got {}",
            read.body
        );

        let mut submit = json!({
            "Catalog": "Sandbox",
            "Identifier": id,
            "InvolvementType": involvement
        });
        if let Some(visibility) = visibility {
            submit["Visibility"] = json!(visibility);
        }
        call(&router, "SubmitOpportunity", &submit).await.ok(case);

        let read = get_opportunity(&router, "Sandbox", &id).await;
        let body = read.ok(case).clone();
        assert_eq!(
            body.pointer("/LifeCycle/ReviewStatus").and_then(Value::as_str),
            Some("Submitted"),
            "case {case}: submitting must move LifeCycle.ReviewStatus to Submitted, got {body}"
        );
        assert_ne!(
            body.get("LastModifiedDate").cloned().unwrap_or(Value::Null),
            before,
            "case {case}: submitting must advance LastModifiedDate, got {body}"
        );

        // A submitted opportunity is under review: it can be neither resubmitted nor
        // edited until the review completes.
        call(&router, "SubmitOpportunity", &submit)
            .await
            .expect_error(case, 400, "ValidationException");
        let version = read.version(case);
        call(
            &router,
            "UpdateOpportunity",
            &json!({
                "Catalog": "Sandbox",
                "Identifier": id,
                "LastModifiedDate": version,
                "Project": { "Title": "edited after submission" }
            }),
        )
        .await
        .expect_error(case, 400, "ValidationException");
    }
}

/// The ids one `ListOpportunities` page of at most `max_results` reports.
async fn list_ids(router: &MultiServiceRouter, catalog: &str, max_results: i64) -> Vec<String> {
    let listed = call(
        router,
        "ListOpportunities",
        &json!({ "Catalog": catalog, "MaxResults": max_results }),
    )
    .await;
    listed.ok("list");
    summaries_ids(&listed.body)
}

/// The ids of a `ListOpportunities` response's summaries.
fn summaries_ids(body: &Value) -> Vec<String> {
    body.get("OpportunitySummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn listing_pages_through_every_opportunity_exactly_once() {
    let router = emulator();
    const RECORDS: usize = 5;
    for index in 0..RECORDS {
        create_opportunity(
            &router,
            "page setup",
            "AWS",
            &format!("page-{index}"),
            &json!({ "Project": { "Title": format!("project {index}") } }),
        )
        .await;
    }
    // One opportunity in another catalog: it must never appear in the AWS pages.
    let other = create_opportunity(&router, "page setup", "Sandbox", "page-other", &json!({})).await;

    let whole = list_ids(&router, "AWS", 100).await;
    assert_eq!(
        whole.len(),
        RECORDS,
        "an unbounded page must report every opportunity of the catalog, got {whole:?}"
    );
    assert!(
        !whole.contains(&other),
        "a list must not cross catalogs, got {whole:?}"
    );

    // The rows span page sizes from one record per page to more than the whole list.
    for page_size in [1_usize, 2, 3, 5, 7] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({ "Catalog": "AWS", "MaxResults": page_size });
            if let Some(token) = next_token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let listed = call(&router, "ListOpportunities", &request).await;
            let body = listed.ok(&case).clone();
            let ids = summaries_ids(&body);
            pages += 1;
            assert!(
                ids.len() <= page_size,
                "case {case}: a page must honour MaxResults, got {} ids",
                ids.len()
            );
            let remaining = RECORDS - seen.len();
            assert_eq!(
                ids.len(),
                remaining.min(page_size),
                "case {case}: expected a full page while records remain, got {ids:?}"
            );
            seen.extend(ids);
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                next_token.is_some(),
                seen.len() < RECORDS,
                "case {case}: NextToken is present exactly while records remain, got {body}"
            );
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= RECORDS + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen, whole,
            "case {case}: paging must report the same records, in the same order, as one page"
        );
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: paging repeated a record, got {seen:?}"
        );
    }

    // A token this service did not issue is refused rather than silently restarting.
    for (case, token) in [
        ("not base64", "!!!not-a-token!!!"),
        ("base64 of foreign content", "Zm9yZ2VkOjEyMw=="),
    ] {
        call(
            &router,
            "ListOpportunities",
            &json!({ "Catalog": "AWS", "NextToken": token }),
        )
        .await
        .expect_error(case, 400, "ValidationException");
    }
}

#[tokio::test]
async fn listing_applies_the_requested_filters() {
    let router = emulator();
    // Three opportunities that differ in stage and customer, so each filter has both
    // matching and non-matching rows to separate.
    let acme = create_opportunity(
        &router,
        "filter setup",
        "AWS",
        "filter-1",
        &json!({
            "Customer": { "Account": { "CompanyName": "Acme" } },
            "LifeCycle": { "Stage": "Prospect", "TargetCloseDate": "2027-01-31" }
        }),
    )
    .await;
    let globex = create_opportunity(
        &router,
        "filter setup",
        "AWS",
        "filter-2",
        &json!({
            "Customer": { "Account": { "CompanyName": "Globex" } },
            "LifeCycle": { "Stage": "Qualified", "TargetCloseDate": "2027-06-30" }
        }),
    )
    .await;
    let initech = create_opportunity(
        &router,
        "filter setup",
        "AWS",
        "filter-3",
        &json!({
            "Customer": { "Account": { "CompanyName": "Initech" } },
            "LifeCycle": { "Stage": "Qualified", "TargetCloseDate": "2028-02-29" }
        }),
    )
    .await;
    call(
        &router,
        "SubmitOpportunity",
        &json!({ "Catalog": "AWS", "Identifier": globex, "InvolvementType": "Co-Sell" }),
    )
    .await
    .ok("filter setup");

    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "no filter",
            json!({}),
            vec![&acme, &globex, &initech],
        ),
        (
            "Identifier, one value",
            json!({ "Identifier": [globex] }),
            vec![&globex],
        ),
        (
            "Identifier, two values",
            json!({ "Identifier": [acme, initech] }),
            vec![&acme, &initech],
        ),
        (
            "Identifier, an id in no catalog",
            json!({ "Identifier": ["O999999999"] }),
            vec![],
        ),
        (
            "Identifier, an empty list constrains nothing",
            json!({ "Identifier": [] }),
            vec![&acme, &globex, &initech],
        ),
        (
            "LifeCycleStage, one value",
            json!({ "LifeCycleStage": ["Qualified"] }),
            vec![&globex, &initech],
        ),
        (
            "LifeCycleStage, a stage nothing is in",
            json!({ "LifeCycleStage": ["Launched"] }),
            vec![],
        ),
        (
            "LifeCycleReviewStatus, the submitted one",
            json!({ "LifeCycleReviewStatus": ["Submitted"] }),
            vec![&globex],
        ),
        (
            "LifeCycleReviewStatus, the unsubmitted ones",
            json!({ "LifeCycleReviewStatus": ["Pending Submission"] }),
            vec![&acme, &initech],
        ),
        (
            "CustomerCompanyName",
            json!({ "CustomerCompanyName": ["Initech"] }),
            vec![&initech],
        ),
        (
            "TargetCloseDate, before a bound",
            json!({ "TargetCloseDate": { "BeforeTargetCloseDate": "2027-12-31" } }),
            vec![&acme, &globex],
        ),
        (
            "TargetCloseDate, after a bound",
            json!({ "TargetCloseDate": { "AfterTargetCloseDate": "2027-06-30" } }),
            vec![&globex, &initech],
        ),
        (
            "two filters together",
            json!({
                "LifeCycleStage": ["Qualified"],
                "CustomerCompanyName": ["Globex"]
            }),
            vec![&globex],
        ),
    ];

    for (case, filters, expected) in cases {
        let mut request = filters;
        request["Catalog"] = json!("AWS");
        request["MaxResults"] = json!(100);
        let listed = call(&router, "ListOpportunities", &request).await;
        listed.ok(case);
        let mut got = summaries_ids(&listed.body);
        got.sort();
        let mut want: Vec<String> = expected.into_iter().cloned().collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case}: unexpected filtered ids, body was {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn listing_sorts_by_the_requested_key() {
    let router = emulator();
    let first = create_opportunity(
        &router,
        "sort setup",
        "AWS",
        "sort-1",
        &json!({ "Customer": { "Account": { "CompanyName": "Charlie" } } }),
    )
    .await;
    separate_instants().await;
    let second = create_opportunity(
        &router,
        "sort setup",
        "AWS",
        "sort-2",
        &json!({ "Customer": { "Account": { "CompanyName": "Alpha" } } }),
    )
    .await;
    separate_instants().await;
    let third = create_opportunity(
        &router,
        "sort setup",
        "AWS",
        "sort-3",
        &json!({ "Customer": { "Account": { "CompanyName": "Bravo" } } }),
    )
    .await;
    separate_instants().await;

    // Touch the FIRST record last, so the default LastModifiedDate ordering differs
    // from the creation order. The update carries the whole payload, as the operation
    // documents, so the customer the sort key reads is unchanged.
    let version = get_opportunity(&router, "AWS", &first)
        .await
        .version("sort setup");
    call(
        &router,
        "UpdateOpportunity",
        &json!({
            "Catalog": "AWS",
            "Identifier": first,
            "LastModifiedDate": version,
            "Customer": { "Account": { "CompanyName": "Charlie" } },
            "Project": { "Title": "touched" }
        }),
    )
    .await
    .ok("sort setup");

    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "default sort is LastModifiedDate descending",
            json!({}),
            vec![&first, &third, &second],
        ),
        (
            "Identifier ascending",
            json!({ "Sort": { "SortBy": "Identifier", "SortOrder": "ASCENDING" } }),
            vec![&first, &second, &third],
        ),
        (
            "Identifier descending",
            json!({ "Sort": { "SortBy": "Identifier", "SortOrder": "DESCENDING" } }),
            vec![&third, &second, &first],
        ),
        (
            "CustomerCompanyName ascending",
            json!({ "Sort": { "SortBy": "CustomerCompanyName", "SortOrder": "ASCENDING" } }),
            vec![&second, &third, &first],
        ),
        (
            "CreatedDate ascending",
            json!({ "Sort": { "SortBy": "CreatedDate", "SortOrder": "ASCENDING" } }),
            vec![&first, &second, &third],
        ),
    ];

    for (case, sort, expected) in cases {
        let mut request = sort;
        request["Catalog"] = json!("AWS");
        request["MaxResults"] = json!(100);
        let listed = call(&router, "ListOpportunities", &request).await;
        listed.ok(case);
        let got = summaries_ids(&listed.body);
        let want: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            got, want,
            "case {case}: unexpected order, body was {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn list_summaries_report_the_stored_record() {
    let router = emulator();
    let members = json!({
        "Customer": {
            "Account": {
                "CompanyName": "Acme",
                "Industry": "Software and Internet",
                "WebsiteUrl": "https://acme.test",
                "Address": { "City": "Seattle", "CountryCode": "US", "PostalCode": "98101" }
            }
        },
        "Project": {
            "Title": "Migration",
            "DeliveryModels": ["SaaS or PaaS"],
            "CustomerBusinessProblem": BUSINESS_PROBLEM
        },
        "LifeCycle": { "Stage": "Qualified", "TargetCloseDate": "2027-06-30" },
        "OpportunityType": "Expansion",
        "PartnerOpportunityIdentifier": "crm-99"
    });
    let id = create_opportunity(&router, "summary", "AWS", "summary-1", &members).await;

    let listed = call(
        &router,
        "ListOpportunities",
        &json!({ "Catalog": "AWS", "MaxResults": 1 }),
    )
    .await;
    listed.ok("summary");
    let summary = listed
        .body
        .pointer("/OpportunitySummaries/0")
        .cloned()
        .unwrap_or(Value::Null);

    let read = get_opportunity(&router, "AWS", &id).await;
    let full = read.ok("summary").clone();

    // Every member the summary shapes declare carries the stored value.
    for member in [
        "Catalog",
        "Id",
        "Arn",
        "OpportunityType",
        "PartnerOpportunityIdentifier",
        "CreatedDate",
        "LastModifiedDate",
    ] {
        assert_eq!(
            summary.get(member),
            full.get(member),
            "case summary: {member} must match the stored record, got {summary}"
        );
    }
    for (path, expected) in [
        ("/LifeCycle/Stage", json!("Qualified")),
        ("/LifeCycle/TargetCloseDate", json!("2027-06-30")),
        ("/LifeCycle/ReviewStatus", json!("Pending Submission")),
        ("/Customer/Account/CompanyName", json!("Acme")),
        ("/Customer/Account/Industry", json!("Software and Internet")),
        ("/Customer/Account/Address/City", json!("Seattle")),
        ("/Project/Title", json!("Migration")),
    ] {
        // `ProjectSummary` declares no `Title`, so that one must be absent.
        let found = summary.pointer(path).cloned().unwrap_or(Value::Null);
        if path == "/Project/Title" {
            assert_eq!(
                found,
                Value::Null,
                "case summary: ProjectSummary declares no Title, got {summary}"
            );
        } else {
            assert_eq!(
                found, expected,
                "case summary: {path} must match the stored record, got {summary}"
            );
        }
    }
    assert_eq!(
        summary.pointer("/Project/DeliveryModels"),
        Some(&json!(["SaaS or PaaS"])),
        "case summary: ProjectSummary.DeliveryModels must match the stored record, got {summary}"
    );
}

#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let router = emulator();
    create_opportunity(&router, "range setup", "AWS", "range-1", &json!({})).await;

    // The modelled range is 1..100: each published bound is accepted and one past it
    // on either side is refused.
    for (case, max_results, expected_status) in [
        ("below the minimum", 0, 400),
        ("at the minimum", 1, 200),
        ("at the maximum", 100, 200),
        ("above the maximum", 101, 400),
    ] {
        let listed = call(
            &router,
            "ListOpportunities",
            &json!({ "Catalog": "AWS", "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            listed.status, expected_status,
            "case {case}: MaxResults {max_results} expected HTTP {expected_status}, got HTTP {} with body {}",
            listed.status, listed.body
        );
        if expected_status == 400 {
            assert_eq!(
                listed.error_code(),
                "ValidationException",
                "case {case}: expected a ValidationException, got {}",
                listed.body
            );
        }
    }
}

#[tokio::test]
async fn a_catalog_outside_the_allowlist_is_refused() {
    let router = emulator();
    let id = create_opportunity(&router, "catalog setup", "AWS", "catalog-1", &json!({})).await;
    let version = get_opportunity(&router, "AWS", &id)
        .await
        .version("catalog setup");

    // `^[a-zA-Z]+$` is all the model pins, so every row below reaches the service's
    // own allowlist check: the catalog is "AWS" or "Sandbox", case included.
    for catalog in ["Unknown", "aws", "sandbox", "AWSSandbox"] {
        let cases: Vec<(String, &str, Value)> = vec![
            (
                format!("CreateOpportunity, catalog {catalog}"),
                "CreateOpportunity",
                json!({ "Catalog": catalog, "ClientToken": "catalog-bad" }),
            ),
            (
                format!("GetOpportunity, catalog {catalog}"),
                "GetOpportunity",
                json!({ "Catalog": catalog, "Identifier": id }),
            ),
            (
                format!("UpdateOpportunity, catalog {catalog}"),
                "UpdateOpportunity",
                json!({ "Catalog": catalog, "Identifier": id, "LastModifiedDate": version }),
            ),
            (
                format!("AssignOpportunity, catalog {catalog}"),
                "AssignOpportunity",
                json!({
                    "Catalog": catalog,
                    "Identifier": id,
                    "Assignee": {
                        "Email": "rep@partner.test",
                        "FirstName": "Rep",
                        "LastName": "One",
                        "BusinessTitle": "OpportunityOwner"
                    }
                }),
            ),
            (
                format!("SubmitOpportunity, catalog {catalog}"),
                "SubmitOpportunity",
                json!({ "Catalog": catalog, "Identifier": id, "InvolvementType": "Co-Sell" }),
            ),
            (
                format!("ListOpportunities, catalog {catalog}"),
                "ListOpportunities",
                json!({ "Catalog": catalog }),
            ),
        ];
        for (case, operation, body) in cases {
            call(&router, operation, &body)
                .await
                .expect_error(&case, 400, "ValidationException");
        }
    }

    // And the opportunity the refused calls named is untouched.
    let read = get_opportunity(&router, "AWS", &id).await;
    assert_eq!(
        read.version("catalog setup"),
        version,
        "a refused request must not change the record, got {}",
        read.body
    );
}

#[tokio::test]
async fn the_store_and_the_idempotency_ledger_stay_bounded() {
    let (router, engine) = emulator_with_engine();

    // Past the ledger's cap the OLDEST token is forgotten, while every record it
    // created stays: the ledger is bounded by its own rule, not by the store's.
    let overshoot = MAX_CREATE_REPLAYS + 2;
    for index in 0..overshoot {
        let case = format!("create {index}");
        call(
            &router,
            "CreateOpportunity",
            &create_body("AWS", &format!("bound-{index}"), &json!({})),
        )
        .await
        .ok(&case);
    }
    {
        let store = service_state(&*engine).expect("the service store is reachable");
        let store = store.lock();
        assert_eq!(
            store.create_replays.len(),
            MAX_CREATE_REPLAYS,
            "the idempotency ledger must stay at its cap of {MAX_CREATE_REPLAYS}"
        );
        assert_eq!(
            store.opportunities.len(),
            overshoot,
            "evicting a token must not drop the opportunity it created"
        );
    }

    // Replaying a token the ledger has forgotten mints a new opportunity, exactly as
    // a token whose idempotency window has passed would.
    let replayed = call(
        &router,
        "CreateOpportunity",
        &create_body("AWS", "bound-0", &json!({})),
    )
    .await;
    replayed.ok("evicted token");
    assert_eq!(
        replayed.field("evicted token", "Id"),
        format!("O{}", overshoot + 1),
        "a forgotten token must mint the next opportunity"
    );

    // And the store's own ceiling refuses a create past it, with the quota-shaped
    // modelled error, instead of growing.
    let mut created = overshoot + 1;
    while created < MAX_OPPORTUNITIES {
        let case = format!("fill {created}");
        call(
            &router,
            "CreateOpportunity",
            &create_body("AWS", &format!("fill-{created}"), &json!({})),
        )
        .await
        .ok(&case);
        created += 1;
    }
    call(
        &router,
        "CreateOpportunity",
        &create_body("AWS", "one-too-many", &json!({})),
    )
    .await
    .expect_error("past the ceiling", 429, "ThrottlingException");

    let store = service_state(&*engine).expect("the service store is reachable");
    let store = store.lock();
    assert_eq!(
        store.opportunities.len(),
        MAX_OPPORTUNITIES,
        "the store must stay at its ceiling of {MAX_OPPORTUNITIES}"
    );
    assert!(
        store.create_replays.len() <= MAX_CREATE_REPLAYS,
        "the idempotency ledger must stay within its cap, got {}",
        store.create_replays.len()
    );
}

/// End-to-end tests for the CustomerAccount lifecycle, over the wire harness above:
/// `CreateCustomerAccount`, `GetCustomerAccount`, `UpdateCustomerAccount`,
/// `DeleteCustomerAccount`, `SearchCustomerAccounts` and
/// `GetCustomerAccountStatistics`.
///
/// Each test is a TABLE over the class its rule covers: the rows differ in catalog, in
/// which optional members a request carries, in whether the customer is identified by a
/// DUNS number, in filter shape, in sort key and direction, and in page size.
///
/// The asynchronous matching step is waited for by POLLING the account until it leaves
/// `PENDING`, never by sleeping for its span: the span is the service's, and a test that
/// assumed it would fail on a loaded host.
mod customer_accounts {
    use std::time::{Duration, Instant};

    use service_com_amazonaws_partnercentralselling_awspartnercentralselling::ops::search_customer_accounts::{
        MAX_FILTER_DEPTH, MAX_FILTER_NODES,
    };
    use service_com_amazonaws_partnercentralselling_awspartnercentralselling::state::{
        MAX_CUSTOMER_ACCOUNTS, MAX_SEARCH_MAX_RESULTS,
    };

    use super::*;

    /// How long a poll waits for a matching outcome before giving up. Generous on
    /// purpose: it bounds a hang, it does not measure the service's span.
    const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

    /// How often a poll looks again.
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    /// The `EnrichmentStatus` a new account starts in.
    const PENDING: &str = "PENDING";

    /// The outcome of matching an account whose customer no other account maps.
    const NO_MATCH_FOUND: &str = "NO_MATCH_FOUND";

    /// The outcome of matching an account whose customer another account already maps.
    const ACTION_REQUIRED: &str = "ACTION_REQUIRED";

    /// The two instants the date-range rows compare against, as the epoch seconds a
    /// `DateValue` travels as on this protocol: the epoch itself, and an instant no
    /// account created by a test run can reach.
    const EPOCH: i64 = 0;
    const FAR_FUTURE: i64 = 32_503_680_000;

    /// One row of the missing-group table: the case, whether a group for the accounts
    /// with no value is asked for, and the (value, count) groups the answer must carry.
    type MissingGroupRow = (&'static str, Option<bool>, Vec<(String, i64)>);

    /// Create one customer account and hand back the whole answered call.
    async fn create(router: &MultiServiceRouter, catalog: &str, members: &Value) -> Call {
        let mut body = members.clone();
        body["Catalog"] = json!(catalog);
        call(router, "CreateCustomerAccount", &body).await
    }

    /// Create one customer account that must succeed, and hand back its id.
    async fn create_ok(
        router: &MultiServiceRouter,
        case: &str,
        catalog: &str,
        members: &Value,
    ) -> String {
        let created = create(router, catalog, members).await;
        created.ok(case);
        created.field(case, "Id")
    }

    /// Read one customer account back, by id or by ARN.
    async fn get(router: &MultiServiceRouter, catalog: &str, identifier: &str) -> Call {
        call(
            router,
            "GetCustomerAccount",
            &json!({ "Catalog": catalog, "Identifier": identifier }),
        )
        .await
    }

    /// Run one search.
    async fn search(router: &MultiServiceRouter, body: &Value) -> Call {
        call(router, "SearchCustomerAccounts", body).await
    }

    /// An account body whose only customer detail is the required company name.
    fn named(company: &str) -> Value {
        json!({ "Account": { "CompanyName": company } })
    }

    /// An account body naming the company and its DUNS number — the value that makes
    /// two accounts describe the same customer.
    fn named_with_duns(company: &str, duns: &str) -> Value {
        json!({ "Account": { "CompanyName": company, "Duns": duns } })
    }

    /// Poll `identifier` until its `EnrichmentStatus` is no longer `PENDING`, and hand
    /// back the settled body.
    async fn settled(
        router: &MultiServiceRouter,
        case: &str,
        catalog: &str,
        identifier: &str,
    ) -> Value {
        poll_until(router, case, catalog, identifier, |status| status != PENDING).await
    }

    /// Poll `identifier` until its `EnrichmentStatus` is exactly `wanted`.
    async fn settled_as(
        router: &MultiServiceRouter,
        case: &str,
        catalog: &str,
        identifier: &str,
        wanted: &str,
    ) -> Value {
        poll_until(router, case, catalog, identifier, |status| status == wanted).await
    }

    /// Look at the account again until its status satisfies `holds`, or the deadline
    /// passes.
    async fn poll_until(
        router: &MultiServiceRouter,
        case: &str,
        catalog: &str,
        identifier: &str,
        holds: impl Fn(&str) -> bool,
    ) -> Value {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let read = get(router, catalog, identifier).await;
            read.ok(case);
            let status = read.field(case, "EnrichmentStatus");
            if holds(&status) {
                return read.body;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the account never reached the status this case waits \
                 for; last read was {}",
                read.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The modelled `CustomerAccountId` pattern, `^cust-acc-[0-9a-z]{13}$`.
    fn is_customer_account_id(value: &str) -> bool {
        let Some(tail) = value.strip_prefix("cust-acc-") else {
            return false;
        };
        tail.len() == 13
            && tail
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }

    /// Assert `arn` is the modelled `CustomerAccountArn` of `id` in `catalog`:
    /// `^arn:[a-z\-]*:partnercentral:[a-z0-9\-]*:[0-9]{12}:catalog/[a-zA-Z]+/customer-account/cust-acc-[0-9a-z]{13}$`.
    fn assert_customer_account_arn(case: &str, arn: &str, catalog: &str, id: &str) {
        let segments: Vec<&str> = arn.splitn(6, ':').collect();
        assert_eq!(
            segments.len(),
            6,
            "case {case:?}: expected a six-segment ARN, got {arn:?}"
        );
        assert_eq!(
            (segments[0], segments[2], segments[3]),
            ("arn", "partnercentral", REGION),
            "case {case:?}: expected an arn:*:partnercentral:{REGION} ARN, got {arn:?}"
        );
        assert!(
            segments[4].len() == 12 && segments[4].bytes().all(|byte| byte.is_ascii_digit()),
            "case {case:?}: expected a 12-digit account segment, got {arn:?}"
        );
        assert_eq!(
            segments[5],
            format!("catalog/{catalog}/customer-account/{id}"),
            "case {case:?}: expected the catalog and account in the resource path, got \
             {arn:?}"
        );
    }

    /// The ids a search answered, in the order it answered them.
    fn searched_ids(page: &Value) -> Vec<String> {
        summaries(page, |summary| summary.get("Id"))
    }

    /// The company names a search answered, in order.
    fn searched_companies(page: &Value) -> Vec<String> {
        summaries(page, |summary| {
            summary
                .get("CustomerAccount")
                .and_then(|details| details.get("CompanyName"))
        })
    }

    /// One string member of each summary a search answered, in order.
    fn summaries(page: &Value, member: impl Fn(&Value) -> Option<&Value>) -> Vec<String> {
        page.get("CustomerAccountSummaries")
            .and_then(Value::as_array)
            .map(|summaries| {
                summaries
                    .iter()
                    .map(|summary| {
                        member(summary)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The (value, count) pairs of one aggregation result, in the order it reported them.
    fn buckets_of(aggregation: &Value) -> Vec<(String, i64)> {
        aggregation
            .get("Buckets")
            .and_then(Value::as_array)
            .map(|buckets| {
                buckets
                    .iter()
                    .map(|bucket| {
                        let value = bucket
                            .get("Values")
                            .and_then(Value::as_array)
                            .and_then(|values| values.first())
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let count = bucket.get("Count").and_then(Value::as_i64).unwrap_or(-1);
                        (value, count)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Seed one catalog with accounts a search and a statistics call select over, and
    /// hand back (company name, id) for each, settled.
    async fn seeded(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
        // (company name, DUNS) — "Delta" shares "Alpha"'s customer, so exactly one of
        // the four settles as a duplicate; "Epsilon" lives in the other catalog.
        let rows: [(&'static str, Option<&str>); 4] = [
            ("Alpha Analytics", Some("100000001")),
            ("Beta Builders", Some("100000002")),
            ("Gamma Goods", None),
            ("Delta Data", Some("100000001")),
        ];
        let mut seeded = Vec::new();
        for (company, duns) in rows {
            let members = match duns {
                Some(duns) => named_with_duns(company, duns),
                None => named(company),
            };
            let id = create_ok(router, company, "AWS", &members).await;
            seeded.push((company, id));
        }
        create_ok(router, "Epsilon Export", "Sandbox", &named("Epsilon Export")).await;

        for (company, id) in &seeded {
            settled(router, company, "AWS", id).await;
        }
        seeded
    }

    /// Whatever a create carried comes back from the read, with the service-owned
    /// members the model declares: a minted id and ARN of the modelled shape, the
    /// `PENDING` status matching starts in, and the dates.
    #[tokio::test]
    async fn created_customer_accounts_come_back_from_get() {
        let router = emulator();

        // (case, catalog, request members)
        let rows: Vec<(&str, &str, Value)> = vec![
            (
                "only the required company name, AWS catalog",
                "AWS",
                named("Required Only Corp"),
            ),
            (
                "every customer detail and a partner identifier, Sandbox catalog",
                "Sandbox",
                json!({
                    "Account": {
                        "CompanyName": "Full Details GmbH",
                        "WebsiteUrl": "https://full-details.example",
                        "Duns": "123456789",
                        "Domain": "full-details.example",
                        "Industry": "Software and Internet",
                        "ParentCompanyName": "Full Details Holding",
                        "Address": {
                            "City": "Berlin",
                            "PostalCode": "10115",
                            "StateOrRegion": "Berlin",
                            "CountryCode": "DE",
                            "StreetAddress": "Chausseestrasse 1"
                        }
                    },
                    "PartnerAccountIdentifier": "crm-00042"
                }),
            ),
            (
                "an industry described by OtherIndustry, and two tags",
                "AWS",
                json!({
                    "Account": {
                        "CompanyName": "Tagged Ltd",
                        "Industry": "Other",
                        "OtherIndustry": "Artisanal coffee roasting"
                    },
                    "Tags": [
                        { "Key": "team", "Value": "alliances" },
                        { "Key": "tier", "Value": "silver" }
                    ]
                }),
            ),
        ];

        for (case, catalog, members) in rows {
            let created = create(&router, catalog, &members).await;
            created.ok(case);
            let id = created.field(case, "Id");
            assert!(
                is_customer_account_id(&id),
                "case {case:?}: expected a ^cust-acc-[0-9a-z]{{13}}$ id, got {id:?}"
            );
            assert_customer_account_arn(case, &created.field(case, "Arn"), catalog, &id);
            assert_eq!(
                created.field(case, "Status"),
                PENDING,
                "case {case:?}: a new account starts as PENDING, got {}",
                created.body
            );
            assert_eq!(
                created.field(case, "Catalog"),
                catalog,
                "case {case:?}: the answer is about the catalog asked for, got {}",
                created.body
            );
            assert!(
                created.body.get("CreatedDate").is_some(),
                "case {case:?}: the create reports when the account was created, got {}",
                created.body
            );

            let read = get(&router, catalog, &id).await;
            read.ok(case);
            assert_echoed(case, "", &members, &read.body);
            assert_eq!(
                read.field(case, "Id"),
                id,
                "case {case:?}: the read answers the account asked for, got {}",
                read.body
            );
            assert_customer_account_arn(case, &read.field(case, "Arn"), catalog, &id);
            assert!(
                [PENDING, NO_MATCH_FOUND, ACTION_REQUIRED]
                    .contains(&read.field(case, "EnrichmentStatus").as_str()),
                "case {case:?}: the read reports a modelled EnrichmentStatus, got {}",
                read.body
            );
            assert_eq!(
                read.body.get("CreatedDate"),
                created.body.get("CreatedDate"),
                "case {case:?}: the read reports the create's own CreatedDate, got {}",
                read.body
            );
            assert!(
                read.body.get("LastModifiedDate").is_some(),
                "case {case:?}: the read reports when the account was last modified, got \
                 {}",
                read.body
            );
            // The modelled tag list is `len 1..200`, so an account with no tags reports
            // no list at all rather than an empty one.
            assert_eq!(
                read.body.get("Tags"),
                members.get("Tags"),
                "case {case:?}: the read reports the tags the create carried, got {}",
                read.body
            );

            // The same record, addressed by its ARN: "Specifies the ARN or the
            // identifier of the customer account."
            let arn = created.field(case, "Arn");
            let by_arn = get(&router, catalog, &arn).await;
            by_arn.ok(case);
            for member in ["Id", "Arn", "Catalog", "Account", "CreatedDate"] {
                assert_eq!(
                    by_arn.body.get(member),
                    read.body.get(member),
                    "case {case:?}: reading by ARN answers the same account's {member}, \
                     got {}",
                    by_arn.body
                );
            }

            // And the catalog is part of the identity: the other catalog holds no such
            // account.
            let other_catalog = if catalog == "AWS" { "Sandbox" } else { "AWS" };
            get(&router, other_catalog, &id)
                .await
                .expect_error(case, 404, "ResourceNotFoundException");
        }

        // An identifier no account was ever minted for, and a catalog outside the
        // documented list, are each refused in their own way.
        get(&router, "AWS", "cust-acc-0000000000000")
            .await
            .expect_error("an id never minted", 404, "ResourceNotFoundException");
        get(&router, "Production", "cust-acc-0000000000000")
            .await
            .expect_error(
                "a catalog outside AWS and Sandbox",
                400,
                "ValidationException",
            );
    }

    /// One `ClientToken` creates one account: the same token with the same body answers
    /// the account the first call minted, and the same token with a different body is a
    /// conflict.
    #[tokio::test]
    async fn one_client_token_creates_one_customer_account() {
        let router = emulator();

        // (case, catalog, members)
        let rows: Vec<(&str, &str, Value)> = vec![
            ("the required members only", "AWS", named("Token Corp")),
            (
                "a fuller body, in the other catalog",
                "Sandbox",
                json!({
                    "Account": { "CompanyName": "Token GmbH", "Duns": "222333444" },
                    "PartnerAccountIdentifier": "crm-7"
                }),
            ),
        ];

        for (case, catalog, members) in rows {
            let mut first_body = members.clone();
            first_body["ClientToken"] = json!(format!("token-{catalog}"));
            let first = create(&router, catalog, &first_body).await;
            first.ok(case);

            let replay = create(&router, catalog, &first_body).await;
            replay.ok(case);
            assert_eq!(
                replay.field(case, "Id"),
                first.field(case, "Id"),
                "case {case:?}: replaying a token answers the account it minted, got {}",
                replay.body
            );
            assert_eq!(
                replay.body.get("CreatedDate"),
                first.body.get("CreatedDate"),
                "case {case:?}: a replay mints nothing, so the creation instant is the \
                 first one, got {}",
                replay.body
            );

            let mut changed = first_body.clone();
            changed["Account"]["CompanyName"] = json!("Another Company");
            create(&router, catalog, &changed)
                .await
                .expect_error(case, 409, "ConflictException");

            // A different token creates a second account for the same details.
            let mut second_body = members.clone();
            second_body["ClientToken"] = json!(format!("token-{catalog}-2"));
            let second = create(&router, catalog, &second_body).await;
            second.ok(case);
            assert_ne!(
                second.field(case, "Id"),
                first.field(case, "Id"),
                "case {case:?}: another token mints another account, got {}",
                second.body
            );
        }
    }

    /// Matching runs asynchronously and every pending account reaches an outcome: an
    /// account whose customer no other account maps settles as `NO_MATCH_FOUND`, and one
    /// whose customer an earlier account already maps settles as `ACTION_REQUIRED`,
    /// naming that account. Deleting the account that held the mapping makes the
    /// duplicate match again.
    #[tokio::test]
    async fn matching_settles_every_pending_account_and_names_the_duplicate() {
        let router = emulator();

        // The first account to describe the customer with this DUNS number.
        let first = create(&router, "AWS", &named_with_duns("First Mapper", "111222333")).await;
        first.ok("the first account for a customer");
        let first_id = first.field("the first account for a customer", "Id");
        let first_arn = first.field("the first account for a customer", "Arn");

        // (case, catalog, members, expected settled status)
        let rows: Vec<(&str, &str, Value, &str)> = vec![
            (
                "a second account for the same customer",
                "AWS",
                named_with_duns("Second Mapper", "111222333"),
                ACTION_REQUIRED,
            ),
            (
                "an account identified by another DUNS number",
                "AWS",
                named_with_duns("Other Customer", "999888777"),
                NO_MATCH_FOUND,
            ),
            (
                "an account with no DUNS number at all",
                "AWS",
                named("Unidentified Customer"),
                NO_MATCH_FOUND,
            ),
            (
                "the same DUNS number in the other catalog",
                "Sandbox",
                named_with_duns("Sandbox Mapper", "111222333"),
                NO_MATCH_FOUND,
            ),
        ];

        // Created first, waited for after, so the spans overlap instead of adding up.
        let mut created = Vec::new();
        for (case, catalog, members, expected) in &rows {
            let id = create_ok(&router, case, catalog, members).await;
            created.push((*case, *catalog, id, *expected));
        }

        let first_settled =
            settled(&router, "the first account for a customer", "AWS", &first_id).await;
        assert_eq!(
            first_settled
                .get("EnrichmentStatus")
                .and_then(Value::as_str),
            Some(NO_MATCH_FOUND),
            "case \"the first account for a customer\": the account that maps the \
             customer is nobody's duplicate, got {first_settled}"
        );
        assert_eq!(
            first_settled.get("ReasonCode"),
            None,
            "case \"the first account for a customer\": a settled account with no \
             problem reports no reason code, got {first_settled}"
        );

        let mut duplicate_id = String::new();
        for (case, catalog, id, expected) in created {
            let body = settled_as(&router, case, catalog, &id, expected).await;
            if expected == ACTION_REQUIRED {
                duplicate_id = id.clone();
                assert_eq!(
                    body.get("ReasonCode").and_then(Value::as_str),
                    Some("DUPLICATE_CUSTOMER_MATCH"),
                    "case {case:?}: a duplicate names the reason, got {body}"
                );
                assert_eq!(
                    body.get("ExistingCustomerAccountArn")
                        .and_then(Value::as_str),
                    Some(first_arn.as_str()),
                    "case {case:?}: a duplicate names the account that already maps the \
                     customer, got {body}"
                );
                assert!(
                    body.get("Message")
                        .and_then(Value::as_str)
                        .is_some_and(|message| message.contains(&first_id)),
                    "case {case:?}: the message describes the problem and names that \
                     account, got {body}"
                );
            } else {
                assert_eq!(
                    body.get("ExistingCustomerAccountArn"),
                    None,
                    "case {case:?}: an account that duplicates nothing names no other \
                     account, got {body}"
                );
                assert_eq!(
                    body.get("AwsEnrichment"),
                    None,
                    "case {case:?}: enrichment is reported only for a matched customer, \
                     got {body}"
                );
            }
        }

        // The duplicate's outcome rests on the account that holds the mapping. Delete
        // that account and the duplicate is matched again, this time as the only
        // account describing its customer.
        call(
            &router,
            "DeleteCustomerAccount",
            &json!({ "Catalog": "AWS", "Identifier": first_id }),
        )
        .await
        .ok("deleting the account that held the mapping");
        let rematched = settled_as(
            &router,
            "the duplicate after the mapping is deleted",
            "AWS",
            &duplicate_id,
            NO_MATCH_FOUND,
        )
        .await;
        assert_eq!(
            rematched.get("ReasonCode"),
            None,
            "case \"the duplicate after the mapping is deleted\": the duplicate reason \
             is dropped with the account it named, got {rematched}"
        );
        assert_eq!(
            rematched.get("ExistingCustomerAccountArn"),
            None,
            "case \"the duplicate after the mapping is deleted\": the deleted account is \
             no longer named, got {rematched}"
        );
    }

    /// An update replaces the customer details whole, moves the record's version,
    /// matches the account again, and refuses a version other than the stored one.
    #[tokio::test]
    async fn update_replaces_the_details_rematches_and_refuses_a_stale_version() {
        let router = emulator();

        // (case, catalog, whether the update addresses the account by its ARN)
        let rows: Vec<(&str, &str, bool)> = vec![
            ("addressed by its id, AWS catalog", "AWS", false),
            ("addressed by its ARN, Sandbox catalog", "Sandbox", true),
        ];

        for (case, catalog, by_arn) in rows {
            let before = create(
                &router,
                catalog,
                &json!({
                    "Account": {
                        "CompanyName": "Before Update Inc",
                        "WebsiteUrl": "https://before.example",
                        "Duns": "333444555"
                    },
                    "PartnerAccountIdentifier": "crm-before"
                }),
            )
            .await;
            before.ok(case);
            let id = before.field(case, "Id");
            let identifier = if by_arn {
                before.field(case, "Arn")
            } else {
                id.clone()
            };

            // The version a caller must echo is the one the read reports.
            let read = get(&router, catalog, &identifier).await;
            read.ok(case);
            let version = read.version(case);

            // "Use UpdateCustomerAccount to replace the customer information": the
            // details the request omits are gone from the record, not merged with the
            // stored ones.
            let updated = call(
                &router,
                "UpdateCustomerAccount",
                &json!({
                    "Catalog": catalog,
                    "Identifier": identifier,
                    "Account": { "CompanyName": "After Update Inc", "Industry": "Healthcare" },
                    "LastModifiedDate": version
                }),
            )
            .await;
            updated.ok(case);
            assert_eq!(
                updated.field(case, "Id"),
                id,
                "case {case:?}: the update answers the account addressed, got {}",
                updated.body
            );
            assert_eq!(
                updated.field(case, "Status"),
                PENDING,
                "case {case:?}: an update matches the account again, so it is pending \
                 once more, got {}",
                updated.body
            );
            assert_ne!(
                updated.body.get("LastModifiedDate"),
                Some(&version),
                "case {case:?}: an update moves the record's version, got {}",
                updated.body
            );
            assert_eq!(
                updated.body.get("CreatedDate"),
                before.body.get("CreatedDate"),
                "case {case:?}: an update does not move the creation instant, got {}",
                updated.body
            );

            let after = get(&router, catalog, &id).await;
            after.ok(case);
            assert_echoed(
                case,
                "",
                &json!({
                    "Account": { "CompanyName": "After Update Inc", "Industry": "Healthcare" }
                }),
                &after.body,
            );
            for dropped in ["WebsiteUrl", "Duns"] {
                assert_eq!(
                    after
                        .body
                        .get("Account")
                        .and_then(|account| account.get(dropped)),
                    None,
                    "case {case:?}: a detail the update omitted is gone from the record, \
                     got {}",
                    after.body
                );
            }
            assert_eq!(
                after.body.get("PartnerAccountIdentifier"),
                None,
                "case {case:?}: the partner identifier the update omitted is gone too, \
                 got {}",
                after.body
            );

            // The version the first read reported is now stale: "If the account changed
            // since then, the request fails with a ConflictException."
            call(
                &router,
                "UpdateCustomerAccount",
                &json!({
                    "Catalog": catalog,
                    "Identifier": identifier,
                    "Account": { "CompanyName": "Stale Update Inc" },
                    "LastModifiedDate": version
                }),
            )
            .await
            .expect_error(case, 409, "ConflictException");

            // And an account the catalog does not hold is missing, not stale.
            call(
                &router,
                "UpdateCustomerAccount",
                &json!({
                    "Catalog": catalog,
                    "Identifier": "cust-acc-0000000000000",
                    "Account": { "CompanyName": "Nowhere Inc" },
                    "LastModifiedDate": version
                }),
            )
            .await
            .expect_error(case, 404, "ResourceNotFoundException");
        }
    }

    /// An update that gives the account the customer of an existing one makes it that
    /// account's duplicate, which is the matching rule applied to the new details.
    #[tokio::test]
    async fn an_update_onto_another_customers_duns_settles_as_a_duplicate() {
        let router = emulator();

        let holder = create(&router, "AWS", &named_with_duns("Mapping Holder", "555666777")).await;
        holder.ok("the account that maps the customer");
        let holder_arn = holder.field("the account that maps the customer", "Arn");

        let moving_id = create_ok(
            &router,
            "the account that moves onto that customer",
            "AWS",
            &named("Moving Account"),
        )
        .await;
        let settled_before = settled(
            &router,
            "the account that moves onto that customer",
            "AWS",
            &moving_id,
        )
        .await;
        assert_eq!(
            settled_before
                .get("EnrichmentStatus")
                .and_then(Value::as_str),
            Some(NO_MATCH_FOUND),
            "case \"the account that moves onto that customer\": it starts as nobody's \
             duplicate, got {settled_before}"
        );

        let version = settled_before
            .get("LastModifiedDate")
            .cloned()
            .unwrap_or(Value::Null);
        call(
            &router,
            "UpdateCustomerAccount",
            &json!({
                "Catalog": "AWS",
                "Identifier": moving_id,
                "Account": { "CompanyName": "Moving Account", "Duns": "555666777" },
                "LastModifiedDate": version
            }),
        )
        .await
        .ok("the update onto another customer");

        let after = settled_as(
            &router,
            "the update onto another customer",
            "AWS",
            &moving_id,
            ACTION_REQUIRED,
        )
        .await;
        assert_eq!(
            after
                .get("ExistingCustomerAccountArn")
                .and_then(Value::as_str),
            Some(holder_arn.as_str()),
            "case \"the update onto another customer\": the duplicate names the account \
             that maps the customer, got {after}"
        );
    }

    /// Deleting a customer account removes it and succeeds whether or not it was there:
    /// "This operation is idempotent. Deleting a customer account that was already
    /// deleted succeeds."
    #[tokio::test]
    async fn deleting_a_customer_account_is_idempotent() {
        let router = emulator();

        // (case, catalog, whether the delete addresses the account by its ARN)
        let rows: Vec<(&str, &str, bool)> = vec![
            ("deleted by its id, AWS catalog", "AWS", false),
            ("deleted by its ARN, Sandbox catalog", "Sandbox", true),
        ];

        for (case, catalog, by_arn) in rows {
            let created = create(&router, catalog, &named("Deletable Inc")).await;
            created.ok(case);
            let id = created.field(case, "Id");
            let identifier = if by_arn {
                created.field(case, "Arn")
            } else {
                id.clone()
            };

            call(
                &router,
                "DeleteCustomerAccount",
                &json!({ "Catalog": catalog, "Identifier": identifier }),
            )
            .await
            .ok(case);
            get(&router, catalog, &id)
                .await
                .expect_error(case, 404, "ResourceNotFoundException");

            // Again, and for an account that never existed: both succeed.
            for (repeat, target) in [
                ("the same account twice", identifier.clone()),
                (
                    "an account never minted",
                    "cust-acc-0000000000000".to_string(),
                ),
            ] {
                let again = call(
                    &router,
                    "DeleteCustomerAccount",
                    &json!({ "Catalog": catalog, "Identifier": target }),
                )
                .await;
                assert_eq!(
                    again.status, 200,
                    "case {case:?} / {repeat:?}: deleting what is not there succeeds, got \
                     HTTP {} with body {}",
                    again.status, again.body
                );
            }
        }

        // A catalog outside the documented list is still refused.
        call(
            &router,
            "DeleteCustomerAccount",
            &json!({ "Catalog": "Production", "Identifier": "cust-acc-0000000000000" }),
        )
        .await
        .expect_error(
            "a catalog outside AWS and Sandbox",
            400,
            "ValidationException",
        );
    }

    /// A search selects the accounts of one catalog that satisfy the filter, orders them
    /// as `Sort` asks, and pages the result without repeating or skipping an account.
    #[tokio::test]
    async fn search_filters_sorts_and_pages_customer_accounts() {
        let router = emulator();
        let seeded = seeded(&router).await;
        let id_of = |company: &str| {
            seeded
                .iter()
                .find(|(name, _)| *name == company)
                .map(|(_, id)| id.clone())
                .unwrap_or_default()
        };

        // (case, filter, the companies that satisfy it)
        let rows: Vec<(&str, Value, Vec<&str>)> = vec![
            (
                "no filter at all",
                Value::Null,
                vec![
                    "Alpha Analytics",
                    "Beta Builders",
                    "Delta Data",
                    "Gamma Goods",
                ],
            ),
            (
                "EqualsTo on the company name",
                json!({ "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Gamma Goods" } } }),
                vec!["Gamma Goods"],
            ),
            (
                "ContainsAny over two company names",
                json!({ "ContainsAny": { "Key": "COMPANY_NAME", "Value": { "StringListValue": ["Alpha Analytics", "Beta Builders"] } } }),
                vec!["Alpha Analytics", "Beta Builders"],
            ),
            (
                "NotFilter around a company name",
                json!({ "NotFilter": { "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Alpha Analytics" } } } }),
                vec!["Beta Builders", "Delta Data", "Gamma Goods"],
            ),
            (
                "EqualsTo on the settled enrichment status",
                json!({ "EqualsTo": { "Key": "ENRICHMENT_STATUS", "Value": { "StringValue": NO_MATCH_FOUND } } }),
                vec!["Alpha Analytics", "Beta Builders", "Gamma Goods"],
            ),
            (
                "EqualsTo on the status of the one duplicate",
                json!({ "EqualsTo": { "Key": "ENRICHMENT_STATUS", "Value": { "StringValue": ACTION_REQUIRED } } }),
                vec!["Delta Data"],
            ),
            (
                "AndAllFilters over a status and a name",
                json!({ "AndAllFilters": [
                    { "EqualsTo": { "Key": "ENRICHMENT_STATUS", "Value": { "StringValue": NO_MATCH_FOUND } } },
                    { "NotFilter": { "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Alpha Analytics" } } } }
                ] }),
                vec!["Beta Builders", "Gamma Goods"],
            ),
            (
                "OrAllFilters over two names",
                json!({ "OrAllFilters": [
                    { "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Delta Data" } } },
                    { "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Gamma Goods" } } }
                ] }),
                vec!["Delta Data", "Gamma Goods"],
            ),
            (
                "a creation-date range that covers every account",
                json!({ "AndAllFilters": [
                    { "GreaterThanOrEquals": { "Key": "CREATED_DATE", "Value": { "DateValue": EPOCH } } },
                    { "LessThan": { "Key": "CREATED_DATE", "Value": { "DateValue": FAR_FUTURE } } }
                ] }),
                vec![
                    "Alpha Analytics",
                    "Beta Builders",
                    "Delta Data",
                    "Gamma Goods",
                ],
            ),
            (
                "a creation-date range that covers none",
                json!({ "LessThan": { "Key": "CREATED_DATE", "Value": { "DateValue": EPOCH } } }),
                vec![],
            ),
            (
                "a field only an enriched account carries",
                json!({ "EqualsTo": { "Key": "AWS_INDUSTRY", "Value": { "StringValue": "Software and Internet" } } }),
                vec![],
            ),
            (
                "ContainsAll over the programs an enriched account is eligible for",
                json!({ "ContainsAll": { "Key": "PROGRAM_ELIGIBILITY", "Value": { "StringListValue": ["ISV Accelerate"] } } }),
                vec![],
            ),
        ];

        for (case, filter, expected) in rows {
            let mut body = json!({
                "Catalog": "AWS",
                "Sort": { "SortBy": "COMPANY_NAME", "SortOrder": "ASCENDING" }
            });
            if !filter.is_null() {
                body["Filter"] = filter;
            }
            let page = search(&router, &body).await;
            let companies = searched_companies(page.ok(case));
            assert_eq!(
                companies,
                expected
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
                "case {case:?}: expected the accounts {expected:?}, got {}",
                page.body
            );
            assert_eq!(
                page.body.get("NextToken"),
                None,
                "case {case:?}: a page that holds every match carries no continuation, \
                 got {}",
                page.body
            );
        }

        // The other catalog is a separate set of accounts.
        let sandbox = search(&router, &json!({ "Catalog": "Sandbox" })).await;
        assert_eq!(
            searched_companies(sandbox.ok("the other catalog")),
            vec!["Epsilon Export".to_string()],
            "case \"the other catalog\": a search answers only its own catalog, got {}",
            sandbox.body
        );

        // A summary carries the reduced customer details, "omitting sensitive fields
        // such as the Data Universal Numbering System (DUNS) number and the street
        // address".
        let summary = sandbox
            .body
            .get("CustomerAccountSummaries")
            .and_then(Value::as_array)
            .and_then(|summaries| summaries.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            summary
                .get("CustomerAccount")
                .and_then(Value::as_object)
                .map(|details| details.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["CompanyName".to_string()]),
            "case \"the other catalog\": the summary carries the company name and \
             nothing else of the customer, got {summary}"
        );

        // (case, sort, the companies in the order it asks for)
        let orders: Vec<(&str, Value, Vec<&str>)> = vec![
            (
                "by company name, ascending",
                json!({ "SortBy": "COMPANY_NAME", "SortOrder": "ASCENDING" }),
                vec![
                    "Alpha Analytics",
                    "Beta Builders",
                    "Delta Data",
                    "Gamma Goods",
                ],
            ),
            (
                "by company name, descending",
                json!({ "SortBy": "COMPANY_NAME", "SortOrder": "DESCENDING" }),
                vec![
                    "Gamma Goods",
                    "Delta Data",
                    "Beta Builders",
                    "Alpha Analytics",
                ],
            ),
            (
                "by creation date, ascending",
                json!({ "SortBy": "CREATED_DATE", "SortOrder": "ASCENDING" }),
                vec![
                    "Alpha Analytics",
                    "Beta Builders",
                    "Gamma Goods",
                    "Delta Data",
                ],
            ),
            (
                "by creation date, descending",
                json!({ "SortBy": "CREATED_DATE", "SortOrder": "DESCENDING" }),
                vec![
                    "Delta Data",
                    "Gamma Goods",
                    "Beta Builders",
                    "Alpha Analytics",
                ],
            ),
        ];
        for (case, sort, expected) in orders {
            let page = search(&router, &json!({ "Catalog": "AWS", "Sort": sort })).await;
            assert_eq!(
                searched_companies(page.ok(case)),
                expected
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
                "case {case:?}: expected {expected:?}, got {}",
                page.body
            );
        }

        // A score nothing computed leaves every account in the result, in the total
        // order the id tie-break gives: "accounts that have no score last" is every one
        // of them.
        let scored = search(
            &router,
            &json!({
                "Catalog": "AWS",
                "Sort": { "SortBy": "MARKETPLACE_ENGAGEMENT_SCORE", "SortOrder": "DESCENDING" }
            }),
        )
        .await;
        let mut scored_ids = searched_ids(scored.ok("sorted by a score no account carries"));
        scored_ids.sort();
        let mut every_id: Vec<String> = seeded.iter().map(|(_, id)| id.clone()).collect();
        every_id.sort();
        assert_eq!(
            scored_ids, every_id,
            "case \"sorted by a score no account carries\": every account is still \
             reported, got {}",
            scored.body
        );

        // (case, page size, the page sizes the walk must see)
        let pagings: Vec<(&str, i64, Vec<usize>)> = vec![
            ("one account per page", 1, vec![1, 1, 1, 1]),
            ("two accounts per page", 2, vec![2, 2]),
            ("three accounts per page", 3, vec![3, 1]),
            ("a page larger than the result", 50, vec![4]),
        ];
        for (case, page_size, expected_sizes) in pagings {
            let mut seen: Vec<String> = Vec::new();
            let mut sizes: Vec<usize> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut body = json!({
                    "Catalog": "AWS",
                    "MaxResults": page_size,
                    "Sort": { "SortBy": "COMPANY_NAME", "SortOrder": "ASCENDING" }
                });
                if let Some(token) = token.as_deref() {
                    body["NextToken"] = json!(token);
                }
                let page = search(&router, &body).await;
                page.ok(case);
                let ids = searched_ids(&page.body);
                assert!(
                    ids.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                    "case {case:?}: a page never exceeds MaxResults, got {}",
                    page.body
                );
                sizes.push(ids.len());
                seen.extend(ids);
                token = page
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if token.is_none() {
                    break;
                }
                assert!(
                    sizes.len() <= 10,
                    "case {case:?}: the walk must terminate, saw {sizes:?}"
                );
            }
            assert_eq!(
                sizes, expected_sizes,
                "case {case:?}: expected pages of {expected_sizes:?}, got {sizes:?}"
            );
            let mut unique = seen.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case:?}: the walk repeats no account, got {seen:?}"
            );
            assert_eq!(
                seen,
                vec![
                    id_of("Alpha Analytics"),
                    id_of("Beta Builders"),
                    id_of("Delta Data"),
                    id_of("Gamma Goods"),
                ],
                "case {case:?}: the walk reports every account once, in the order asked \
                 for"
            );
        }

        // A continuation token belongs to the catalog it was issued for.
        let first_page = search(
            &router,
            &json!({
                "Catalog": "AWS",
                "MaxResults": 1,
                "Sort": { "SortBy": "COMPANY_NAME", "SortOrder": "ASCENDING" }
            }),
        )
        .await;
        let token = first_page.field("a token issued for one catalog", "NextToken");
        search(&router, &json!({ "Catalog": "Sandbox", "NextToken": token }))
            .await
            .expect_error(
                "a token issued for one catalog",
                400,
                "ValidationException",
            );
    }

    /// A filter the service cannot evaluate is refused with the modelled
    /// `ValidationException`, rather than quietly matching or missing accounts.
    #[tokio::test]
    async fn search_refuses_a_filter_it_cannot_evaluate() {
        let router = emulator();
        create_ok(&router, "a seeded account", "AWS", &named("Refusal Corp")).await;

        // (case, filter)
        let rows: Vec<(&str, Value)> = vec![
            ("a node that specifies no member", json!({})),
            (
                "a node that specifies two members",
                json!({
                    "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Refusal Corp" } },
                    "ContainsAny": { "Key": "COMPANY_NAME", "Value": { "StringListValue": ["Refusal Corp"] } }
                }),
            ),
            (
                "an empty node nested under a combinator",
                json!({ "AndAllFilters": [{}] }),
            ),
            (
                "EqualsTo carrying a list of values",
                json!({ "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringListValue": ["Refusal Corp"] } } }),
            ),
            (
                "ContainsAny carrying a single value",
                json!({ "ContainsAny": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Refusal Corp" } } }),
            ),
            (
                "ContainsAll on a field that holds one value",
                json!({ "ContainsAll": { "Key": "COMPANY_NAME", "Value": { "StringListValue": ["Refusal Corp"] } } }),
            ),
            (
                "EqualsTo on a date field",
                json!({ "EqualsTo": { "Key": "CREATED_DATE", "Value": { "StringValue": "1970-01-01T00:00:00Z" } } }),
            ),
            (
                "a range comparison on a field that is not a date",
                json!({ "GreaterThan": { "Key": "COMPANY_NAME", "Value": { "DateValue": EPOCH } } }),
            ),
            (
                "a range comparison carrying a string instead of a date",
                json!({ "LessThanOrEquals": { "Key": "CREATED_DATE", "Value": { "StringValue": "1970" } } }),
            ),
        ];

        for (case, filter) in rows {
            search(
                &router,
                &json!({ "Catalog": "AWS", "Filter": filter.clone() }),
            )
            .await
            .expect_error(case, 400, "ValidationException");
            // The same refusal on the statistics path, which evaluates the same filter.
            call(
                &router,
                "GetCustomerAccountStatistics",
                &json!({ "Catalog": "AWS", "Filter": filter }),
            )
            .await
            .expect_error(case, 400, "ValidationException");
        }

        // And a continuation token this service never issued.
        for token in ["not-base64-at-all", "dj E6QVdTOjA="] {
            search(&router, &json!({ "Catalog": "AWS", "NextToken": token }))
                .await
                .expect_error(
                    "a token this service never issued",
                    400,
                    "ValidationException",
                );
        }
    }

    /// Statistics count the accounts the filter selects and group them by the value each
    /// account carries for the field asked for.
    #[tokio::test]
    async fn statistics_count_and_group_the_matching_accounts() {
        let router = emulator();
        let seeded = seeded(&router).await;

        // (case, request, expected total)
        let totals: Vec<(&str, Value, i64)> = vec![
            (
                "every account in the catalog",
                json!({ "Catalog": "AWS" }),
                4,
            ),
            (
                "the accounts one company name selects",
                json!({
                    "Catalog": "AWS",
                    "Filter": { "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Alpha Analytics" } } }
                }),
                1,
            ),
            (
                "the accounts that settled without a problem",
                json!({
                    "Catalog": "AWS",
                    "Filter": { "EqualsTo": { "Key": "ENRICHMENT_STATUS", "Value": { "StringValue": NO_MATCH_FOUND } } }
                }),
                3,
            ),
            (
                "a filter no account satisfies",
                json!({
                    "Catalog": "AWS",
                    "Filter": { "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Nobody Ltd" } } }
                }),
                0,
            ),
            ("the other catalog", json!({ "Catalog": "Sandbox" }), 1),
        ];

        for (case, body, expected) in totals {
            let answer = call(&router, "GetCustomerAccountStatistics", &body).await;
            answer.ok(case);
            assert_eq!(
                answer.body.get("TotalCount").and_then(Value::as_i64),
                Some(expected),
                "case {case:?}: expected TotalCount {expected}, got {}",
                answer.body
            );
            assert_eq!(
                answer.body.get("Aggregations"),
                None,
                "case {case:?}: a request that asks for no grouping gets none, got {}",
                answer.body
            );
        }

        // Grouping by the status every account carries.
        let grouped = call(
            &router,
            "GetCustomerAccountStatistics",
            &json!({ "Catalog": "AWS", "Aggregations": [{ "GroupBy": ["ENRICHMENT_STATUS"] }] }),
        )
        .await;
        grouped.ok("grouped by enrichment status");
        let aggregations = grouped
            .body
            .get("Aggregations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            aggregations.len(),
            1,
            "case \"grouped by enrichment status\": one aggregation was asked for, got {}",
            grouped.body
        );
        assert_eq!(
            aggregations[0].get("GroupBy").and_then(Value::as_array),
            Some(&vec![json!("ENRICHMENT_STATUS")]),
            "case \"grouped by enrichment status\": the result names the field grouped \
             by, got {}",
            grouped.body
        );
        assert_eq!(
            buckets_of(&aggregations[0]),
            vec![
                (NO_MATCH_FOUND.to_string(), 3),
                (ACTION_REQUIRED.to_string(), 1),
            ],
            "case \"grouped by enrichment status\": the most populous group comes first, \
             got {}",
            grouped.body
        );
        assert_eq!(
            aggregations[0].get("OtherCount").and_then(Value::as_i64),
            Some(0),
            "case \"grouped by enrichment status\": every group is reported, so no \
             account is counted elsewhere, got {}",
            grouped.body
        );

        // (case, include missing, the buckets a field no account carries produces)
        let missing_rows: Vec<MissingGroupRow> = vec![
            (
                "a field no account carries, missing groups left out",
                None,
                vec![],
            ),
            (
                "a field no account carries, missing groups left out explicitly",
                Some(false),
                vec![],
            ),
            (
                "a field no account carries, missing groups included",
                Some(true),
                vec![(String::new(), 4)],
            ),
        ];
        for (case, include_missing, expected) in missing_rows {
            let mut body = json!({
                "Catalog": "AWS",
                "Aggregations": [{ "GroupBy": ["AWS_INDUSTRY"] }]
            });
            if let Some(include_missing) = include_missing {
                body["IncludeMissing"] = json!(include_missing);
            }
            let answer = call(&router, "GetCustomerAccountStatistics", &body).await;
            answer.ok(case);
            let aggregations = answer
                .body
                .get("Aggregations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                aggregations.len(),
                1,
                "case {case:?}: one aggregation was asked for, got {}",
                answer.body
            );
            assert_eq!(
                buckets_of(&aggregations[0]),
                expected,
                "case {case:?}: expected the groups {expected:?}, got {}",
                answer.body
            );
        }

        // Several aggregations in one request, each reporting its own field.
        let several = call(
            &router,
            "GetCustomerAccountStatistics",
            &json!({
                "Catalog": "AWS",
                "Aggregations": [
                    { "GroupBy": ["ENRICHMENT_STATUS"] },
                    { "GroupBy": ["COUNTRY"] },
                    { "GroupBy": ["PROGRAM_ELIGIBILITY"] }
                ]
            }),
        )
        .await;
        several.ok("three aggregations in one request");
        let fields: Vec<String> = several
            .body
            .get("Aggregations")
            .and_then(Value::as_array)
            .map(|results| {
                results
                    .iter()
                    .filter_map(|result| {
                        result
                            .get("GroupBy")
                            .and_then(Value::as_array)
                            .and_then(|group| group.first())
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec!["ENRICHMENT_STATUS", "COUNTRY", "PROGRAM_ELIGIBILITY"],
            "case \"three aggregations in one request\": each aggregation answers the \
             field it asked for, in order, got {}",
            several.body
        );

        // And the reporting calls above changed no account.
        for (company, id) in &seeded {
            let read = get(&router, "AWS", id).await;
            read.ok(company);
            assert_eq!(
                read.field(company, "Id"),
                *id,
                "case {company:?}: a reporting call changes no account, got {}",
                read.body
            );
        }
    }

    /// A `Filter` tree is walked only within the depth and size the service bounds it
    /// to: inside the bounds every level is evaluated, and a tree past either bound is
    /// refused with the modelled `ValidationException` rather than walked.
    #[tokio::test]
    async fn search_bounds_the_filter_tree_it_walks() {
        let router = emulator();
        create_ok(&router, "the named account", "AWS", &named("Nesting Corp")).await;
        create_ok(&router, "the other account", "AWS", &named("Other Corp")).await;

        // (case, filter, the companies it selects)
        let accepted: Vec<(&str, Value, Vec<&str>)> = vec![
            (
                "one level: the comparison alone",
                nested_filter(1),
                vec!["Nesting Corp"],
            ),
            (
                "one level short of the deepest the service walks",
                nested_filter(MAX_FILTER_DEPTH - 1),
                // An even number of negations around the comparison.
                vec!["Nesting Corp"],
            ),
            (
                "the deepest level the service walks",
                nested_filter(MAX_FILTER_DEPTH),
                // One more negation, so the other account is the match.
                vec!["Other Corp"],
            ),
        ];
        for (case, filter, expected) in accepted {
            let page = search(&router, &json!({ "Catalog": "AWS", "Filter": filter })).await;
            let mut companies = searched_companies(page.ok(case));
            companies.sort();
            assert_eq!(
                companies,
                expected
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
                "case {case:?}: every level of the tree is evaluated, got {}",
                page.body
            );
        }

        // (case, filter past a bound)
        let refused: Vec<(&str, Value)> = vec![
            (
                "one level deeper than the service walks",
                nested_filter(MAX_FILTER_DEPTH + 1),
                ),
            (
                "far deeper than the service walks",
                nested_filter(MAX_FILTER_DEPTH * 20),
            ),
            ("more nodes than the service walks", wide_filter()),
        ];
        for (case, filter) in refused {
            search(
                &router,
                &json!({ "Catalog": "AWS", "Filter": filter.clone() }),
            )
            .await
            .expect_error(case, 400, "ValidationException");
            // The statistics path evaluates the same tree, so it refuses it too.
            call(
                &router,
                "GetCustomerAccountStatistics",
                &json!({ "Catalog": "AWS", "Filter": filter }),
            )
            .await
            .expect_error(case, 400, "ValidationException");
        }
    }

    /// A chain of `levels` nodes: `levels - 1` negations around one comparison, so the
    /// tree is exactly `levels` deep and its answer flips with each level.
    fn nested_filter(levels: usize) -> Value {
        let mut filter = json!({
            "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Nesting Corp" } }
        });
        for _ in 1..levels {
            filter = json!({ "NotFilter": filter });
        }
        filter
    }

    /// A shallow tree with more nodes than the service walks: 10 combinators of 10
    /// comparisons each, 111 nodes in all, which `MAX_FILTER_NODES` is below.
    fn wide_filter() -> Value {
        const NODES: usize = 1 + 10 + 100;
        const _: () = assert!(
            MAX_FILTER_NODES < NODES,
            "this tree must be larger than the node bound it is built to exceed"
        );
        let leaf = json!({
            "EqualsTo": { "Key": "COMPANY_NAME", "Value": { "StringValue": "Nesting Corp" } }
        });
        let branch = json!({ "AndAllFilters": vec![leaf; 10] });
        json!({ "AndAllFilters": vec![branch; 10] })
    }

    /// A page never holds more accounts than the modelled maximum: a `MaxResults`
    /// outside `range 1..100` is refused, and the handler clamps whatever reaches it.
    #[tokio::test]
    async fn a_search_page_never_exceeds_the_modelled_maximum() {
        let router = emulator();
        let stored = usize::try_from(MAX_SEARCH_MAX_RESULTS).unwrap_or(100) + 1;
        for index in 0..stored {
            let case = format!("create {index}");
            create(&router, "AWS", &named(&format!("Paged {index:04} Inc")))
                .await
                .ok(&case);
        }

        // (case, the MaxResults the request carries, the page size it gets)
        let rows: Vec<(&str, Option<i64>, usize)> = vec![
            ("no MaxResults at all", None, 20),
            (
                "the modelled maximum",
                Some(i64::from(MAX_SEARCH_MAX_RESULTS)),
                100,
            ),
            ("the modelled minimum", Some(1), 1),
        ];
        for (case, max_results, expected) in rows {
            let mut body = json!({
                "Catalog": "AWS",
                "Sort": { "SortBy": "COMPANY_NAME", "SortOrder": "ASCENDING" }
            });
            if let Some(max_results) = max_results {
                body["MaxResults"] = json!(max_results);
            }
            let page = search(&router, &body).await;
            page.ok(case);
            assert_eq!(
                searched_ids(&page.body).len(),
                expected,
                "case {case:?}: expected a page of {expected} accounts, got {}",
                page.body
            );
            assert!(
                page.body.get("NextToken").is_some(),
                "case {case:?}: the accounts past the page are reachable by a \
                 continuation, got {}",
                page.body
            );
        }

        // A page size outside the modelled `range 1..100` is refused, so no request can
        // ask for the whole store in one answer.
        for (case, max_results) in [
            ("one past the modelled maximum", i64::from(MAX_SEARCH_MAX_RESULTS) + 1),
            ("as large a page as the member can carry", i64::from(i32::MAX)),
            ("a page of none", 0),
        ] {
            search(
                &router,
                &json!({ "Catalog": "AWS", "MaxResults": max_results }),
            )
            .await
            .expect_error(case, 400, "ValidationException");
        }
    }

    /// The customer-account store is bounded: a create past the ceiling is refused with
    /// the modelled quota error instead of growing the store.
    #[tokio::test]
    async fn the_customer_account_store_stays_bounded() {
        let (router, engine) = emulator_with_engine();

        for index in 0..MAX_CUSTOMER_ACCOUNTS {
            let case = format!("create {index}");
            create(&router, "AWS", &named(&format!("Bounded {index} Inc")))
                .await
                .ok(&case);
        }
        create(&router, "AWS", &named("One Too Many Inc"))
            .await
            .expect_error(
                "past the ceiling",
                402,
                "ServiceQuotaExceededException",
            );

        let store = service_state(&*engine).expect("the service store is reachable");
        let store = store.lock();
        assert_eq!(
            store.customer_accounts.len(),
            MAX_CUSTOMER_ACCOUNTS,
            "the store must stay at its ceiling of {MAX_CUSTOMER_ACCOUNTS}"
        );
    }
}
