// SPDX-License-Identifier: CC0-1.0

//! What this awsQuery front door answers when a handler refuses a request PARAMETER, and
//! how it words the refusals whose text the service owns.
//!
//! Three rules, each asserted over a set of inputs rather than over one call:
//!
//! 1. A value outside what its own member accepts is refused HTTP 400 with the Query API
//!    common error code `InvalidParameterValue` — the code Amazon RDS answers that with
//!    and the one a client branches on. The framework's synthetic `ValidationException` is
//!    not an RDS error code, so no refusal of a parameter value may carry it. The table
//!    spans the AXES the rule generalises over: the member (seven of them, across creates,
//!    modifies, deletes and describes), the constraint broken (leading non-letter,
//!    trailing hyphen, doubled hyphen, an out-of-alphabet character, one past the length
//!    bound), the KIND of value (an identifier, an enumerated value list, a numeric range,
//!    a pagination token, a filter name) and the direction a bound is crossed.
//! 2. The members RDS words by their SUBJECT carry exactly `Invalid <subject>:  <value>`,
//!    with the value from the request and no reason — so one text answers whichever of the
//!    member's constraints the value broke.
//! 3. A `Marker` no prior page issued is refused in the fixed text RDS answers it with —
//!    the sentence that reflects nothing of the request back, which is what every report
//!    here answers with.
//! 4. A value that broke a constraint the MODEL declares — refused by the boundary
//!    validator, before any handler runs — carries that SAME `InvalidParameterValue` code,
//!    so one class of refusal has one code whichever layer found it. The table spans the
//!    constraint KINDS a member can declare (a numeric range below and above its bound, a
//!    string length, an enumerated value set) across four operations, with the published
//!    bound itself served to show the refusal is not blanket.
//!
//! A fifth test pins the two not-found wordings of the snapshot paths —
//! `CreateDBSnapshot` naming an absent DB instance and `DescribeDBSnapshots` naming an
//! absent snapshot — each with the modelled code and status beside it, so a row cannot
//! pass by failing for another reason, and reads a stored snapshot back so the absent
//! rows cannot pass on a read that refuses everything.
//!
//! The harness (router, call, XML element readers) is the one `db_instances` owns; only
//! the tables here are local.

use service_com_amazonaws_rds_amazonrdsv19::state::{invalid_marker_message, MAX_RECORDS_RANGE};

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, text, wait_for_settle, REGION,
};

/// The DB instance the rows that need an existing one are aimed at, so they are refused
/// for their parameter rather than for an absent resource.
const LIVE_INSTANCE: &str = "emu-prc-live";

