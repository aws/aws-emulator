// SPDX-License-Identifier: CC0-1.0

//! Whether the `@timestampFormat("date-time")` wire form is checked EVERYWHERE this
//! service can report one.
//!
//! RULE: a structure member whose target carries `@timestampFormat("date-time")` travels
//! on this service's restJson1 wire as an RFC 3339 / ISO-8601 date-time STRING, never as
//! the epoch NUMBER the protocol's default timestamp encoding produces. The rule is about
//! the MEMBER, so it reaches every structure the model marks — not only the ones a list
//! written by hand happens to name.
//!
//! `iso8601_timestamp_members.rs` asserts the rule shape by shape over such a list
//! ([`shapes_under_test`]), and that list is the weak link. The JSON codec picks a
//! timestamp's rendering from the MEMBER SCHEMA's own format
//! (`vendor/smithy-schema-server/src/json.rs::write_timestamp`) and falls back to epoch
//! seconds when the schema reports none, so a marked shape nobody checks reports no format
//! and every response carrying it goes onto the wire as a number. This suite derives the
//! set of shapes the rule reaches FROM THE SHIPPED MODEL — by walking every operation's
//! output closure, the same way a response is assembled — and fails when one of them is
//! outside that list. The error side carries no instant to render, which is asserted here
//! rather than assumed.
//!
//! One shape in that set is deliberately outside it, and the second test keeps that
//! exception honest on the wire: the service must report no instant for it at all.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, emulator};
use crate::domain_name_lifecycle::{
    call, certificate_arn, configurations, create_domain_name, get_domain_name,
};
use crate::iso8601_timestamp_members::{is_date_time, model, shapes_under_test, NAMESPACE};

/// The wire key of the one `date-time` member no response of this service carries.
const UPLOAD_DATE_MEMBER: &str = "certificateUploadDate";

/// The shapes the model marks that the hand-written list deliberately leaves out, each
/// with the fact that keeps the member off the wire.
const OUTSIDE_THE_LIST: &[(&str, &str)] = &[(
    "DomainNameConfiguration",
    "no response reports its CertificateUploadDate — nothing is uploaded here, so \
     `state::DomainNameEndpoint::to_wire` leaves the member unset — and the shape a \
     REQUEST also carries keeps the derived schema that enforces its members' constraints",
)];

/// The shape ids one shape points at: its members' targets, a list's element, a map's key
/// and its value.
fn targets_of(shape: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |entry: Option<&Value>| {
        if let Some(target) = entry
            .and_then(|entry| entry.get("target"))
            .and_then(Value::as_str)
        {
            out.push(target.to_string());
        }
    };
    if let Some(members) = shape.get("members").and_then(Value::as_object) {
        for member in members.values() {
            push(Some(member));
        }
    }
    push(shape.get("member"));
    push(shape.get("key"));
    push(shape.get("value"));
    out
}

/// Which side of an operation a closure walk starts from.
#[derive(Clone, Copy, Debug)]
enum Side {
    /// The operation's `output` — the shapes a response body is assembled from.
    Output,
    /// The operation's `errors` — the shapes a failure body is assembled from.
    Errors,
}

