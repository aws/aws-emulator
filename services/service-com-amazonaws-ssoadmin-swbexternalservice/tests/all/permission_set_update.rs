// SPDX-License-Identifier: CC0-1.0

//! `UpdatePermissionSet` end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: `UpdatePermissionSet` writes exactly the updatable members the
//! request CARRIES (`Description`, `RelayState`, `SessionDuration`) onto the one stored
//! permission set its `(InstanceArn, PermissionSetArn)` pair names, leaves every member
//! the request omits at its stored value, touches no other permission set and changes
//! neither the `Name` nor the `CreatedDate` nor the ARN the create minted; every later
//! `DescribePermissionSet` answers from that one record. A request whose target instance
//! or permission set does not exist is a missing resource, a request outside the modelled
//! constraints is refused, and a refused request writes nothing.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: WHICH of the three members the request carries, the published length bounds of
//! `Description` (1..700) and `RelayState` (1..240), the character classes both admit, and
//! the ISO-8601 forms the `Duration` pattern admits — not over one example.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `X-Amz-Target` routing are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance (asserted here too).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    account_instance_router, assert_refused, call, organization_router, UNKNOWN_INSTANCE_ARN,
    UNKNOWN_PERMISSION_SET_ARN,
};

/// `Description`'s published upper length bound (`PermissionSetDescription`,
/// `length(min: 1, max: 700)`).
const DESCRIPTION_MAX: usize = 700;

/// `RelayState`'s published upper length bound (`RelayState`, `length(min: 1, max: 240)`).
const RELAY_STATE_MAX: usize = 240;

/// `SessionDuration`'s published upper length bound (`Duration`,
/// `length(min: 1, max: 100)`).
const SESSION_DURATION_MAX: usize = 100;

/// An ISO-8601 duration of exactly `length` characters: `PT`, a run of digits, `H` — the
/// shape the model's `Duration` pattern
/// `^(-?)P(?=\d|T\d)(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)([DW]))?(?:T(?:(\d+)H)?…)?$` admits, so a
/// length bound can be hit without leaving the pattern.
fn hours_of_length(length: usize) -> String {
    // `PT` and `H` are three of the characters, so the digit run needs the rest.
    debug_assert!(length >= 4, "a PT..H duration is at least 4 characters");
    format!("PT{}H", "9".repeat(length - 3))
}

/// A duration of exactly `length` characters naming ONE HOUR: `PT`, 3600 seconds padded
/// with leading zeros, `S`.
///
/// The digit run of the same `(\d+)S` component the pattern admits, so the published length
/// bound can be hit by a value that is also inside the session-duration window the service
/// serves (1 hour to 12 hours) — [`hours_of_length`] fills the digits with nines and so
/// names a span far outside it.
fn padded_hour_of_length(length: usize) -> String {
    // `PT` and `S` are three of the characters, and `3600` is four more.
    debug_assert!(length >= 7, "a PT..S duration of one hour is at least 7 characters");
    format!("PT{}3600S", "0".repeat(length - 7))
}

/// The values the create stores, so every row below starts from a permission set that
/// already carries all three updatable members and an omitted member has something to be
/// left alone.
const BASE_DESCRIPTION: &str = "The description the create stored";
const BASE_RELAY_STATE: &str = "https://example.com/start";
const BASE_SESSION_DURATION: &str = "PT1H";

/// The three members `UpdatePermissionSet` can write, as the model names them.
const UPDATABLE_MEMBERS: [&str; 3] = ["Description", "RelayState", "SessionDuration"];

/// One row of the update table: the `UpdatePermissionSet` members the request carries.
struct UpdateCase {
    label: &'static str,
    sent: Value,
}

