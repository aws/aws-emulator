// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE way this service refuses a request: its modelled
//! `ValidationException` (HTTP 400).
//!
//! The rule under test: a refused request is answered with the modelled
//! `ValidationException` — the `@required` `Message` always, `Reason` only when the
//! service classifies the failure with a modelled `ValidationExceptionReason`
//! (`FIELD_VALIDATION_FAILED` for a member value the service refused,
//! `CANNOT_PARSE` for a value it could not decode) or carries it unclassified for a
//! request it could not READ as the shape requires, and one `Fields` entry per member a
//! field validation names.
//!
//! Every request goes through the real `MultiServiceRouter`, so the boundary walk
//! (deserialize, then constraint validation) and the handler are both exercised exactly
//! as they are in production — including for a request no handler of this service can
//! serve, whose refusal envelope is the same one.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

/// The wire error code every refusal in this file must carry.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `WorkloadId`-shaped value (`^[0-9a-f]{32}$`), so a request aimed at a path with a
/// `WorkloadId` label is refused for the member under test and not for its URI.
const WORKLOAD_ID: &str = "0123456789abcdef0123456789abcdef";

/// Assert the shared facts of a refusal and hand back the parsed body.
fn refused(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, VALIDATION_EXCEPTION,
        "case {case:?}: expected the error code {VALIDATION_EXCEPTION}, got {answer:?}"
    );
    let message = answer.body.get("Message").and_then(Value::as_str);
    assert!(
        message.is_some_and(|message| message.starts_with("[Validation] ")
            && message.len() > "[Validation] ".len()),
        "case {case:?}: Message is @required on ValidationException and opens with this \
         service's failure-class tag, got {answer:?}"
    );
    answer.body.clone()
}

/// A value outside a modelled numeric `@range` is a field validation: `Reason`
/// `FIELD_VALIDATION_FAILED`, one `Fields` entry naming the member and the published
/// bounds, and `Message` stating that sentence.
///
/// Rows differ along the operation (two implemented, one still scaffolded), along how
/// `MaxResults` reaches the service (a body member, or the `@httpQuery` binding
/// `ListProfiles` gives it), along which bound is broken, and along how far past it the
/// value is. Each published bound is also tested ON the bound and one step past it.
#[tokio::test]
async fn out_of_range_page_size_names_the_member_and_its_bounds() {
    let client = Client::new();
    // `ListMilestones` RESOLVES its path `WorkloadId`, so its row needs a workload that
    // exists: the refusals below are then about `MaxResults` and not about the id.
    let workload_id = created_id(
        &client
            .create(minimal_create("page-size-host", "tok-page-size"))
            .await,
        "page-size-host",
    );

    // `MaxResults` is `@range(min: 1, max: 50)` on every paginated operation of this
    // service — they all target the one modelled `MaxResults` shape
    // (`smithy_query(query="operation", operation="ListWorkloads,ListMilestones,ListProfiles")`).
    // `in_query` is whether that member is bound to the query string instead of the body.
    let rows: Vec<(&str, &str, String, bool)> = vec![
        (
            "ListWorkloads",
            "POST",
            "/workloadsSummaries".to_string(),
            false,
        ),
        (
            "ListMilestones",
            "POST",
            format!("/workloads/{workload_id}/milestonesSummaries"),
            false,
        ),
        ("ListProfiles", "GET", "/profileSummaries".to_string(), true),
    ];
    let request_for = |max_results: i64, path: &str, in_query: bool| {
        if in_query {
            (format!("{path}?MaxResults={max_results}"), None)
        } else {
            (path.to_string(), Some(json!({ "MaxResults": max_results })))
        }
    };

    for (operation, method, path, in_query) in &rows {
        for max_results in [-1, 0, 51, 100, 1_000] {
            let case = format!("{operation} MaxResults {max_results}");
            let (uri, body) = request_for(max_results, path, *in_query);
            let answer = client.send(method, &uri, body).await;
            let body = refused(&answer, &case);
            assert_eq!(
                body.get("Reason").and_then(Value::as_str),
                Some("FIELD_VALIDATION_FAILED"),
                "case {case:?}: a value outside a published range is a field validation, got {body:?}"
            );
            assert_eq!(
                body.get("Fields"),
                Some(&json!([{
                    "Name": "MaxResults",
                    "Message": "MaxResults must be between 1 and 50.",
                }])),
                "case {case:?}: Fields must name the member and its bounds, got {body:?}"
            );
            assert_eq!(
                body.get("Message").and_then(Value::as_str),
                Some("[Validation] MaxResults must be between 1 and 50."),
                "case {case:?}: Message must state the field's sentence once, got {body:?}"
            );
        }

        // ON each published bound the value is accepted, so the refusals above are about
        // the value and not about the request. An implemented operation answers 200; a
        // scaffolded one answers its 501, which is what "accepted" means for it.
        for max_results in [1, 50] {
            let case = format!("{operation} MaxResults {max_results}");
            let (uri, body) = request_for(max_results, path, *in_query);
            let answer = client.send(method, &uri, body).await;
            assert!(
                answer.status == 200 || answer.status == 501,
                "case {case:?}: a value on the published bound must not be refused, got {answer:?}"
            );
        }
    }
}

