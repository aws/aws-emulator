// SPDX-License-Identifier: CC0-1.0

//! Authorization precedence on a zonal-shift mutation's target.
//!
//! The rule under test, asserted over a TABLE of targets rather than one request:
//! an operation that acts on a target it names (`StartZonalShift` by
//! `resourceIdentifier`, `CancelZonalShift` and `UpdateZonalShift` by
//! `zonalShiftId`) has the caller's authorization on that target decided BEFORE the
//! target is resolved, so an identifier ARC holds nothing for is refused with
//! `AccessDeniedException` — `@error client`, `@httpError 403`, a service-common
//! error of `PercDataPlane` — and the refusal discloses nothing about whether the
//! identifier names anything (never `ResourceNotFoundException`, 404).
//!
//! What such a refusal STATES depends on what the call named: a denial taken on a
//! `resourceIdentifier` states ARC's reason about that resource, and a denial taken on a
//! `zonalShiftId` — which is not a resource an IAM policy grants an action on, so there
//! is no resolved target to report about — states no reason at all, the model's
//! `AccessDeniedException$message` being optional. That rule is the subject of
//! `refusal_disclosure.rs`; here it is one assertion per row.
//!
//! The rows differ in the FORM and the SIZE of the identifier (ARN and non-ARN,
//! other account, other Region, each end of the modelled length range, letters,
//! digits and hyphens) and in the request payload carried with it, so what is
//! pinned is the branch and not one recorded value. The boundary case is here too:
//! a target the store DOES hold is authorized, so the same operations proceed —
//! the refusal is computed from the store, not returned unconditionally.
//!
//! The same ordering decides the branch a REPORTING operation on a named target
//! takes, which is where the two refusals separate: `GetManagedResource` can only be
//! authorized on an identifier shaped like a resource ARN (`ResourceArn`'s
//! `@pattern`, `^arn:.*$`), because that is what an IAM policy grants an ARC action
//! on, so an identifier that is not ARN-shaped is denied 403 before the store is
//! consulted, while an ARN-shaped identifier ARC holds no record of is resolved and
//! reported as `ResourceNotFoundException` (404).

use super::zonal_shift_lifecycle::{
    call, emulator, encoded, error_code, refused, seeded, start_body,
};
use service_com_amazonaws_arczonalshift_percdataplane::state::TARGET_UNAVAILABLE_MESSAGE;

/// ARN-shaped `resourceIdentifier`s no managed resource carries, as
/// `(identifier, class)`.
///
/// "ARN-shaped" is what `ResourceArn`'s `@pattern` requires — `^arn:.*$`
/// (`smithy_query(query="shape", shape="ResourceArn")`) — so each row names a
/// resource that could have been granted to a caller and simply is not registered.
const UNKNOWN_RESOURCE_ARNS: &[(&str, &str)] = &[
    (
        "arn:aws:elasticloadbalancing:us-east-1:111111111111:loadbalancer/app/absent/73e2d6bc24d8a067",
        "a load balancer ARN in another account",
    ),
    (
        "arn:aws:autoscaling:us-west-2:000000000000:autoScalingGroupName/absent",
        "an Auto Scaling group ARN in another Region",
    ),
    (
        "arn:aws:eks:us-east-1:111111111111:cluster/absent",
        "an EKS cluster ARN ARC has no record of",
    ),
    (
        "arn:this-is-not-a-well-formed-arn",
        "an ARN-prefixed value with no further structure",
    ),
];

/// `resourceIdentifier`s that are not shaped like a resource ARN at all, as
/// `(identifier, class)`.
///
/// The sizes come from the model: `ResourceIdentifier` is length 8..1024 with no
/// pattern (`smithy_query(query="shape", shape="ResourceIdentifier")`), so the
/// shortest value it accepts is 8 characters and a value that is not an ARN at all
/// still reaches the handler. `ResourceArn`'s `^arn:.*$` is case-sensitive, so an
/// upper-cased prefix is not ARN-shaped either.
const NON_ARN_RESOURCE_IDENTIFIERS: &[(&str, &str)] = &[
    ("abcdefgh", "the shortest identifier the model accepts"),
    ("not-an-arn-at-all", "a value that is not an ARN"),
    ("zonal-shift-alb", "a resource name rather than its ARN"),
    (
        "ARN:aws:eks:us-east-1:111111111111:cluster/absent",
        "an upper-cased arn: prefix, which the pattern does not admit",
    ),
    (
        "i-0123456789abcdef0",
        "an instance id: an AWS identifier that is not an ARN",
    ),
];