/// The rows written out: which members are present, and the value forms each member's
/// modelled pattern admits.
fn written_update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "no updatable member at all",
            sent: json!({}),
        },
        UpdateCase {
            label: "Description only",
            sent: json!({"Description": "Read-only access for auditors"}),
        },
        UpdateCase {
            label: "RelayState only",
            sent: json!({"RelayState": "https://console.aws.amazon.com/s3/home"}),
        },
        UpdateCase {
            label: "SessionDuration only",
            sent: json!({"SessionDuration": "PT8H"}),
        },
        UpdateCase {
            label: "all three members at once",
            sent: json!({
                "Description": "Break-glass administrator access",
                "RelayState": "https://console.aws.amazon.com/cloudwatch/home",
                "SessionDuration": "PT12H",
            }),
        },
        UpdateCase {
            label: "Description and SessionDuration, no RelayState",
            sent: json!({
                "Description": "Support engineer access",
                "SessionDuration": "PT4H30M",
            }),
        },
        UpdateCase {
            // `Description` at its lower length bound (1..700).
            label: "Description one character long",
            sent: json!({"Description": "x"}),
        },
        UpdateCase {
            // `PermissionSetDescription`'s class is
            // `^[\u0009\u000A\u000D\u0020-\u007E\u00A1-\u00FF]*$`, so printable ASCII,
            // tab/newline/carriage return and the Latin-1 supplement.
            label: "Description over the full character class",
            sent: json!({"Description": "tab:\there\nline two\r¡Año! café ÿ ~{}[]"}),
        },
        UpdateCase {
            // `Duration`'s pattern admits a day component before the time components.
            label: "SessionDuration with a day component",
            sent: json!({"SessionDuration": "P0DT6H"}),
        },
        UpdateCase {
            // The session-duration window's lower bound, named in seconds.
            label: "SessionDuration in seconds",
            sent: json!({"SessionDuration": "PT3600S"}),
        },
        UpdateCase {
            label: "SessionDuration with every time component",
            sent: json!({"SessionDuration": "PT2H15M30S"}),
        },
        UpdateCase {
            // `RelayState` at its lower length bound (1..240); `/` is in its class.
            label: "RelayState one character long",
            sent: json!({"RelayState": "/"}),
        },
        UpdateCase {
            // `RelayState`'s class is
            // `^[a-zA-Z0-9&$@#\\\/%?=~\-_'"|!:,.;*+\[\]\ \(\)\{\}]+$`.
            label: "RelayState over the full character class",
            sent: json!({"RelayState": "aZ9&$@#\\/%?=~-_'\"|!:,.;*+[] (){}"}),
        },
    ]
}

/// The rows whose values are BUILT: each of the two length-bounded members exactly AT its
/// published upper bound.
fn update_cases_at_upper_bounds() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "Description at its upper length bound",
            sent: json!({"Description": "d".repeat(DESCRIPTION_MAX)}),
        },
        UpdateCase {
            label: "RelayState at its upper length bound",
            sent: json!({"RelayState": "r".repeat(RELAY_STATE_MAX)}),
        },
        UpdateCase {
            label: "SessionDuration at its upper length bound",
            sent: json!({"SessionDuration": padded_hour_of_length(SESSION_DURATION_MAX)}),
        },
    ]
}

/// Every row the write/leave-alone rule is asserted over.
fn every_update_case() -> Vec<UpdateCase> {
    let mut cases = written_update_cases();
    cases.extend(update_cases_at_upper_bounds());
    cases
}

/// Create a permission set named `name` carrying all three updatable members, and return
/// the `PermissionSet` the create answered with.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> Value {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "Name": name,
            "Description": BASE_DESCRIPTION,
            "RelayState": BASE_RELAY_STATE,
            "SessionDuration": BASE_SESSION_DURATION,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSet"].clone()
}

/// The ARN a create minted, read off its own answer.
fn minted_arn(created: &Value) -> String {
    created["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no minted PermissionSetArn in {created}"))
        .to_string()
}