/// Every shape id reachable from one SIDE of every operation the model declares.
///
/// The walk follows exactly the edges a body is assembled along — a structure's members, a
/// list's element, a map's key and value — so a shape it reaches is a shape some body of
/// that side can carry.
fn closure(model: &Value, side: Side) -> BTreeSet<String> {
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the model must declare shapes");

    let mut queue: Vec<String> = shapes
        .values()
        .filter(|shape| shape.get("type").and_then(Value::as_str) == Some("operation"))
        .flat_map(|operation| match side {
            Side::Output => operation
                .get("output")
                .and_then(|output| output.get("target"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .into_iter()
                .collect::<Vec<String>>(),
            Side::Errors => operation
                .get("errors")
                .and_then(Value::as_array)
                .map(|errors| {
                    errors
                        .iter()
                        .filter_map(|error| error.get("target").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default(),
        })
        .collect();
    assert!(
        !queue.is_empty(),
        "the model must bind at least one operation with a {side:?} side"
    );

    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(shape) = shapes.get(&id) {
            queue.extend(targets_of(shape));
        }
    }
    seen
}

/// The structures and unions in `closure` that declare a `date-time` member, as local
/// name -> the member names that carry one.
fn marked_structures(model: &Value, closure: &BTreeSet<String>) -> BTreeMap<String, Vec<String>> {
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the model must declare shapes");

    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in closure {
        let Some(local) = id.strip_prefix(&format!("{NAMESPACE}#")) else {
            continue;
        };
        let Some(shape) = shapes.get(id) else {
            continue;
        };
        // A union's members are encoded the same way a structure's are, so one carrying a
        // `date-time` member is under the rule too.
        if !matches!(
            shape.get("type").and_then(Value::as_str),
            Some("structure") | Some("union")
        ) {
            continue;
        }
        let Some(members) = shape.get("members").and_then(Value::as_object) else {
            continue;
        };
        let marked: Vec<String> = members
            .iter()
            .filter(|(_, member)| is_date_time(model, member))
            .map(|(name, _)| name.clone())
            .collect();
        if !marked.is_empty() {
            out.insert(local.to_string(), marked);
        }
    }
    out
}

/// RULE, applied to the CHECK itself: every structure the model marks `date-time` that a
/// response can carry is one whose schema a case asserts the format on.
#[test]
fn every_modelled_date_time_structure_a_response_can_carry_is_checked() {
    let model = model();
    let marked = marked_structures(&model, &closure(&model, Side::Output));

    let checked: BTreeSet<&str> = shapes_under_test()
        .into_iter()
        .map(|(local, _)| local)
        .collect();
    let excused: BTreeSet<&str> = OUTSIDE_THE_LIST.iter().map(|(local, _)| *local).collect();

    // The walk really reached the shapes the rule is already checked on, so the loop below
    // is not passing by being empty: every name on the hand-written list is a shape this
    // walk rediscovers and the model marks.
    let reached: BTreeSet<&str> = marked.keys().map(String::as_str).collect();
    assert!(
        checked.is_subset(&reached),
        "the output closure must reach every shape a case already checks and the model \
         marks, missing {:?}",
        checked.difference(&reached).collect::<Vec<_>>()
    );

    for (local, members) in &marked {
        assert!(
            checked.contains(local.as_str()) || excused.contains(local.as_str()),
            "shape {local:?} declares the date-time member(s) {members:?}, so a response \
             carrying it must render them as RFC 3339 strings — but no case asserts the \
             format on its schema, which is what the JSON codec reads, so the member would \
             go onto the wire as an epoch number: add {local:?} to \
             iso8601_timestamp_members::shapes_under_test, or to OUTSIDE_THE_LIST with the \
             fact that keeps the member off the wire"
        );
    }

    // And every excuse still names a shape the model marks and a response can carry, so a
    // stale one fails here instead of hiding a shape nobody checks.
    for (local, reason) in OUTSIDE_THE_LIST {
        assert!(
            marked.contains_key(*local),
            "{local:?} is excused ({reason}), but no shape of that name in an operation's \
             output closure declares a date-time member any more: drop the entry"
        );
    }
}

/// The reason the walk above follows outputs only: no shape a FAILURE body is assembled
/// from declares a `date-time` member, so an error response carries no instant to render.
/// Asserted rather than assumed, so a timestamp added to a modelled error shape fails here
/// instead of slipping past the walk.
#[test]
fn no_modelled_error_shape_declares_a_date_time_member() {
    let model = model();
    let marked = marked_structures(&model, &closure(&model, Side::Errors));
    assert!(
        marked.is_empty(),
        "a modelled error shape now declares a date-time member ({marked:?}), so the \
         coverage walk must follow the error side as well as the output side"
    );
}

/// RULE on the one marked shape outside the list: the service reports no instant for it,
/// on every path that could carry one — which is what makes leaving it out sound.
///
/// The rows vary what the rule generalises over: how many configurations the domain name
/// carries and which of their members the request sets, including the one that names a
/// certificate by name as well as by ARN.
#[tokio::test]
async fn the_shape_outside_the_list_reports_no_instant() {
    let emulator = emulator();

    for (case, request) in [
        (
            "one configuration that names nothing itself",
            json!({
                "domainName": "no-instant-bare.example.com",
                "domainNameConfigurations": [{}]
            }),
        ),
        (
            "one configuration naming a certificate by ARN and by name",
            json!({
                "domainName": "no-instant-named.example.com",
                "domainNameConfigurations": [{
                    "certificateArn": certificate_arn("no-instant-named"),
                    "certificateName": "no-instant-cert"
                }]
            }),
        ),
        (
            "three configurations, each naming its own certificate",
            json!({
                "domainName": "no-instant-many.example.com",
                "domainNameConfigurations": [
                    {"certificateArn": certificate_arn("no-instant-first")},
                    {"certificateArn": certificate_arn("no-instant-second"), "endpointType": "EDGE"},
                    {"certificateArn": certificate_arn("no-instant-third"), "securityPolicy": "TLS_1_2"}
                ]
            }),
        ),
    ] {
        let (status, created) = create_domain_name(&emulator, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateDomainName, got {status} {created}"
        );
        let domain = as_str(&created, "domainName");

        let (status, fetched) = get_domain_name(&emulator, &domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDomainName, got {status} {fetched}"
        );

        let (status, listed) = call(&emulator, "GET", "/v2/domainnames", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDomainNames, got {status} {listed}"
        );
        let entry = listed
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| as_str(item, "domainName") == domain)
                    .cloned()
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: GetDomainNames must report {domain}, got {listed}")
            });

        for (path, body) in [
            ("CreateDomainName", &created),
            ("GetDomainName", &fetched),
            ("GetDomainNames item", &entry),
        ] {
            let entries = configurations(body);
            assert_eq!(
                entries.len(),
                request["domainNameConfigurations"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                "case {case:?}: {path} must report one configuration per requested one, got \
                 {body}"
            );
            for (index, configuration) in entries.iter().enumerate() {
                let reported = configuration.get(UPLOAD_DATE_MEMBER);
                assert!(
                    reported.is_none_or(Value::is_null),
                    "case {case:?}: {path} reports {UPLOAD_DATE_MEMBER} as {reported:?} on \
                     configuration {index}; the service claims no certificate upload, and \
                     this shape's schema carries no date-time format, so an instant here \
                     would go onto the wire as an epoch number"
                );
            }
        }
    }
}