/// Every identifier no managed resource carries, ARN-shaped or not: what a MUTATION
/// refuses alike, whichever form the target took.
fn unknown_resource_identifiers() -> Vec<(&'static str, &'static str)> {
    UNKNOWN_RESOURCE_ARNS
        .iter()
        .chain(NON_ARN_RESOURCE_IDENTIFIERS)
        .copied()
        .collect()
}

/// `zonalShiftId`s no stored shift carries, as `(id, class)`.
///
/// The sizes and the alphabet come from the model: `ZonalShiftId` is length 6..36
/// with pattern `^[A-Za-z0-9-]+$` (`smithy_query(query="shape",
/// shape="ZonalShiftId")`), so each end of that range and each kind of character it
/// admits is a row.
pub(crate) const UNKNOWN_ZONAL_SHIFT_IDS: &[(&str, &str)] = &[
    (
        "7f1b3c2d-4e5a-4b6c-8d9e-0a1b2c3d4e5f",
        "a uuid-shaped id, the form this service mints",
    ),
    ("zs-0000009999", "a prefixed, zero-padded id"),
    ("abcdef", "the shortest id the model accepts"),
    (
        "A1b2C3d4E5f6A1b2C3d4E5f6A1b2C3d4E5f6",
        "the longest id the model accepts",
    ),
    ("Zs-UNKNOWN-0001", "mixed case with hyphens"),
    ("123456", "digits only"),
];

/// The payloads an `UpdateZonalShift` row carries: the refusal does not depend on
/// what the caller asked to change.
const UPDATE_PAYLOADS: &[&str] = &[
    "{}",
    r#"{"comment":"a new comment"}"#,
    r#"{"expiresIn":"30m"}"#,
    r#"{"comment":"both","expiresIn":"2h"}"#,
];

/// Every refusal of an unauthorized target looks the same: 403, the
/// `AccessDeniedException` code an SDK dispatches on, and a body that discloses
/// nothing about the target it was asked about. `reason` is what ARC states about it:
/// the resource message for a call that named a resource, nothing at all for one that
/// named a shift id.
///
/// ARC answers a denial from the layer that decides authorization, so the class is named
/// in `x-amzn-errortype` and the document carries the text alone, under `Message`
/// (`access_denial_envelope.rs` states that rule over the whole class).
fn assert_refused(
    case: &str,
    target: &str,
    reason: Option<&str>,
    status: u16,
    class: &str,
    body: &serde_json::Value,
) {
    assert_eq!(
        status, 403,
        "case {case:?}: an unauthorized target is refused 403, got {status} {body}"
    );
    assert_eq!(
        class, "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got {body}"
    );
    let rendered = body.to_string();
    assert!(
        !rendered.contains(target),
        "case {case:?}: the refusal must not disclose the target {target:?}, got {rendered}"
    );
    match reason {
        Some(expected) => assert_eq!(
            body["Message"].as_str(),
            Some(expected),
            "case {case:?}: expected the non-disclosing refusal message {expected:?}, \
             got {body}"
        ),
        None => assert!(
            body["Message"].is_null() || body["Message"].as_str() == Some(""),
            "case {case:?}: a denial on a shift id states no reason, got {body}"
        ),
    }
}