/// `DescribePermissionSet`'s `PermissionSet`, which must answer 200.
async fn describe(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> Value {
    let (status, body) = call(
        router,
        "DescribePermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribePermissionSet expected 200 for {permission_set_arn}, got {status} {body}"
    );
    body["PermissionSet"].clone()
}

/// Just the three updatable members of a `PermissionSet`, as a comparable object: an
/// absent member reads as `null`.
fn updatable(permission_set: &Value) -> Value {
    let mut members = serde_json::Map::new();
    for member in UPDATABLE_MEMBERS {
        members.insert(member.to_string(), permission_set[member].clone());
    }
    Value::Object(members)
}

/// What the three updatable members must read as after an update carrying `sent` is
/// applied to a permission set whose members currently read as `previous`: the sent value
/// where the request carries the member, the stored one where it does not.
fn expected_after(previous: &Value, sent: &Value) -> Value {
    let mut expected = previous.clone();
    for member in UPDATABLE_MEMBERS {
        if let Some(value) = sent.get(member) {
            expected[member] = value.clone();
        }
    }
    expected
}

/// An `UpdatePermissionSet` call carrying `sent` plus the two required ARNs.
async fn update(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    sent: &Value,
) -> (u16, Value) {
    let mut body = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    for (member, value) in sent.as_object().expect("the row is an object") {
        body[member] = value.clone();
    }
    call(router, "UpdatePermissionSet", &body).await
}

/// THE RULE: an update writes the members it carries and leaves the rest at their stored
/// values — over every row, each on its own permission set — and the identity the create
/// minted (ARN, `Name`, `CreatedDate`) is untouched.
#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_leaves_the_rest_alone() {
    let (router, instance_arn) = organization_router();

    for (index, case) in every_update_case().into_iter().enumerate() {
        let name = format!("Updated{index}");
        let created = create_permission_set(&router, &instance_arn, &name).await;
        let permission_set_arn = minted_arn(&created);

        let before = describe(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            updatable(&before),
            updatable(&created),
            "case {:?}: the create and the first describe must agree, got {before}",
            case.label
        );

        let (status, body) = update(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdatePermissionSet expected 200, got {status} {body}",
            case.label
        );

        let after = describe(&router, &instance_arn, &permission_set_arn).await;
        let expected = expected_after(&updatable(&before), &case.sent);
        assert_eq!(
            updatable(&after),
            expected,
            "case {:?}: expected the updated members {expected}, got {after}",
            case.label
        );

        // The update's input carries no `Name` and no `CreatedDate`, so neither moves, and
        // the ARN is the one the create minted.
        assert_eq!(
            after["PermissionSetArn"], created["PermissionSetArn"],
            "case {:?}: the ARN must not change, got {after}",
            case.label
        );
        assert_eq!(
            after["Name"],
            json!(name),
            "case {:?}: the Name must not change, got {after}",
            case.label
        );
        assert_eq!(
            after["CreatedDate"], created["CreatedDate"],
            "case {:?}: the CreatedDate must not change, got {after}",
            case.label
        );

        // A read changes nothing a later caller sees: a second describe agrees.
        let again = describe(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            again, after,
            "case {:?}: repeated describes must agree, got {after} then {again}",
            case.label
        );
    }
}

/// The stored record is the one the updates accumulate on: walking the whole table over ONE
/// permission set, each describe reports the newest value of every member any earlier row
/// wrote. Applying the same row twice is the same stored record (an update SETS).
#[tokio::test]
async fn updates_accumulate_on_the_one_stored_permission_set() {
    let (router, instance_arn) = organization_router();
    let created = create_permission_set(&router, &instance_arn, "Accumulating").await;
    let permission_set_arn = minted_arn(&created);
    let mut expected = updatable(&created);

    for case in every_update_case() {
        for round in 0..2 {
            let (status, body) =
                update(&router, &instance_arn, &permission_set_arn, &case.sent).await;
            assert_eq!(
                status, 200,
                "case {:?} round {round}: UpdatePermissionSet expected 200, got {status} {body}",
                case.label
            );
            expected = expected_after(&expected, &case.sent);

            let after = describe(&router, &instance_arn, &permission_set_arn).await;
            assert_eq!(
                updatable(&after),
                expected,
                "case {:?} round {round}: expected {expected}, got {after}",
                case.label
            );
        }
    }
}

/// An update changes only the permission set its `PermissionSetArn` names: every other
/// stored permission set of the same instance reads exactly as its create left it.
#[tokio::test]
async fn an_update_changes_only_the_permission_set_it_names() {
    let (router, instance_arn) = organization_router();
    let target = create_permission_set(&router, &instance_arn, "Target").await;
    let target_arn = minted_arn(&target);
    let bystander = create_permission_set(&router, &instance_arn, "Bystander").await;
    let bystander_arn = minted_arn(&bystander);

    for case in every_update_case() {
        let (status, body) = update(&router, &instance_arn, &target_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdatePermissionSet expected 200, got {status} {body}",
            case.label
        );

        let untouched = describe(&router, &instance_arn, &bystander_arn).await;
        assert_eq!(
            updatable(&untouched),
            updatable(&bystander),
            "case {:?}: the permission set the update did not name must be unchanged, got \
             {untouched}",
            case.label
        );
        assert_eq!(
            untouched["Name"],
            json!("Bystander"),
            "case {:?}: the bystander's Name must not change, got {untouched}",
            case.label
        );
    }
}