/// One row of the parameter-value table: a request, and a fragment its refusal must carry
/// so the row cannot pass on a refusal of something else.
struct ValueRow {
    case: String,
    action: &'static str,
    params: Vec<(&'static str, String)>,
    says: String,
}

/// `CreateOptionGroup`'s other required members, for the rows that are about its name.
fn option_group_rest() -> Vec<(&'static str, String)> {
    vec![
        ("EngineName", "mysql".to_string()),
        ("MajorEngineVersion", "8.0".to_string()),
        ("OptionGroupDescription", "a group".to_string()),
    ]
}

/// The sentence pair Amazon RDS refuses a DB instance identifier with where the value is a
/// token of the member's shape that breaks one of the two hyphen rules: it names the member
/// at fault and states the whole rule, echoing nothing the request carried.
fn states_the_rule(member: &str) -> String {
    format!(
        "The parameter {member} is not a valid identifier. Identifiers must begin with a \
         letter; must contain only ASCII letters, digits, and hyphens; and must not end with \
         a hyphen or contain two consecutive hyphens."
    )
}

/// The rows of the parameter-value table.
///
/// The identifier constraints are the ones those members publish ("Must contain from 1 to
/// N letters, numbers, or hyphens. First character must be a letter. Can't end with a
/// hyphen or contain two consecutive hyphens"); the `EngineName` value list is
/// `CreateOptionGroupMessage$EngineName`'s own; `MaxRecords` is "Minimum 20, maximum 100";
/// the filter names are each operation's own documented set.
fn value_rows() -> Vec<ValueRow> {
    let mut rows = Vec::new();

    // One member, every constraint it publishes — the axis the identifier rule
    // generalises over. A value that is no identifier TOKEN of the member at all is worded
    // by the member's subject and echoes the value; a token that breaks one of the two
    // hyphen rules states the rule instead.
    for (case, identifier, hyphen_rule) in [
        ("a leading digit", "1emu-prc-db".to_string(), false),
        ("a leading hyphen", "-emu-prc-db".to_string(), false),
        ("a trailing hyphen", "emu-prc-db-".to_string(), true),
        ("two consecutive hyphens", "emu--prc-db".to_string(), true),
        (
            "an underscore, outside the alphabet",
            "emu_prc_db".to_string(),
            false,
        ),
        ("a multi-byte character", "emu-prc-café".to_string(), false),
        (
            "one past the 63-character bound",
            format!("e{}", "a".repeat(63)),
            false,
        ),
    ] {
        rows.push(ValueRow {
            case: format!("CreateDBInstance with {case}"),
            action: "CreateDBInstance",
            params: vec![
                ("DBInstanceIdentifier", identifier.clone()),
                ("Engine", "postgres".to_string()),
                ("DBInstanceClass", "db.t3.micro".to_string()),
            ],
            says: if hyphen_rule {
                states_the_rule("DBInstanceIdentifier")
            } else {
                format!("Invalid database identifier:  {identifier}")
            },
        });
    }

    // The same identifier rule on other members, each refused by an operation of a
    // different kind: a create, a modify, a rename and a delete.
    rows.push(ValueRow {
        case: "CreateOptionGroup with a leading digit".to_string(),
        action: "CreateOptionGroup",
        params: {
            let mut params = vec![("OptionGroupName", "1emu-prc-og".to_string())];
            params.extend(option_group_rest());
            params
        },
        says: "Invalid option group name:  1emu-prc-og".to_string(),
    });
    rows.push(ValueRow {
        case: "ModifyOptionGroup with a trailing hyphen".to_string(),
        action: "ModifyOptionGroup",
        params: vec![("OptionGroupName", "emu-prc-og-".to_string())],
        says: "Invalid option group name:  emu-prc-og-".to_string(),
    });
    rows.push(ValueRow {
        case: "CreateDBSnapshot with a doubled hyphen in the snapshot identifier".to_string(),
        action: "CreateDBSnapshot",
        params: vec![
            ("DBSnapshotIdentifier", "emu--prc-snap".to_string()),
            ("DBInstanceIdentifier", LIVE_INSTANCE.to_string()),
        ],
        says: "DBSnapshotIdentifier".to_string(),
    });
    rows.push(ValueRow {
        case: "CreateDBSnapshot with a source identifier outside the alphabet".to_string(),
        action: "CreateDBSnapshot",
        params: vec![
            ("DBSnapshotIdentifier", "emu-prc-snap-ok".to_string()),
            ("DBInstanceIdentifier", "emu_prc_source".to_string()),
        ],
        says: "Invalid database identifier:  emu_prc_source".to_string(),
    });
    rows.push(ValueRow {
        case: "DeleteDBInstance with a final snapshot identifier outside the alphabet"
            .to_string(),
        action: "DeleteDBInstance",
        params: vec![
            ("DBInstanceIdentifier", LIVE_INSTANCE.to_string()),
            ("FinalDBSnapshotIdentifier", "emu_prc_final".to_string()),
        ],
        says: "FinalDBSnapshotIdentifier".to_string(),
    });
    rows.push(ValueRow {
        case: "ModifyDBInstance renaming to a name with a leading digit".to_string(),
        action: "ModifyDBInstance",
        params: vec![
            ("DBInstanceIdentifier", LIVE_INSTANCE.to_string()),
            ("NewDBInstanceIdentifier", "9emu-prc-new".to_string()),
        ],
        says: "Invalid database identifier:  9emu-prc-new".to_string(),
    });

    // A published value list, not an identifier: the same rule over a different kind of
    // constraint, and the list is case-sensitive.
    for (case, engine) in [
        ("an engine no option group can name", "not-an-engine"),
        ("an engine of a hosted family in the wrong case", "MySQL"),
    ] {
        rows.push(ValueRow {
            case: format!("CreateOptionGroup with {case}"),
            action: "CreateOptionGroup",
            params: vec![
                ("OptionGroupName", "emu-prc-engine".to_string()),
                ("EngineName", engine.to_string()),
                ("MajorEngineVersion", "8.0".to_string()),
                ("OptionGroupDescription", "a group".to_string()),
            ],
            says: "EngineName".to_string(),
        });
    }

    // A numeric range, past each published bound and well outside both, on two different
    // reports — the window is one rule the reports share.
    for action in ["DescribeDBEngineVersions", "DescribeDBInstances"] {
        for (case, value) in [
            ("one below the published minimum", MAX_RECORDS_RANGE.start() - 1),
            ("one above the published maximum", MAX_RECORDS_RANGE.end() + 1),
            ("zero", 0),
            ("a negative page size", -1),
        ] {
            rows.push(ValueRow {
                case: format!("{action} with MaxRecords {case}"),
                action,
                params: vec![("MaxRecords", value.to_string())],
                says: format!(
                    "Invalid value {value} for MaxRecords. Must be between {} and {}",
                    MAX_RECORDS_RANGE.start(),
                    MAX_RECORDS_RANGE.end()
                ),
            });
        }
    }

    // A filter name the operation does not document, on three reports, including a
    // documented name in the wrong case ("Filter names are case-sensitive").
    for (action, name) in [
        ("DescribeDBInstances", "db-instance-name"),
        ("DescribeDBInstances", "DB-Instance-Id"),
        ("DescribeDBSnapshots", "not-a-filter"),
        ("DescribeDBEngineVersions", "Engine"),
    ] {
        rows.push(ValueRow {
            case: format!("{action} with the filter name {name}"),
            action,
            params: vec![
                ("Filters.Filter.1.Name", name.to_string()),
                ("Filters.Filter.1.Values.Value.1", "anything".to_string()),
            ],
            says: name.to_string(),
        });
    }

    rows
}

/// Create the DB instance the rows that need an existing one are aimed at, and wait until
/// it is `available`.
async fn seed_live_instance(router: &smithy_server_runtime::MultiServiceRouter) {
    expect_ok(
        "seed the live instance",
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", LIVE_INSTANCE),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.t3.micro"),
            ],
        )
        .await,
    );
    wait_for_settle();
}