#[tokio::test]
async fn starting_a_shift_for_a_target_arc_holds_nothing_for_is_refused_403() {
    let (router, engine) = emulator();
    let mut targets: Vec<(String, String)> = unknown_resource_identifiers()
        .iter()
        .map(|(identifier, class)| ((*identifier).to_string(), (*class).to_string()))
        .collect();
    // A near miss on a target the store DOES hold: the same ARN with one more
    // character is still a target ARC holds nothing for.
    let (held, zones) = seeded(&engine)[0].clone();
    targets.push((
        format!("{held}0"),
        "a near miss on an identifier the store holds".to_string(),
    ));

    // The zone and the length of time are varied across the rows as well, so the
    // branch is shown to be taken whatever else the request asks for.
    let shapes: [(&str, &str); 3] = [
        (&zones[0], "1h"),
        ("use1-az1", "4320m"),
        ("usw2-az3", "15m"),
    ];
    for (row, (target, class)) in targets.iter().enumerate() {
        let (away_from, expires_in) = shapes[row % shapes.len()];
        let case = format!("StartZonalShift/{class}/awayFrom={away_from}");
        let (status, class, body) = refused(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(target, away_from, expires_in, "shifting an unheld target"),
        )
        .await;
        assert_refused(
            &case,
            target,
            Some(TARGET_UNAVAILABLE_MESSAGE),
            status,
            &class,
            &body,
        );
    }
}

#[tokio::test]
async fn cancelling_or_updating_an_id_arc_holds_nothing_for_is_refused_403() {
    let (router, _engine) = emulator();
    for (row, (id, class)) in UNKNOWN_ZONAL_SHIFT_IDS.iter().enumerate() {
        let (status, answered, body) =
            refused(&router, "DELETE", &format!("/zonalshifts/{id}"), "").await;
        assert_refused(
            &format!("CancelZonalShift/{class}"),
            id,
            None,
            status,
            &answered,
            &body,
        );

        let payload = UPDATE_PAYLOADS[row % UPDATE_PAYLOADS.len()];
        let (status, answered, body) =
            refused(&router, "PATCH", &format!("/zonalshifts/{id}"), payload).await;
        assert_refused(
            &format!("UpdateZonalShift/{class}/payload={payload}"),
            id,
            None,
            status,
            &answered,
            &body,
        );
    }
}

#[tokio::test]
async fn an_id_the_store_held_and_dropped_is_refused_the_same_way() {
    // The refusal is read from the store, so an id that WAS valid is refused once the
    // store no longer holds it — the same branch, reached by a different route.
    let (router, engine) = emulator();
    let (resource, zones) = seeded(&engine)[0].clone();
    let (status, started) = call(
        &router,
        "POST",
        "/zonalshifts",
        &start_body(&resource, &zones[0], "1h", "a shift to forget"),
    )
    .await;
    assert_eq!(status, 201, "the shift starts, got {status} {started}");
    let id = started["zonalShiftId"]
        .as_str()
        .expect("zonalShiftId is required")
        .to_string();

    // No modelled operation drops a record, so the store is edited directly — the
    // same seam the clock-driven case in the lifecycle suite uses.
    engine
        .state::<service_com_amazonaws_arczonalshift_percdataplane::ComAmazonawsArczonalshiftPercdataplaneState>()
        .lock()
        .zonal_shifts
        .remove(&id);

    for method in ["DELETE", "PATCH"] {
        let body_text = if method == "PATCH" { "{}" } else { "" };
        let (status, class, body) =
            refused(&router, method, &format!("/zonalshifts/{id}"), body_text).await;
        assert_refused(
            &format!("{method} a dropped id"),
            &id,
            None,
            status,
            &class,
            &body,
        );
    }
}