/// An `InstanceArn` no instance is held under, a `PermissionSetArn` never minted, and a
/// permission set already deleted are all missing resources — never a 200 — and the
/// permission set that does exist is left exactly as it was.
#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
    let (router, instance_arn) = organization_router();
    let present = create_permission_set(&router, &instance_arn, "Present").await;
    let present_arn = minted_arn(&present);

    let deleted = create_permission_set(&router, &instance_arn, "Deleted").await;
    let deleted_arn = minted_arn(&deleted);
    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": deleted_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeletePermissionSet expected 200, got {status} {body}"
    );

    let rows: Vec<(String, String, String)> = vec![
        (
            "an InstanceArn no instance is held under".to_string(),
            UNKNOWN_INSTANCE_ARN.to_string(),
            present_arn.clone(),
        ),
        (
            "a PermissionSetArn never minted".to_string(),
            instance_arn.clone(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
        (
            "a deleted PermissionSetArn".to_string(),
            instance_arn.clone(),
            deleted_arn,
        ),
    ];

    let sent = json!({"Description": "Should never be stored", "SessionDuration": "PT3H"});
    for (label, instance, permission_set) in rows {
        let (status, body) = update(&router, &instance, &permission_set, &sent).await;
        assert_refused(
            &label,
            "UpdatePermissionSet",
            "ResourceNotFoundException",
            status,
            &body,
        );

        let untouched = describe(&router, &instance_arn, &present_arn).await;
        assert_eq!(
            updatable(&untouched),
            updatable(&present),
            "case {label:?}: a refused update must store nothing, got {untouched}"
        );
    }
}

/// A value outside a member's modelled constraints is refused with `ValidationException`,
/// and the refusal writes nothing: every member keeps the value the create stored. Each
/// length-bounded member is asserted ONE PAST its published bound (and AT it in the
/// round-trip table above) and below its lower bound.
#[tokio::test]
async fn an_update_outside_the_modeled_constraints_is_refused_and_stores_nothing() {
    let (router, instance_arn) = organization_router();
    let created = create_permission_set(&router, &instance_arn, "Constrained").await;
    let permission_set_arn = minted_arn(&created);

    let rows: Vec<(String, Value)> = vec![
        (
            "Description one character past its upper bound".to_string(),
            json!({"Description": "d".repeat(DESCRIPTION_MAX + 1)}),
        ),
        (
            "Description below its lower bound (empty)".to_string(),
            json!({"Description": ""}),
        ),
        (
            "RelayState one character past its upper bound".to_string(),
            json!({"RelayState": "r".repeat(RELAY_STATE_MAX + 1)}),
        ),
        (
            "RelayState below its lower bound (empty)".to_string(),
            json!({"RelayState": ""}),
        ),
        (
            // `RelayState`'s class admits none of `<`, `>` or backtick.
            "a RelayState the pattern does not admit".to_string(),
            json!({"RelayState": "<script>"}),
        ),
        (
            "a SessionDuration one character past its upper bound".to_string(),
            json!({"SessionDuration": hours_of_length(SESSION_DURATION_MAX + 1)}),
        ),
        (
            "a SessionDuration below its lower bound (empty)".to_string(),
            json!({"SessionDuration": ""}),
        ),
        (
            // A refusal is a refusal for the WHOLE request: the valid member beside the
            // invalid one is not stored either.
            "a valid member beside an invalid one".to_string(),
            json!({"Description": "Valid", "RelayState": "<script>"}),
        ),
    ];

    for (label, sent) in rows {
        let (status, body) = update(&router, &instance_arn, &permission_set_arn, &sent).await;
        assert_refused(
            &label,
            "UpdatePermissionSet",
            "ValidationException",
            status,
            &body,
        );

        let after = describe(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            updatable(&after),
            updatable(&created),
            "case {label:?}: a refused update must store nothing, got {after}"
        );
    }
}

/// `UpdatePermissionSet` belongs to the permission-set surface, which is WITHDRAWN on an
/// account instance: there it answers the modelled `ValidationException` naming account
/// instances, before any resource lookup.
#[tokio::test]
async fn the_operation_is_refused_on_an_account_instance() {
    let (router, account_instance_arn) = account_instance_router().await;
    let permission_set_arn = format!(
        "arn:aws:sso:::permissionSet/{}/ps-0123456789abcdef",
        account_instance_arn.rsplit('/').next().unwrap_or_default()
    );

    for case in every_update_case() {
        let (status, body) = update(
            &router,
            &account_instance_arn,
            &permission_set_arn,
            &case.sent,
        )
        .await;
        assert_refused(
            case.label,
            "UpdatePermissionSet",
            "ValidationException",
            status,
            &body,
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains("account instances"),
            "case {:?}: the refusal must say the operation is unsupported for account \
             instances, got {message:?}",
            case.label
        );
    }
}