/// A value that fails a string shape's constraint (`@length`, `@pattern`, an enum value
/// set) is measured by the layer that READ the request body, so the modelled `Message` is
/// that layer's finding — what it found, not the member it was found on — and nothing is
/// classified: no `Reason`, no `Fields`.
///
/// Rows differ along the constraint class, the operation (one of the four is still
/// scaffolded, so the envelope cannot come from a handler), and the member's binding;
/// `super::schema_finding_wording` pins the whole message of each class over a wider table.
#[tokio::test]
async fn string_constraint_refusals_carry_the_message_alone() {
    // Bounds and value sets from `smithy_query(query="operation", ...)`:
    // CreateWorkload WorkloadName @length(3..100) / Description @length(3..250),
    // CreateMilestone MilestoneName @length(3..100),
    // CreateReviewTemplate TemplateName @pattern("^[A-Za-z0-9-_.,:/()@!&?#+'’\\s]+$"),
    // UpdateAnswer Reason -> AnswerReason enum. The last element of a row is a fragment the
    // finding has to carry: the value the layer measured, or the enum form it reports.
    let cases: Vec<(&str, &str, String, Value, &str)> = vec![
        (
            "workload-name-below-min",
            "POST",
            "/workloads".to_string(),
            {
                let mut request = minimal_create("ab", "tok-name-short");
                request["WorkloadName"] = json!("ab");
                request
            },
            "string \"ab\" is too short (length: 2, required minimum: 3)",
        ),
        (
            "workload-name-above-max",
            "POST",
            "/workloads".to_string(),
            minimal_create(&"n".repeat(101), "tok-name-long"),
            "is too long (length: 101, maximum allowed: 100)",
        ),
        (
            "description-below-min",
            "POST",
            "/workloads".to_string(),
            {
                let mut request = minimal_create("short-description", "tok-desc-short");
                request["Description"] = json!("ab");
                request
            },
            "string \"ab\" is too short (length: 2, required minimum: 3)",
        ),
        (
            "review-owner-below-min",
            "POST",
            "/workloads".to_string(),
            {
                let mut request = minimal_create("short-owner", "tok-owner-short");
                request["ReviewOwner"] = json!("ab");
                request
            },
            "string \"ab\" is too short (length: 2, required minimum: 3)",
        ),
        (
            "milestone-name-below-min",
            "POST",
            format!("/workloads/{WORKLOAD_ID}/milestones"),
            json!({ "MilestoneName": "ab", "ClientRequestToken": "tok-milestone" }),
            "string \"ab\" is too short (length: 2, required minimum: 3)",
        ),
        (
            "template-name-outside-the-pattern",
            "POST",
            "/reviewTemplates".to_string(),
            json!({
                "TemplateName": "probe*template*name",
                "Description": "a template whose name holds a character the pattern bars",
                "Lenses": ["wellarchitected"],
                "ClientRequestToken": "tok-template",
            }),
            "does not match input string \"probe*template*name\"",
        ),
        (
            // A value outside an enumerated type's set is reported as the schema finding
            // that quotes the value and the set rather than as a sentence naming the
            // member, so this row states the finding; `super::enum_refusal_wording` pins
            // the whole message of that class.
            "answer-reason-outside-the-enum",
            "PATCH",
            format!("/workloads/{WORKLOAD_ID}/lensReviews/wellarchitected/answers/q1"),
            json!({ "Reason": "NOT_AN_ANSWER_REASON" }),
            "not found in enum",
        ),
    ];

    let client = Client::new();
    for (case, method, path, request, finding) in cases {
        let answer = client.send(method, &path, Some(request)).await;
        let body = refused(&answer, case);
        let message = body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(finding),
            "case {case:?}: Message must state the finding {finding:?}, got {message:?}"
        );
        assert_eq!(
            body.get("Reason"),
            None,
            "case {case:?}: a shape constraint is not classified, got {body:?}"
        );
        assert_eq!(
            body.get("Fields"),
            None,
            "case {case:?}: a shape constraint reports no field findings, got {body:?}"
        );
    }
}