#[tokio::test]
async fn a_target_the_store_holds_is_authorized_and_the_mutation_proceeds() {
    let (router, engine) = emulator();
    for (resource, zones) in seeded(&engine) {
        for away_from in &zones {
            let case = format!("{resource}/{away_from}");
            let (status, started) = call(
                &router,
                "POST",
                "/zonalshifts",
                &start_body(&resource, away_from, "1h", "a held target"),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: a target the store holds is authorized, got {status} {started}"
            );
            let id = started["zonalShiftId"]
                .as_str()
                .expect("zonalShiftId is required")
                .to_string();

            // Read back through an implemented reader: the shift the authorized call
            // created is persisted.
            let (status, view) = call(
                &router,
                "GET",
                &format!("/managedresources/{}", encoded(&resource)),
                "",
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetManagedResource answers 200, got {status} {view}"
            );
            let shifts = view["zonalShifts"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: zonalShifts is required, got {view}"));
            assert!(
                shifts
                    .iter()
                    .any(|shift| shift["zonalShiftId"].as_str() == Some(id.as_str())),
                "case {case:?}: the started shift {id} is reported, got {view}"
            );

            let (status, updated) = call(
                &router,
                "PATCH",
                &format!("/zonalshifts/{id}"),
                r#"{"comment":"still held"}"#,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: updating a held shift answers 200, got {status} {updated}"
            );
            assert_eq!(
                updated["comment"], "still held",
                "case {case:?}: the comment is replaced, got {updated}"
            );

            let (status, canceled) =
                call(&router, "DELETE", &format!("/zonalshifts/{id}"), "").await;
            assert_eq!(
                status, 200,
                "case {case:?}: cancelling a held shift answers 200, got {status} {canceled}"
            );

            // The id is still one the store holds, so acting on it again is refused on
            // the SHIFT'S STATE (409) — not on authorization.
            for (method, payload) in [("DELETE", ""), ("PATCH", "{}")] {
                let (status, body) =
                    call(&router, method, &format!("/zonalshifts/{id}"), payload).await;
                assert_eq!(
                    status, 409,
                    "case {case:?}: {method} on a shift that is over conflicts, \
                     got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ConflictException",
                    "case {case:?}: {method} expected ConflictException, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_read_of_an_arn_shaped_resource_arc_has_no_record_of_is_reported_as_not_found() {
    // The boundary of the rule: an ARN-shaped identifier is one authorization CAN be
    // decided on, so `GetManagedResource` resolves it, and the model declares
    // `ResourceNotFoundException` (404) for a resource ARC holds no record of.
    let (router, engine) = emulator();
    let mut arns: Vec<(String, String)> = UNKNOWN_RESOURCE_ARNS
        .iter()
        .map(|(identifier, class)| ((*identifier).to_string(), (*class).to_string()))
        .collect();
    // A near miss on an ARN the store DOES hold: still ARN-shaped, still unregistered.
    let (held, _zones) = seeded(&engine)[0].clone();
    arns.push((
        format!("{held}0"),
        "a near miss on an ARN the store holds".to_string(),
    ));
    for (identifier, class) in &arns {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/managedresources/{}", encoded(identifier)),
            "",
        )
        .await;
        assert_eq!(
            status, 404,
            "case {class:?}: GetManagedResource is not found, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {class:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_read_of_an_identifier_that_is_not_arn_shaped_is_denied_403() {
    // The rule's other side: authorization on the named target is decided BEFORE the
    // target is resolved, and an identifier that is not shaped like a resource ARN
    // (`ResourceArn`'s `@pattern`, `^arn:.*$`) matches no resource ARN an IAM policy
    // could have granted this action on — so the read is denied rather than looked up,
    // and never reports whether anything is registered.
    let (router, _engine) = emulator();
    for (identifier, class) in NON_ARN_RESOURCE_IDENTIFIERS {
        let (status, answered, body) = refused(
            &router,
            "GET",
            &format!("/managedresources/{}", encoded(identifier)),
            "",
        )
        .await;
        assert_eq!(
            status, 403,
            "case {class:?}: a non-ARN identifier is denied 403, got {status} {body}"
        );
        assert_eq!(
            answered, "AccessDeniedException",
            "case {class:?}: expected AccessDeniedException, got {body}"
        );
        // `AccessDeniedException$message` is optional, and a denial taken before the
        // target was resolved reports nothing about it.
        assert!(
            body["Message"].is_null() || body["Message"].as_str() == Some(""),
            "case {class:?}: the denial carries no message, got {body}"
        );
        assert!(
            !body.to_string().contains(identifier),
            "case {class:?}: the denial must not disclose the identifier, got {body}"
        );
    }
}