/// Every refusal of a parameter VALUE answers HTTP 400 with the Query API's
/// `InvalidParameterValue`, and says what it refused.
#[tokio::test]
async fn a_value_its_member_does_not_accept_is_refused_as_an_invalid_parameter_value() {
    let router = emulator();
    seed_live_instance(&router).await;

    for row in value_rows() {
        let body = expect_error(
            &row.case,
            call(&router, row.action, &row.params).await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(&row.says),
            "case {:?}: expected a refusal saying {:?}, got {message:?}",
            row.case,
            row.says
        );
    }
}

/// The members RDS words by their subject carry exactly `Invalid <subject>:  <value>` —
/// the value from the request, and no reason — where the value is no identifier TOKEN of
/// that member at all, and for the OPTION GROUP name whichever clause it broke; a DB
/// instance identifier that is a token and breaks a hyphen rule states the rule instead.
#[tokio::test]
async fn an_identifier_refusal_is_worded_by_the_subject_its_member_carries() {
    let router = emulator();

    // (case, action, the members the call carries, the whole message it must answer with).
    type SubjectRow = (&'static str, &'static str, Vec<(&'static str, String)>, String);
    let rows: Vec<SubjectRow> = vec![
        (
            "CreateDBInstance, a leading digit",
            "CreateDBInstance",
            vec![
                ("DBInstanceIdentifier", "1emu-subject".to_string()),
                ("Engine", "postgres".to_string()),
                ("DBInstanceClass", "db.t3.micro".to_string()),
            ],
            "Invalid database identifier:  1emu-subject".to_string(),
        ),
        (
            "CreateDBInstance, a trailing hyphen",
            "CreateDBInstance",
            vec![
                ("DBInstanceIdentifier", "emu-subject-".to_string()),
                ("Engine", "postgres".to_string()),
                ("DBInstanceClass", "db.t3.micro".to_string()),
            ],
            states_the_rule("DBInstanceIdentifier"),
        ),
        (
            "CreateOptionGroup, a leading hyphen",
            "CreateOptionGroup",
            {
                let mut params = vec![("OptionGroupName", "-emu-subject".to_string())];
                params.extend(option_group_rest());
                params
            },
            "Invalid option group name:  -emu-subject".to_string(),
        ),
        (
            "CreateOptionGroup, an out-of-alphabet character",
            "CreateOptionGroup",
            {
                let mut params = vec![("OptionGroupName", "emu_subject".to_string())];
                params.extend(option_group_rest());
                params
            },
            "Invalid option group name:  emu_subject".to_string(),
        ),
        (
            // `DBInstanceIdentifier` "is stored as a lowercase string", but the refusal
            // quotes the value the REQUEST carried, so its case survives.
            "CreateDBInstance, a mixed-case identifier with a leading digit",
            "CreateDBInstance",
            vec![
                ("DBInstanceIdentifier", "1EMU-Subject-Mixed".to_string()),
                ("Engine", "postgres".to_string()),
                ("DBInstanceClass", "db.t3.micro".to_string()),
            ],
            "Invalid database identifier:  1EMU-Subject-Mixed".to_string(),
        ),
        (
            "ModifyDBInstance, a mixed-case rename target with a leading digit",
            "ModifyDBInstance",
            vec![
                ("DBInstanceIdentifier", "emu-subject-source".to_string()),
                ("NewDBInstanceIdentifier", "9EMU-Subject-Renamed".to_string()),
            ],
            "Invalid database identifier:  9EMU-Subject-Renamed".to_string(),
        ),
        (
            "ModifyDBInstance, a rename target ending in a hyphen",
            "ModifyDBInstance",
            vec![
                ("DBInstanceIdentifier", "emu-subject-source".to_string()),
                ("NewDBInstanceIdentifier", "EMU-Subject-Renamed-".to_string()),
            ],
            states_the_rule("NewDBInstanceIdentifier"),
        ),
        (
            "ModifyOptionGroup, a doubled hyphen",
            "ModifyOptionGroup",
            vec![("OptionGroupName", "emu--subject".to_string())],
            "Invalid option group name:  emu--subject".to_string(),
        ),
    ];

    for (case, action, params, expected) in rows {
        let body = expect_error(
            case,
            call(&router, action, &params).await,
            400,
            "InvalidParameterValue",
        );
        assert_eq!(
            text(&body, "Message"),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            text(&body, "Message")
        );
    }
}

/// A `Marker` no prior page issued is refused in one wording, identically on every one of
/// these paginated reports: the fixed sentence, which reflects no part of the request back.
#[tokio::test]
async fn an_unissued_marker_is_refused_in_one_fixed_text_on_every_report() {
    let router = emulator();
    for action in [
        "DescribeDBEngineVersions",
        "DescribeDBInstances",
        "DescribeOptionGroups",
        "DescribeDBSnapshots",
    ] {
        for marker in ["not-a-marker", "999999", "000000000000"] {
            let case = format!("{action} resumed from {marker:?}");
            let body = expect_error(
                &case,
                call(&router, action, &[("Marker", marker)]).await,
                400,
                "InvalidParameterValue",
            );
            assert_eq!(
                text(&body, "Message"),
                invalid_marker_message(),
                "case {case:?}: expected the service's own marker refusal, got {:?}",
                text(&body, "Message")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The constraints the MODEL declares
// ---------------------------------------------------------------------------

/// One row of the modelled-constraint table: a request whose value breaks a constraint the
/// model declares, and the clause its refusal must state.
struct ConstraintRow {
    case: String,
    action: &'static str,
    params: Vec<(&'static str, String)>,
    /// The member the clause names, spelled the way a validation path spells it.
    path: &'static str,
    /// The value the clause echoes back.
    value: String,
    /// The requirement the clause states.
    requirement: String,
}

/// The requirement clause of a numeric member below its `@range` minimum.
fn at_least(bound: i32) -> String {
    format!("Member must have value greater than or equal to {bound}")
}

/// The requirement clause of a numeric member above its `@range` maximum.
fn at_most(bound: i32) -> String {
    format!("Member must have value less than or equal to {bound}")
}

/// The requirement clause of a string shorter than its `@length` minimum.
fn at_least_long(bound: usize) -> String {
    format!("Member must have length greater than or equal to {bound}")
}

/// The requirement clause of a string longer than its `@length` maximum.
fn at_most_long(bound: usize) -> String {
    format!("Member must have length less than or equal to {bound}")
}

/// The longest `Engine` and `MajorEngineVersion`
/// `DescribeDBMajorEngineVersions` admits: both members are `len 1..50`
/// (`smithy_query(query="shape",shape="DescribeDBMajorEngineVersionsRequest")`).
const MAX_ENGINE_LEN: usize = 50;

/// The longest `Marker` that operation admits: `len 1..340`, from the same shape.
const MAX_MARKER_LEN: usize = 340;

/// The values `ExportSourceType` publishes
/// (`smithy_query(query="shape",shape="ExportSourceType")`), in the order the enum lists
/// them — which is the order the requirement clause states them in.
const EXPORT_SOURCE_TYPES: [&str; 2] = ["SNAPSHOT", "CLUSTER"];

/// The rows of the modelled-constraint table.
///
/// The axes are the constraint KIND (a numeric `@range` crossed in each direction, a
/// `@length` crossed in each direction, an enumerated value set) and the operation it is
/// declared on: the three reports below each take their page size against the one
/// `MaxRecords` shape the model bounds at 20..100, so the same bound is asserted through
/// three different front doors rather than one.
fn constraint_rows() -> Vec<ConstraintRow> {
    let mut rows = Vec::new();

    let (low, high) = (*MAX_RECORDS_RANGE.start(), *MAX_RECORDS_RANGE.end());
    for action in [
        "DescribeDBMajorEngineVersions",
        "DescribeExportTasks",
        "DescribeDBShardGroups",
    ] {
        for (case, value, requirement) in [
            ("one below the modelled minimum", low - 1, at_least(low)),
            ("zero", 0, at_least(low)),
            ("a negative page size", -1, at_least(low)),
            ("one above the modelled maximum", high + 1, at_most(high)),
            ("far above the modelled maximum", 1000, at_most(high)),
        ] {
            rows.push(ConstraintRow {
                case: format!("{action} with MaxRecords {case}"),
                action,
                params: vec![("MaxRecords", value.to_string())],
                path: "maxRecords",
                value: value.to_string(),
                requirement,
            });
        }
    }

    // A `@length` on a string member, crossed in each direction, on two members of one
    // operation.
    for member in ["Engine", "MajorEngineVersion"] {
        let path = if member == "Engine" {
            "engine"
        } else {
            "majorEngineVersion"
        };
        let too_long = "a".repeat(MAX_ENGINE_LEN + 1);
        rows.push(ConstraintRow {
            case: format!("DescribeDBMajorEngineVersions with a {member} one past its bound"),
            action: "DescribeDBMajorEngineVersions",
            params: vec![(if member == "Engine" { "Engine" } else { "MajorEngineVersion" }, too_long.clone())],
            path,
            value: too_long,
            requirement: at_most_long(MAX_ENGINE_LEN),
        });
        rows.push(ConstraintRow {
            case: format!("DescribeDBMajorEngineVersions with an empty {member}"),
            action: "DescribeDBMajorEngineVersions",
            params: vec![(if member == "Engine" { "Engine" } else { "MajorEngineVersion" }, String::new())],
            path,
            value: String::new(),
            requirement: at_least_long(1),
        });
    }
    rows.push(ConstraintRow {
        case: "DescribeDBMajorEngineVersions with an empty Marker".to_string(),
        action: "DescribeDBMajorEngineVersions",
        params: vec![("Marker", String::new())],
        path: "marker",
        value: String::new(),
        requirement: at_least_long(1),
    });

    // An enumerated value set: a value outside the one the model publishes.
    for value in ["not-a-source-type", "snapshot"] {
        rows.push(ConstraintRow {
            case: format!("DescribeExportTasks with the SourceType {value:?}"),
            action: "DescribeExportTasks",
            params: vec![("SourceType", value.to_string())],
            path: "sourceType",
            value: value.to_string(),
            requirement: format!(
                "Member must satisfy enum value set: [{}]",
                EXPORT_SOURCE_TYPES.join(", ")
            ),
        });
    }

    rows
}

/// A value that broke a constraint the MODEL declares is refused under the same Query API
/// code as one a handler refuses: HTTP 400 `InvalidParameterValue`, in the awsQuery error
/// envelope this front door answers with, stating the member and the requirement it broke.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_carries_the_same_invalid_parameter_value() {
    let router = emulator();

    for row in constraint_rows() {
        let body = expect_error(
            &row.case,
            call(&router, row.action, &row.params).await,
            400,
            "InvalidParameterValue",
        );
        assert_eq!(
            text(&body, "Message"),
            format!(
                "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
                row.value, row.path, row.requirement
            ),
            "case {:?}: expected the clause naming the member and the requirement, got {body}",
            row.case
        );
        // The envelope stays the one every other refusal of this front door carries.
        assert_eq!(
            text(&body, "Type"),
            "Sender",
            "case {:?}: expected a Sender fault, got {body}",
            row.case
        );
        assert!(
            body.contains("<RequestId>"),
            "case {:?}: expected the envelope's RequestId, got {body}",
            row.case
        );
    }

    // The published bound itself is SERVED, so no row above passed on a report that
    // refuses everything.
    for (case, params) in [
        (
            "the modelled minimum page size",
            vec![("MaxRecords", MAX_RECORDS_RANGE.start().to_string())],
        ),
        (
            "the modelled maximum page size",
            vec![("MaxRecords", MAX_RECORDS_RANGE.end().to_string())],
        ),
        (
            "the longest Engine the member admits",
            vec![("Engine", "a".repeat(MAX_ENGINE_LEN))],
        ),
        (
            "the longest MajorEngineVersion the member admits",
            vec![("MajorEngineVersion", "a".repeat(MAX_ENGINE_LEN))],
        ),
    ] {
        expect_ok(
            case,
            call(&router, "DescribeDBMajorEngineVersions", &params).await,
        );
    }

    // A `Marker` at its length bound passes the boundary and is then refused by the
    // paginator instead — same code, the service's own text — which is what shows the
    // rows above were refused for the constraint they broke rather than for being long.
    let body = expect_error(
        "the longest Marker the member admits",
        call(
            &router,
            "DescribeDBMajorEngineVersions",
            &[("Marker", "m".repeat(MAX_MARKER_LEN))],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    assert_eq!(
        text(&body, "Message"),
        invalid_marker_message(),
        "case \"the longest Marker the member admits\": expected the paginator's own refusal, \
         got {body}"
    );

    // One past that bound is the boundary's refusal again, and the clause still names the
    // member — the value is elided where it is longer than the envelope echoes, so this
    // row reads the clause's ends rather than the whole of it.
    let body = expect_error(
        "one character past the Marker bound",
        call(
            &router,
            "DescribeDBMajorEngineVersions",
            &[("Marker", "m".repeat(MAX_MARKER_LEN + 1))],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    let message = text(&body, "Message");
    assert!(
        message.starts_with("1 validation error detected: Value 'mmm")
            && message.ends_with(&format!(
                "at 'marker' failed to satisfy constraint: {}",
                at_most_long(MAX_MARKER_LEN)
            )),
        "case \"one character past the Marker bound\": expected the clause naming marker and \
         its length bound, got {message:?}"
    );
}

/// The snapshot paths' not-found wordings, each with its modelled code and status: the
/// resource type, then the identifier the request named.
#[tokio::test]
async fn the_snapshot_paths_word_their_not_found_faults_as_the_service_does() {
    let router = emulator();

    // `CreateDBSnapshot` against an instance this region holds no record of. The
    // identifiers vary so a row cannot pass on a value baked into the handler, and the
    // mixed-case row pins that the refusal quotes the value the REQUEST carried even
    // though the lookup folds it.
    for identifier in [
        "emu-prc-missing",
        "another-missing-instance",
        "x",
        "EMU-PRC-Missing-Mixed",
    ] {
        let case = format!("CreateDBSnapshot of the absent {identifier}");
        let body = expect_error(
            &case,
            call(
                &router,
                "CreateDBSnapshot",
                &[
                    ("DBSnapshotIdentifier", format!("{identifier}-snap")),
                    ("DBInstanceIdentifier", identifier.to_string()),
                ],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
        assert_eq!(
            text(&body, "Message"),
            format!("DBInstance not found: {identifier}"),
            "case {case:?}: got {:?}",
            text(&body, "Message")
        );
    }

    // `DescribeDBSnapshots` naming a snapshot this region holds no record of.
    for identifier in ["emu-prc-no-snapshot", "another-missing-snapshot"] {
        let case = format!("DescribeDBSnapshots of the absent {identifier}");
        let body = expect_error(
            &case,
            call(
                &router,
                "DescribeDBSnapshots",
                &[("DBSnapshotIdentifier", identifier)],
            )
            .await,
            404,
            "DBSnapshotNotFound",
        );
        assert_eq!(
            text(&body, "Message"),
            format!("DBSnapshot not found: {identifier}"),
            "case {case:?}: got {:?}",
            text(&body, "Message")
        );
    }

    // A snapshot that IS stored reads back, so the rows above fail for absence rather than
    // because these paths refuse everything.
    seed_live_instance(&router).await;
    expect_ok(
        "snapshot the live instance",
        call(
            &router,
            "CreateDBSnapshot",
            &[
                ("DBSnapshotIdentifier", "emu-prc-snap"),
                ("DBInstanceIdentifier", LIVE_INSTANCE),
            ],
        )
        .await,
    );
    let read_back = expect_ok(
        "read the snapshot back",
        call(
            &router,
            "DescribeDBSnapshots",
            &[("DBSnapshotIdentifier", "emu-prc-snap")],
        )
        .await,
    );
    assert_eq!(
        text(&read_back, "DBSnapshotIdentifier"),
        "emu-prc-snap",
        "the stored snapshot must read back in {REGION}, got {read_back}"
    );
}