/// A member the request had to carry and did not is answered with the property-level
/// finding the layer that READS a request reports, and is answered BEFORE any rule over the
/// members the request DID carry — so the caller is told about the member it omitted, not a
/// different one.
#[tokio::test]
async fn absent_required_member_is_reported_before_any_other_rule() {
    let client = Client::new();

    // Rows differ along WHICH member is absent and along whether the model requires it
    // (`ClientRequestToken`, `Lenses`) or the operation's own documentation does
    // (`ReviewOwner`: "You also must specify ReviewOwner, even though the parameter is
    // listed as not being required").
    for (case, member) in [
        ("modelled-client-request-token", "ClientRequestToken"),
        ("modelled-lenses", "Lenses"),
        ("modelled-description", "Description"),
        ("documented-review-owner", "ReviewOwner"),
    ] {
        let mut request = minimal_create(&format!("absent-{case}"), &format!("tok-{case}"));
        request
            .as_object_mut()
            .expect("the request is a JSON object")
            .remove(member);
        let answer = client.create(request).await;
        let body = refused(&answer, case);
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some(
                format!("[Validation] [object has missing required properties ([\"{member}\"])]")
                    .as_str()
            ),
            "case {case:?}: Message must name the absent property {member:?}, got {body:?}"
        );
        assert_eq!(
            body.get("Reason"),
            Some(&json!("")),
            "case {case:?}: the request was never read, so Reason is carried unclassified, got \
             {body:?}"
        );
        assert_eq!(
            body.get("Fields"),
            None,
            "case {case:?}: no field rule ran, so no field finding is reported, got {body:?}"
        );
    }

    // The absent member wins over the cross-member Regions rule, however many other
    // rules the same request also breaks.
    for (case, request) in [
        (
            "no-owner-and-no-regions",
            json!({
                "WorkloadName": "no-owner-no-regions",
                "Description": "neither ReviewOwner nor any Region",
                "Environment": "PRODUCTION",
                "Lenses": [],
                "ClientRequestToken": "tok-neither",
            }),
        ),
        (
            "no-owner-and-unknown-region",
            json!({
                "WorkloadName": "no-owner-bad-region",
                "Description": "no ReviewOwner and a Region that is not one",
                "Environment": "PRODUCTION",
                "Lenses": [],
                "AwsRegions": ["probe-region-1"],
                "ClientRequestToken": "tok-owner-region",
            }),
        ),
    ] {
        let answer = client.create(request).await;
        let body = refused(&answer, case);
        let message = body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains("ReviewOwner"),
            "case {case:?}: the absent ReviewOwner must be reported, got {message:?}"
        );
        assert!(
            !message.contains("AwsRegions"),
            "case {case:?}: the caller must not be sent to fix a member it did send, got {message:?}"
        );
    }
}

/// The documented rule "Either AwsRegions, NonAwsRegions, or both must be specified"
/// spans two members, so the refusal names BOTH of them in `Fields` with the one
/// sentence that states the rule, and `Message` states it once.
///
/// Rows differ along the form "unspecified" takes: omitted, empty, and one of each.
#[tokio::test]
async fn the_regions_rule_names_every_member_that_satisfies_it() {
    let client = Client::new();
    let sentence = "Either AwsRegions or NonAwsRegions is required.";

    for (case, regions) in [
        ("both-omitted", json!({})),
        ("both-empty", json!({ "AwsRegions": [], "NonAwsRegions": [] })),
        ("aws-empty-non-aws-omitted", json!({ "AwsRegions": [] })),
        ("non-aws-empty-aws-omitted", json!({ "NonAwsRegions": [] })),
    ] {
        let mut request = json!({
            "WorkloadName": format!("regions-{case}"),
            "Description": "a workload that names no Region at all",
            "Environment": "PRODUCTION",
            "Lenses": [],
            "ReviewOwner": "owner@example.com",
            "ClientRequestToken": format!("tok-{case}"),
        });
        for (member, value) in regions.as_object().expect("an object of members") {
            request[member] = value.clone();
        }

        let answer = client.create(request).await;
        let body = refused(&answer, case);
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("FIELD_VALIDATION_FAILED"),
            "case {case:?}: the rule is about the members' values, got {body:?}"
        );
        assert_eq!(
            body.get("Fields"),
            Some(&json!([
                { "Name": "AwsRegions", "Message": sentence },
                { "Name": "NonAwsRegions", "Message": sentence },
            ])),
            "case {case:?}: both members the rule covers must be reported, got {body:?}"
        );
        assert_eq!(
            body.get("Message").and_then(Value::as_str),
            Some(format!("[Validation] {sentence}").as_str()),
            "case {case:?}: Message states the rule once, not once per member, got {body:?}"
        );
    }
}

