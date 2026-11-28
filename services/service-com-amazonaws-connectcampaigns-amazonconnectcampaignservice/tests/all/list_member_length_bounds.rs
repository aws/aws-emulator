// SPDX-License-Identifier: CC0-1.0

//! The modelled `@length` of a LIST member, end to end over HTTP.
//!
//! The rule under test: every list member of this service's inputs is held to the `@length`
//! its MODEL declares, at the request boundary — a list inside those bounds reaches the
//! handler and is served, and a list outside them is refused HTTP 400 `ValidationException`
//! in the envelope every AWS service states a constraint in, whose clause names the member,
//! the requirement it broke and the list the caller submitted. No layer of this service
//! applies a bound of its own on top of the model's, so the size that is refused is the one
//! the model publishes and nothing else.
//!
//! The rows span every list member this service's operations declare, each with its own
//! published bounds and its own binding — two in a request BODY (`GetCampaignStateBatch`'s
//! `campaignIds`, `@length(min 1, max 500)` as the overlay model that operation's input is
//! declared from states it — see `smithy-model/LOCAL-ADDITIONS.md` — and
//! `PutDialRequestBatch`'s `dialRequests`, `@length(min 1, max 25)`) and one in the QUERY
//! STRING (`UntagResource`'s `tagKeys`, `@length(min 0, max 50)`), from
//! `smithy_query(query="operation",
//! operation="GetCampaignStateBatch,PutDialRequestBatch,UntagResource")` — and, per member,
//! the sizes around each published bound: one element, each bound itself, and one element
//! past the maximum.

use serde_json::{json, Value};

use super::common::{call, router, Answer, Harness};

/// The region every case here signs for.
const REGION: &str = "us-east-1";

/// The Connect instance every campaign below is created for.
const INSTANCE: &str = "bounds-instance";

/// The sentence a request refused for ONE broken constraint is answered with: the envelope
/// around a clause naming the value half the class states, the member it sits at, and the
/// requirement it broke. Written out here rather than read back out of the implementation,
/// so this suite pins the wording.
fn one_error(subject: &str, requirement: &str) -> String {
    format!("1 validation error detected: {subject} failed to satisfy constraint: {requirement}")
}

/// A list as a clause quotes the value of the member that carried it: its elements in
/// request order, comma-separated, in brackets.
fn quoted_list(elements: &[String]) -> String {
    format!("[{}]", elements.join(", "))
}

/// One list member of this service's inputs.
#[derive(Clone, Copy, Debug)]
enum ListMember {
    /// `GetCampaignStateBatchRequest$campaignIds`, a list of strings in the body.
    CampaignIds,
    /// `PutDialRequestBatchRequest$dialRequests`, a list of structures in the body.
    DialRequests,
    /// `UntagResourceRequest$tagKeys`, a list of strings in the query string.
    TagKeys,
}

impl ListMember {
    /// The member path a clause about this member names it at — the Smithy member name, as
    /// the framework's own path spelling writes it.
    fn path(self) -> &'static str {
        match self {
            Self::CampaignIds => "campaignIds",
            Self::DialRequests => "dialRequests",
            Self::TagKeys => "tagKeys",
        }
    }

    /// The `@length` bounds the model declares for this member, as `(min, max)`.
    fn bounds(self) -> (usize, usize) {
        match self {
            // The batch state read's own maximum is the one the SERVICE enforces, which the
            // recorded corpus shows is five hundred ids and not the twenty-five the
            // member's documentation prints; it is declared in the overlay model this
            // operation's input comes from (`smithy-model/LOCAL-ADDITIONS.md`, and
            // `crate::ops::get_campaign_state_batch`).
            Self::CampaignIds => (1, 500),
            Self::DialRequests => (1, 25),
            Self::TagKeys => (0, 50),
        }
    }

    /// The elements a request below sends for this member, as the clause about it quotes
    /// them: `None` for a list whose elements are STRUCTURES, which this service's clauses
    /// quote no rendering of.
    fn quoted_elements(self, count: usize) -> Option<Vec<String>> {
        match self {
            Self::CampaignIds => Some((0..count).map(|index| format!("absent-{index}")).collect()),
            // `TagKey`'s pattern admits letters, digits and `-`
            // (`smithy_query(query="shape", shape="TagKey")`).
            Self::TagKeys => Some((0..count).map(|index| format!("key-{index}")).collect()),
            // The EMPTY list is the one value a list of structures has a rendering of here:
            // zero elements render as the empty list and nothing else.
            Self::DialRequests => (count == 0).then(Vec::new),
        }
    }

    /// How many of one answer's per-element entries belong to this member's batch: a batch
    /// operation answers every element it was handed, in `successfulRequests` or in
    /// `failedRequests`. `None` for an operation whose output is `Unit`, which reports no
    /// per-element entry at all.
    fn answered(self, answer: &Answer) -> Option<usize> {
        match self {
            Self::CampaignIds | Self::DialRequests => Some(
                ["successfulRequests", "failedRequests"]
                    .iter()
                    .map(|member| answer.body[*member].as_array().map_or(0, Vec::len))
                    .sum(),
            ),
            Self::TagKeys => None,
        }
    }
}

/// A minimal `CreateCampaign` body: every member the model marks required, and nothing
/// else.
fn create_body(name: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": INSTANCE,
        "dialerConfig": {"agentlessDialerConfig": {}},
        "outboundCallConfig": {"connectContactFlowId": "flow-1"},
    })
}

/// The campaign every case sends its lists against: created for [`INSTANCE`] and started,
/// so a dial batch inside its bounds is one the campaign takes. Handed back as
/// `(id, arn)`.
async fn a_running_campaign(router: &Harness, name: &str) -> (String, String) {
    router.register_connect_instance(REGION, INSTANCE);
    let created = call(router, REGION, "PUT", "/campaigns", &create_body(name)).await;
    assert_eq!(
        created.status, 200,
        "case \"create campaign {name}\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    let id = created.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"create campaign {name}\": no id in {}", created.body))
        .to_string();
    let arn = created.body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"create campaign {name}\": no arn in {}", created.body))
        .to_string();
    let started = call(
        router,
        REGION,
        "POST",
        &format!("/campaigns/{id}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case \"start campaign {name}\": expected HTTP 200, got {} {}",
        started.status, started.body
    );
    (id, arn)
}

/// One `tagKeys` value, percent-encoded for a query string.
fn encoded_query(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('+', "%2B")
        .replace('=', "%3D")
        .replace('&', "%26")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// Send a request carrying `count` elements in `member`, against the campaign
/// `(id, arn)` names.
async fn send(
    router: &Harness,
    member: ListMember,
    count: usize,
    id: &str,
    arn: &str,
) -> Answer {
    match member {
        ListMember::CampaignIds => {
            let ids = member.quoted_elements(count).expect("a list of strings");
            call(
                router,
                REGION,
                "POST",
                "/campaigns-state",
                &json!({"campaignIds": ids}),
            )
            .await
        }
        ListMember::DialRequests => {
            let requests: Vec<Value> = (0..count)
                .map(|index| {
                    json!({
                        "clientToken": format!("tok-{index}"),
                        "phoneNumber": "+15555550100",
                        "expirationTime": "2099-01-01T00:00:00Z",
                        "attributes": {"campaign": "nightly"},
                    })
                })
                .collect();
            call(
                router,
                REGION,
                "PUT",
                &format!("/campaigns/{id}/dial-requests"),
                &json!({"dialRequests": requests}),
            )
            .await
        }
        ListMember::TagKeys => {
            let keys = member.quoted_elements(count).expect("a list of strings");
            let mut path = format!("/tags/{}", encoded_query(arn));
            for (index, key) in keys.iter().enumerate() {
                path.push(if index == 0 { '?' } else { '&' });
                path.push_str("tagKeys=");
                path.push_str(&encoded_query(key));
            }
            call(router, REGION, "DELETE", &path, &Value::Null).await
        }
    }
}

#[tokio::test]
async fn a_list_inside_its_modelled_length_is_served() {
    let router = router();
    let (id, arn) = a_running_campaign(&router, "inside").await;

    // Rows: every list member, at each size inside its own published bounds — one element
    // and the maximum itself. The EMPTY list has no row even where a member's published
    // minimum is zero: `tagKeys` is `@required` and bound to the query string, so a request
    // carrying none of it names no route at all rather than reaching the operation.
    for member in [
        ListMember::CampaignIds,
        ListMember::DialRequests,
        ListMember::TagKeys,
    ] {
        let (_, max) = member.bounds();
        for count in [1, max] {
            let case = format!("{} at {count} elements", member.path());
            let answer = send(&router, member, count, &id, &arn).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a list inside its modelled length is served, got {} {}",
                answer.status, answer.body
            );
            // A list inside its bounds is not merely accepted: every element of it is
            // answered. A batch operation answers one entry per element it was handed, and
            // an operation whose output is `Unit` carries no clause of a refusal at all.
            match member.answered(&answer) {
                Some(answered) => assert_eq!(
                    answered, count,
                    "case {case:?}: expected one answer per element, got {} in {}",
                    answered, answer.body
                ),
                None => assert!(
                    answer.body.get("message").is_none(),
                    "case {case:?}: expected a success carrying no refusal, got {}",
                    answer.body
                ),
            }
        }
    }
}

#[tokio::test]
async fn a_list_outside_its_modelled_length_is_refused_at_the_boundary() {
    let router = router();
    let (id, arn) = a_running_campaign(&router, "outside").await;

    // Rows: every list member, at each size OUTSIDE its own published bounds — one element
    // below the minimum where the model states one above zero, and one past the maximum.
    let mut rows: Vec<(ListMember, usize, String)> = Vec::new();
    for member in [
        ListMember::CampaignIds,
        ListMember::DialRequests,
        ListMember::TagKeys,
    ] {
        let (min, max) = member.bounds();
        if min > 0 {
            rows.push((
                member,
                min - 1,
                format!("Member must have length greater than or equal to {min}"),
            ));
        }
        rows.push((
            member,
            max + 1,
            format!("Member must have length less than or equal to {max}"),
        ));
    }

    for (member, count, requirement) in rows {
        let case = format!("{} at {count} elements", member.path());
        let answer = send(&router, member, count, &id, &arn).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a list outside its modelled length is refused, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} on HTTP {}",
            answer.error_code,
            answer.status
        );
        // The clause states the list the caller submitted as the value of the member that
        // broke the bound — a list has a rendering of its own, which is the half a scalar
        // member's clause drops. A list of STRUCTURES has no such rendering here, so its
        // clause names the member and the requirement and nothing else.
        let subject = match member.quoted_elements(count) {
            Some(elements) => format!(
                "Value '{}' at '{}'",
                quoted_list(&elements),
                member.path()
            ),
            None => format!("Value at '{}'", member.path()),
        };
        assert_eq!(
            answer.message(),
            one_error(&subject, &requirement),
            "case {case:?}: expected this service's constraint sentence, got {}",
            answer.body
        );
    }

    // And no refused request was served: the campaign still takes a batch of its own, and
    // the state read still answers for it.
    let served = send(&router, ListMember::DialRequests, 1, &id, &arn).await;
    assert_eq!(
        served.status, 200,
        "case \"the campaign after every refusal\": expected a dial batch inside the bounds \
         to be served, got {} {}",
        served.status, served.body
    );
}