/// A Region a workload claims to run in has to be a Region: `AwsRegions` entries are
/// checked against the published Region list, and a value no Region carries is refused
/// as a field validation naming `AwsRegions` — before anything is stored.
///
/// Rows differ along the Region id's form (a real Region from several partitions, none
/// of them the one the recorded request used; a value with a Region's shape that is not
/// one; and values that are not Region-shaped at all).
#[tokio::test]
async fn unknown_aws_region_is_refused_and_creates_nothing() {
    // The accepted rows are entries of `state::AWS_REGIONS`, whose source is named
    // there; `eu-west-3`, `ca-west-1`, `ap-southeast-7`, `us-gov-west-1` and
    // `cn-north-1` are not among the regions the issue cited.
    let accepted = [
        "us-east-1",
        "eu-west-3",
        "ca-west-1",
        "ap-southeast-7",
        "us-gov-west-1",
        "cn-north-1",
    ];
    let refused_regions = [
        ("probe-name", "sim2real-probe-region"),
        ("region-shaped-but-absent", "us-east-9"),
        ("uppercased", "US-EAST-1"),
        ("trailing-space", "us-east-1 "),
        ("empty", ""),
        ("partition-only", "us"),
    ];

    let client = Client::new();

    for region in accepted {
        let mut request = minimal_create(&format!("ok-{region}"), &format!("tok-ok-{region}"));
        request["AwsRegions"] = json!([region]);
        let answer = client.create(request).await;
        let workload_id = created_id(&answer, region);
        // Read it back: the Region is stored, not merely accepted.
        let fetched = client.get(&workload_id).await;
        assert_eq!(
            fetched
                .body
                .get("Workload")
                .and_then(|workload| workload.get("AwsRegions")),
            Some(&json!([region])),
            "case {region:?}: GetWorkload must report the Region that was stored, got {fetched:?}"
        );
    }

    for (case, region) in refused_regions {
        // Alone, and beside a Region that IS one — a single bad entry refuses the call.
        for (shape, regions) in [
            ("alone", json!([region])),
            ("beside-a-real-region", json!(["us-west-2", region])),
        ] {
            let case = format!("{case}/{shape}");
            let mut request = minimal_create(
                &format!("bad-{case}"),
                &format!("tok-bad-{case}").replace('/', "-"),
            );
            request["AwsRegions"] = regions;
            let answer = client.create(request).await;
            let body = refused(&answer, &case);
            assert_eq!(
                body.get("Reason").and_then(Value::as_str),
                Some("FIELD_VALIDATION_FAILED"),
                "case {case:?}: an unknown Region is a field validation, got {body:?}"
            );
            assert_eq!(
                body.get("Fields"),
                Some(&json!([{
                    "Name": "AwsRegions",
                    "Message": format!("Unknown region {region}."),
                }])),
                "case {case:?}: Fields must name AwsRegions and the value, got {body:?}"
            );
        }
    }

    // Nothing the refused calls carried reached the store: only the accepted rows did.
    let listed = client.list(json!({ "MaxResults": 50 })).await;
    let names: Vec<String> = listed
        .body
        .get("WorkloadSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary
                .get("WorkloadName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        names.len(),
        accepted.len(),
        "case \"store\": only the accepted Regions may have created a workload, got {names:?}"
    );
    assert!(
        names.iter().all(|name| name.starts_with("ok-")),
        "case \"store\": a refused create must store nothing, got {names:?}"
    );

    // The same rule guards an update, and a refused update leaves the record alone.
    let mut request = minimal_create("update-region", "tok-update-region");
    request["AwsRegions"] = json!(["eu-west-1"]);
    let workload_id = created_id(&client.create(request).await, "update-region");
    for (case, region) in refused_regions {
        let answer = client
            .update(&workload_id, json!({ "AwsRegions": [region] }))
            .await;
        refused(&answer, &format!("update/{case}"));
        let fetched = client.get(&workload_id).await;
        assert_eq!(
            fetched
                .body
                .get("Workload")
                .and_then(|workload| workload.get("AwsRegions")),
            Some(&json!(["eu-west-1"])),
            "case {case:?}: a refused update must leave the stored Regions alone, got {fetched:?}"
        );
    }
}

/// A `NextToken` this service did not issue cannot be decoded at all, so it is
/// classified `CANNOT_PARSE` — "ask for a new token", not "fix this value" — while
/// still naming the member it could not decode. A token this service DID issue still
/// pages.
///
/// Rows differ along the undecodable token's form, each one satisfying the modelled
/// `NextToken` `@pattern("^[A-Za-z0-9+/=_-]+$")` so the refusal is about the token's
/// content and not about its characters.
#[tokio::test]
async fn an_undecodable_next_token_cannot_be_parsed() {
    let client = Client::new();
    for index in 0..3 {
        created_id(
            &client
                .create(minimal_create(&format!("token-{index}"), &format!("tok-{index}")))
                .await,
            "seed",
        );
    }

    // A token this service issued, to mutate the rows from and to prove paging still
    // works at the end.
    let first_page = client.list(json!({ "MaxResults": 1 })).await;
    let issued = first_page
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("three workloads and a page of one leave a NextToken")
        .to_string();

    let undecodable = [
        ("not-hex", "zzzz".to_string()),
        ("hex-but-not-a-cursor", "6162".to_string()),
        ("truncated-issued-token", issued[..issued.len() / 2].to_string()),
        ("issued-token-with-a-suffix", format!("{issued}00")),
        ("single-character", "a".to_string()),
    ];

    for (case, token) in undecodable {
        let answer = client.list(json!({ "NextToken": token })).await;
        let body = refused(&answer, case);
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an opaque token that will not decode is CANNOT_PARSE, got {body:?}"
        );
        assert_eq!(
            body.get("Fields")
                .and_then(Value::as_array)
                .map(|fields| fields
                    .iter()
                    .map(|field| field.get("Name").and_then(Value::as_str).unwrap_or_default())
                    .collect::<Vec<_>>()),
            Some(vec!["NextToken"]),
            "case {case:?}: the finding must name the member that could not be decoded, got {body:?}"
        );
    }

    // The issued token is still accepted, so the refusals above are about the token.
    let resumed = client.list(json!({ "NextToken": issued })).await;
    assert_eq!(
        resumed.status, 200,
        "case \"issued-token\": a token this service issued must page, got {resumed:?}"
    );
}

/// The 501 channel is for an operation NOBODY has written yet, and no operation of this
/// service is one: a well-formed request is SERVED, and one carrying a value the model does
/// not publish is refused with the modelled 400 — neither is answered from that channel.
#[tokio::test]
async fn a_request_this_service_serves_is_never_answered_from_the_501_channel() {
    let client = Client::new();
    for (case, method, path, body, expected) in [
        // `GetConsolidatedReport` — the consolidated report over the account's workloads —
        // was the last scaffold of this service and is now served
        // (`super::consolidated_report`), as are the operations the rows this one replaced
        // named: `GetLensReviewReport` (`super::lens_review_report`),
        // `ListProfileNotifications` (`super::profile_notifications`), `ListNotifications`
        // (`super::lens_notifications`) and `GetProfileTemplate`
        // (`super::profile_template`). The request is well formed — `Format` is a
        // published `ReportFormat` member — so nothing refuses it before the handler runs.
        (
            "GetConsolidatedReport",
            "GET",
            "/consolidatedReport?Format=JSON".to_string(),
            None,
            200,
        ),
        // The same operation with a value no `ReportFormat` publishes: refused at the
        // boundary with the modelled 400, which is not the 501 channel either.
        (
            "GetConsolidatedReport / an unpublished Format",
            "GET",
            "/consolidatedReport?Format=CSV".to_string(),
            None,
            400,
        ),
    ] {
        let answer = client.send(method, &path, body).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
    }
}
