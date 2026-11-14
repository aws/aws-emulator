// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's DB-parameter slice: `ModifyDBParameterGroup`, which
//! writes parameter settings into a DB (instance) parameter group, and the
//! `DescribeDBParameters` that reads them back.
//!
//! A group carries the parameters a CALLER set; the engine's own defaults belong to the
//! FAMILY, which publishes them — the same catalog `DescribeEngineDefaultParameters` reports
//! for that family — so the rule under test is: a modify records the settings it names,
//! validated against the family's published catalog, and a describe reports exactly what
//! state holds, by `Source`.
//!
//! The rules are asserted over TABLES spanning the class each rule generalises over, not
//! over one request:
//!
//! * the value rule is driven by the published catalog ITSELF
//!   (`crate::state::published_db_parameters`) for one family per engine vocabulary, every
//!   entry of it, with each enumerated value, each published range bound and one step past
//!   it, with an unmodifiable parameter refused at a value it otherwise publishes, and with a
//!   name the catalog does not carry taken as sent;
//! * the apply-method rule over both apply types a catalog publishes, both methods the
//!   model's `ApplyMethod` enum names, and the absent one;
//! * the `Source` filter in both of the API's vocabularies, the absent one the member
//!   documents as "all parameter types", and a value naming no source;
//! * the request and group bounds at the documented maximum and one past it;
//! * the regional scope over two regions and one group name;
//! * pagination at the published page-size bounds and across the marker.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    family_parameters, published_db_parameters, ClusterParameterSpec, APPLY_METHOD_IMMEDIATE,
    APPLY_METHOD_PENDING_REBOOT, APPLY_TYPE_DYNAMIC,
    IMMEDIATE_APPLY_METHOD_FOR_A_STATIC_PARAMETER_MESSAGE, MAX_PARAMETERS_PER_GROUP,
    MAX_PARAMETERS_PER_MODIFY, MAX_RECORDS_RANGE, PARAMETER_SOURCE_ENGINE_DEFAULT,
    PARAMETER_SOURCE_SYSTEM, PARAMETER_SOURCE_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, text, REGION,
};
use crate::engine_default_parameters::reported;

/// The error code Amazon RDS answers a parameter value it cannot serve with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The wire code of this service's synthetic refusal for a request outside a rule the model
/// does not express.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `awsQueryError` code of `DBParameterGroupNotFoundFault`.
const NOT_FOUND: &str = "DBParameterGroupNotFound";

/// One parameter setting a modify asks for: the name, the value, and the apply method where
/// the request names one.
type Setting<'a> = (&'a str, &'a str, Option<&'a str>);

/// One reported `<Parameter>`, projected onto what these assertions read: its name, its
/// value, its apply method and the allowed values its family publishes for it.
type ReportedSetting = (String, Option<String>, Option<String>, Option<String>);

/// One row of the filter table: the case, the request members carrying its filters, and the
/// parameter names the report must then carry.
type FilterCase = (String, Vec<(String, String)>, Vec<String>);

/// Create a DB parameter group and return the name the store keys it under.
///
/// `pub(crate)` so a sibling suite of this one test target creates a DB parameter group the
/// same way rather than growing a second copy of this call.
pub(crate) async fn create_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    family: &str,
) -> String {
    expect_ok(
        case,
        call(
            router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", name),
                ("DBParameterGroupFamily", family),
                ("Description", "a group under test"),
            ],
        )
        .await,
    );
    name.to_ascii_lowercase()
}

/// `ModifyDBParameterGroup` on `group` in `region`, carrying one `Parameters.Parameter.<n>`
/// entry per setting.
///
/// `pub(crate)` so a sibling suite of this one test target writes the settings a reset then
/// clears through the SAME call rather than growing a second copy of it.
pub(crate) async fn modify_in(
    router: &MultiServiceRouter,
    region: &str,
    group: &str,
    settings: &[Setting<'_>],
) -> (u16, String) {
    let mut params: Vec<(String, String)> =
        vec![("DBParameterGroupName".to_string(), group.to_string())];
    for (index, (name, value, apply_method)) in settings.iter().enumerate() {
        let position = index + 1;
        params.push((
            format!("Parameters.Parameter.{position}.ParameterName"),
            (*name).to_string(),
        ));
        params.push((
            format!("Parameters.Parameter.{position}.ParameterValue"),
            (*value).to_string(),
        ));
        if let Some(method) = apply_method {
            params.push((
                format!("Parameters.Parameter.{position}.ApplyMethod"),
                (*method).to_string(),
            ));
        }
    }
    call_in(router, region, "ModifyDBParameterGroup", &params).await
}

/// [`modify_in`] against [`REGION`].
async fn modify(
    router: &MultiServiceRouter,
    group: &str,
    settings: &[Setting<'_>],
) -> (u16, String) {
    modify_in(router, REGION, group, settings).await
}

/// `DescribeDBParameters` on `group`, carrying `extra` request members.
///
/// `pub(crate)` for the same reason [`create_group`] is.
pub(crate) async fn describe(
    router: &MultiServiceRouter,
    group: &str,
    extra: &[(&str, String)],
) -> (u16, String) {
    let mut params: Vec<(String, String)> =
        vec![("DBParameterGroupName".to_string(), group.to_string())];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone())),
    );
    call(router, "DescribeDBParameters", &params).await
}

/// The parameters `group` reports for `source`, asserted to answer 200, as
/// `(name, value, apply method, allowed values)` in the order they were reported.
async fn reported_for(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    source: Option<&str>,
) -> Vec<ReportedSetting> {
    let extra: Vec<(&str, String)> = source
        .map(|source| vec![("Source", source.to_string())])
        .unwrap_or_default();
    let body = expect_ok(case, describe(router, group, &extra).await);
    reported(&body)
        .into_iter()
        .map(|entry| {
            (
                entry.name,
                entry.value,
                entry.apply_method,
                entry.allowed_values,
            )
        })
        .collect()
}

/// The value `group` reports for the parameter `name` under `source`, and the `Source` it
/// reports it with.
async fn reported_value(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    source: Option<&str>,
    name: &str,
) -> Option<(String, String)> {
    let extra: Vec<(&str, String)> = source
        .map(|source| vec![("Source", source.to_string())])
        .unwrap_or_default();
    let body = expect_ok(case, describe(router, group, &extra).await);
    reported(&body).into_iter().find_map(|entry| {
        (entry.name == name).then(|| {
            (
                entry.value.unwrap_or_default(),
                entry.source.unwrap_or_default(),
            )
        })
    })
}

// ---------------------------------------------------------------------------
// The slice, end to end
// ---------------------------------------------------------------------------

/// A modify records what it names and the describe reports it from state: the group's own
/// parameter under `Source` `user` with the metadata its family publishes, the family's
/// published default under `engine-default`, and both at once when the request names no
/// source — which is what `Source` documents ("Default: All parameter types returned").
#[tokio::test]
async fn a_modified_parameter_is_reported_from_persisted_state() {
    let router = emulator();
    let family = "mysql8.0";
    let catalog = family_parameters(
        family,
        published_db_parameters(family).expect("this family publishes a catalog"),
    );
    let spec = catalog
        .iter()
        .find(|spec| spec.name == "wait_timeout")
        .expect("the catalog publishes wait_timeout");
    let group = create_group(&router, "create", "Probe-PG", family).await;

    // 1. A freshly created group carries no parameter of its own: the defaults belong to the
    //    family, so the `user` report is empty while the `engine-default` one is the whole
    //    published catalog.
    assert_eq!(
        reported_for(&router, "a fresh group's own parameters", &group, Some("user")).await,
        Vec::new(),
        "case {:?}: a group carries no parameter until one is set",
        "a fresh group's own parameters"
    );
    let defaults = reported_for(
        &router,
        "a fresh group's engine defaults",
        &group,
        Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
    )
    .await;
    assert_eq!(
        defaults.len(),
        catalog.len(),
        "case {:?}: expected the family's whole published catalog, got {defaults:?}",
        "a fresh group's engine defaults"
    );

    // 2. The modify echoes the group it wrote, under the name the store keys it by.
    let answered = expect_ok(
        "modify",
        modify(
            &router,
            "probe-pg",
            &[(spec.name, "600", Some(APPLY_METHOD_IMMEDIATE))],
        )
        .await,
    );
    assert_eq!(
        text(&answered, "DBParameterGroupName"),
        group,
        "case \"modify\": the answer must echo the group it wrote, got {answered}"
    );

    // 3. The group's own report is exactly that parameter, with the metadata the family
    //    publishes for it.
    assert_eq!(
        reported_for(&router, "the group's own parameters", &group, Some("user")).await,
        vec![(
            spec.name.to_string(),
            Some("600".to_string()),
            Some(APPLY_METHOD_IMMEDIATE.to_string()),
            Some(spec.allowed_values.to_string()),
        )],
        "case {:?}: expected the parameter just set, reported with its published metadata",
        "the group's own parameters"
    );

    // 4. The engine-default report is unchanged by the modify: a default belongs to the
    //    family, not to the group.
    assert_eq!(
        reported_value(
            &router,
            "the engine default after a modify",
            &group,
            Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
            spec.name,
        )
        .await,
        Some((
            spec.default_value.to_string(),
            PARAMETER_SOURCE_ENGINE_DEFAULT.to_string()
        )),
        "case {:?}: the family's published default must be reported as the engine's",
        "the engine default after a modify"
    );

    // 5. With no `Source`, every parameter type is reported at once: the whole catalog, with
    //    the one the group set carried under `user` and the rest under `engine-default`.
    let all = reported_for(&router, "every parameter type", &group, None).await;
    assert_eq!(
        all.len(),
        catalog.len(),
        "case {:?}: expected one entry per published parameter, got {all:?}",
        "every parameter type"
    );
    assert_eq!(
        reported_value(&router, "every parameter type", &group, None, spec.name).await,
        Some(("600".to_string(), PARAMETER_SOURCE_USER.to_string())),
        "case {:?}: the parameter the group set must be reported as the caller's",
        "every parameter type"
    );

    // 6. This emulator sets no service-sourced parameter, and a value naming no source at
    //    all selects nothing.
    for source in [PARAMETER_SOURCE_SYSTEM, "service", "not-a-source"] {
        let case = format!("Source {source:?}");
        assert_eq!(
            reported_for(&router, &case, &group, Some(source)).await,
            Vec::new(),
            "case {case:?}: expected no parameter reported"
        );
    }

    // 7. A later modify overwrites the setting rather than adding a second entry for the
    //    same name, and the group is still read by the name as sent in any case.
    expect_ok(
        "a second modify of the same parameter",
        modify(
            &router,
            "PROBE-PG",
            &[(spec.name, "900", Some(APPLY_METHOD_PENDING_REBOOT))],
        )
        .await,
    );
    assert_eq!(
        reported_for(
            &router,
            "a second modify of the same parameter",
            "Probe-PG",
            Some("user"),
        )
        .await,
        vec![(
            spec.name.to_string(),
            Some("900".to_string()),
            Some(APPLY_METHOD_PENDING_REBOOT.to_string()),
            Some(spec.allowed_values.to_string()),
        )],
        "case {:?}: expected one entry carrying the newest setting",
        "a second modify of the same parameter"
    );

    // 8. Both operations answer the modelled 404 for a group that is not there, and nothing
    //    was created by the attempt.
    for (case, settings) in [
        ("modify a missing group", &[(spec.name, "600", None)][..]),
        ("modify a missing group, two parameters", &[
            (spec.name, "600", None),
            ("autocommit", "1", None),
        ][..]),
    ] {
        expect_error(
            case,
            modify(&router, "no-such-group", settings).await,
            404,
            NOT_FOUND,
        );
    }
    expect_error(
        "describe the parameters of a missing group",
        describe(&router, "no-such-group", &[]).await,
        404,
        NOT_FOUND,
    );
}

// ---------------------------------------------------------------------------
// The value rule, over the published catalogs
// ---------------------------------------------------------------------------

/// A value the parameter admits, and one it does not, derived from the parameter's OWN
/// published `AllowedValues`: an enumeration is split, a range is taken at each published
/// bound and one step past it, and a single published value is matched exactly.
///
/// The range halves come from `ClusterParameterSpec::range`, the same reader the service
/// itself admits a value with, so a published bound is read here exactly as it is there.
fn admitted_and_refused(spec: &ClusterParameterSpec) -> (Vec<String>, Vec<String>) {
    if spec.allowed_values.contains(',') {
        let admitted: Vec<String> = spec
            .allowed_values
            .split(',')
            .map(|value| value.trim().to_string())
            .collect();
        return (admitted, vec!["not-a-published-value".to_string()]);
    }
    let numeric = matches!(spec.data_type, "integer" | "float");
    match spec.range().filter(|_| numeric) {
        None => (
            vec![spec.allowed_values.to_string()],
            vec!["not-a-published-value".to_string()],
        ),
        Some((low, high)) if spec.data_type == "integer" => {
            let low: i64 = low.parse().expect("an integer range bound");
            let high: i64 = high.parse().expect("an integer range bound");
            // One step outside each published bound, computed in a wider type so a
            // parameter whose published maximum is the largest integer the service parses
            // still has a value past it to be refused for.
            (
                vec![low.to_string(), high.to_string()],
                vec![
                    (i128::from(low) - 1).to_string(),
                    (i128::from(high) + 1).to_string(),
                    "not-a-number".to_string(),
                ],
            )
        }
        Some((low, high)) => {
            let low: f64 = low.parse().expect("a float range bound");
            let high: f64 = high.parse().expect("a float range bound");
            (
                vec![low.to_string(), high.to_string()],
                vec![
                    (low - 1.0).to_string(),
                    (high + 1.0).to_string(),
                    "not-a-number".to_string(),
                ],
            )
        }
    }
}

/// One family per engine parameter vocabulary this emulator publishes a DB-parameter catalog
/// for, as `CreateDBParameterGroupMessage$DBParameterGroupFamily` documents them.
///
/// The Microsoft SQL Server configuration options carry SPACES in their names, and the
/// Amazon DocumentDB catalog publishes a value carrying a `+` (`tls1.2+`); the shared
/// awsQuery sender deliberately refuses a value that would need form escaping, so those two
/// vocabularies are driven by the suites that own them (`engine_default_parameters` for the
/// published catalog, `cluster_parameter_groups` for the value rule over it).
const CATALOG_FAMILIES: [&str; 4] = ["mysql8.0", "postgres16", "oracle-ee-19", "neptune1.3"];

/// Every parameter a DB parameter group family publishes admits exactly the values its
/// `AllowedValues` names, and refuses anything else with `InvalidParameterValue` — a message
/// naming the parameter. An admitted value is readable back from state; a parameter the
/// engine does not let a caller modify is refused even at a value it publishes; and a name
/// the published catalog does not carry is taken as sent, because that catalog is a sample of
/// the family's list rather than the whole of it.
///
/// The table is the published catalog itself, one family per engine vocabulary, so it covers
/// the parameters and the engines the issue never mentioned.
#[tokio::test]
async fn a_parameter_value_outside_the_published_set_is_refused() {
    let router = emulator();

    for family in CATALOG_FAMILIES {
        let catalog = published_db_parameters(family)
            .unwrap_or_else(|| panic!("case {family:?}: this family must publish a catalog"));
        let group = create_group(
            &router,
            family,
            &format!("value-rule-{}", family.replace('.', "-")),
            family,
        )
        .await;

        for spec in catalog {
            let (admitted, refused) = admitted_and_refused(spec);
            for value in &admitted {
                let case = format!("{family} {} = {value:?}", spec.name);
                if !spec.is_modifiable {
                    // "IsModifiable: indicates whether the parameter can be modified": a
                    // value it publishes is still refused, because the engine owns it.
                    let answered = expect_error(
                        &case,
                        modify(&router, &group, &[(spec.name, value, None)]).await,
                        400,
                        INVALID_PARAMETER_VALUE,
                    );
                    assert!(
                        text(&answered, "Message").contains(spec.name),
                        "case {case:?}: the refusal must name the parameter, got {answered}"
                    );
                    continue;
                }
                let answered = expect_ok(
                    &case,
                    modify(&router, &group, &[(spec.name, value, None)]).await,
                );
                assert_eq!(
                    text(&answered, "DBParameterGroupName"),
                    group,
                    "case {case:?}: expected the group name echoed, got {answered}"
                );
                assert_eq!(
                    reported_value(&router, &case, &group, Some("user"), spec.name).await,
                    Some((value.clone(), PARAMETER_SOURCE_USER.to_string())),
                    "case {case:?}: expected the stored value reported as the caller's"
                );
            }
            for value in &refused {
                let case = format!("{family} {} = {value:?} (refused)", spec.name);
                let answered = expect_error(
                    &case,
                    modify(&router, &group, &[(spec.name, value, None)]).await,
                    400,
                    INVALID_PARAMETER_VALUE,
                );
                assert!(
                    text(&answered, "Message").contains(spec.name),
                    "case {case:?}: the refusal must name the parameter, got {answered}"
                );
            }
        }

        // A parameter name the published catalog does not carry is admitted and stored as
        // sent: an engine publishes hundreds of parameters per family and this catalog is a
        // sample of that list, so refusing an unpublished name would reject a parameter the
        // engine has. It is reported without the metadata only a published parameter has.
        let unpublished = "innodb_old_blocks_time";
        let case = format!("{family}: a parameter the catalog does not carry");
        assert!(
            catalog.iter().all(|spec| spec.name != unpublished),
            "case {case:?}: {unpublished} must be a name this family publishes NO entry for, \
             or the row below stops testing the unpublished-name rule"
        );
        expect_ok(
            &case,
            modify(&router, &group, &[(unpublished, "128", None)]).await,
        );
        let stored = reported_for(&router, &case, &group, Some("user")).await;
        assert!(
            stored.contains(&(
                unpublished.to_string(),
                Some("128".to_string()),
                Some(APPLY_METHOD_PENDING_REBOOT.to_string()),
                None,
            )),
            "case {case:?}: expected the unpublished parameter stored and reported without \
             published metadata, got {stored:?}"
        );

        // The model types `ApplyMethod` as an enum, so a method outside it is refused at the
        // boundary — before the handler — under the code this service answers a parameter
        // value its member does not accept with.
        let modifiable = catalog
            .iter()
            .find(|spec| spec.is_modifiable)
            .expect("every catalog publishes a modifiable parameter");
        let (admitted, _) = admitted_and_refused(modifiable);
        expect_error(
            &format!("{family}: an apply method the model does not name"),
            modify(
                &router,
                &group,
                &[(modifiable.name, &admitted[0], Some("whenever"))],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }
}

/// A family this emulator publishes no catalog for — the Db2 engines, whose parameters the
/// emulator does not publish — carries whatever the caller sets, because refusing a value
/// there would invent a constraint. The value is still read back from state, without the
/// metadata only a published parameter has.
#[tokio::test]
async fn a_family_with_no_published_catalog_carries_any_parameter() {
    let router = emulator();
    // (case, family, publishes a catalog, parameter, value)
    let rows: [(&str, &str, bool, &str, &str); 3] = [
        ("a published family", "mysql8.0", true, "autocommit", "0"),
        (
            "a family the emulator publishes no catalog for",
            "db2-se",
            false,
            "dbheap",
            "1200",
        ),
        (
            "another unpublished family, another parameter",
            "db2-ae",
            false,
            "a_parameter_no_catalog_names",
            "any-value",
        ),
    ];

    for (case, family, publishes, name, value) in rows {
        let group = create_group(
            &router,
            case,
            &format!("catalog-{}", family.replace('.', "-")),
            family,
        )
        .await;
        let spec = published_db_parameters(family)
            .and_then(|catalog| catalog.iter().find(|spec| spec.name == name));
        assert_eq!(
            spec.is_some(),
            publishes,
            "case {case:?}: expected a published spec for {name:?} == {publishes}"
        );
        expect_ok(case, modify(&router, &group, &[(name, value, None)]).await);
        assert_eq!(
            reported_for(&router, case, &group, Some("user")).await,
            vec![(
                name.to_string(),
                Some(value.to_string()),
                // With no method asked for, the change gets the one its published apply type
                // implies; with no published apply type at all, assuming the change needs no
                // reboot would be a guess, so it waits for one.
                Some(
                    match spec {
                        Some(spec) if spec.apply_type == APPLY_TYPE_DYNAMIC => {
                            APPLY_METHOD_IMMEDIATE
                        }
                        _ => APPLY_METHOD_PENDING_REBOOT,
                    }
                    .to_string()
                ),
                spec.map(|spec| spec.allowed_values.to_string()),
            )],
            "case {case:?}: expected the stored parameter reported, with published metadata \
             only for a family that has a catalog"
        );
    }
}

// ---------------------------------------------------------------------------
// The apply method a change gets
// ---------------------------------------------------------------------------

/// `ApplyMethod` is recorded as the request asked for it, where the parameter can take the
/// method asked for; where the request asks for none, the change gets the method its
/// parameter's published apply type implies — a dynamic parameter is applied immediately, and
/// a static one waits for the reboot. A STATIC parameter cannot be applied immediately at all,
/// so that one combination is refused instead of stored.
#[tokio::test]
async fn an_absent_apply_method_follows_the_parameters_apply_type() {
    let router = emulator();
    let family = "postgres16";
    let catalog = published_db_parameters(family).expect("this family publishes a catalog");
    let group = create_group(&router, "apply methods", "apply-method-pg", family).await;

    // The table is derived from the catalog: every published parameter, under each method the
    // model's `ApplyMethod` enum names and under none at all.
    for spec in catalog.iter().filter(|spec| spec.is_modifiable) {
        let (admitted, _) = admitted_and_refused(spec);
        let value = &admitted[0];
        // The apply type this FAMILY publishes. `rds.force_ssl` is the one entry whose apply
        // type moved with the engine: the PostgreSQL release at 15 applies a change to it
        // without a restart, and this family is past that bound, whatever earlier value the
        // catalogue's own column carries.
        let apply_type = if spec.name == "rds.force_ssl" {
            APPLY_TYPE_DYNAMIC
        } else {
            spec.apply_type
        };
        let implied = if apply_type == APPLY_TYPE_DYNAMIC {
            APPLY_METHOD_IMMEDIATE
        } else {
            APPLY_METHOD_PENDING_REBOOT
        };
        for (asked, expected) in [
            (Some(APPLY_METHOD_IMMEDIATE), APPLY_METHOD_IMMEDIATE),
            (Some(APPLY_METHOD_PENDING_REBOOT), APPLY_METHOD_PENDING_REBOOT),
            (None, implied),
        ] {
            let case = format!("{} ({apply_type}) asked for {asked:?}", spec.name);
            // The one combination a parameter cannot take: a change to a STATIC parameter
            // needs a reboot before it can take effect, so asking for it to be applied
            // immediately is refused rather than stored under a method the engine could not
            // honour.
            if asked == Some(APPLY_METHOD_IMMEDIATE) && apply_type != APPLY_TYPE_DYNAMIC {
                let answered = expect_error(
                    &case,
                    modify(&router, &group, &[(spec.name, value, asked)]).await,
                    400,
                    "InvalidParameterCombination",
                );
                let message = text(&answered, "Message");
                assert_eq!(
                    message, IMMEDIATE_APPLY_METHOD_FOR_A_STATIC_PARAMETER_MESSAGE,
                    "case {case:?}: expected the published refusal wording, got {message:?}"
                );
                continue;
            }
            expect_ok(
                &case,
                modify(&router, &group, &[(spec.name, value, asked)]).await,
            );
            let body = expect_ok(
                &case,
                describe(&router, &group, &[("Source", "user".to_string())]).await,
            );
            let method = reported(&body)
                .into_iter()
                .find(|entry| entry.name == spec.name)
                .and_then(|entry| entry.apply_method);
            assert_eq!(
                method.as_deref(),
                Some(expected),
                "case {case:?}: expected ApplyMethod {expected:?}, got {method:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The bounds a modify is held to
// ---------------------------------------------------------------------------

/// "A maximum of 20 parameters can be modified in a single request": the documented maximum
/// is served, one past it is refused and changes nothing, and the TOTAL a group may carry is
/// bounded the same way.
#[tokio::test]
async fn the_documented_request_and_group_bounds_are_enforced() {
    let router = emulator();
    // A family the emulator publishes no catalog for, so the bound rather than the catalog is
    // what refuses a request.
    let group = create_group(&router, "bounds", "bounds-pg", "db2-se").await;
    let names: Vec<String> = (0..=MAX_PARAMETERS_PER_GROUP)
        .map(|index| format!("p_{index:03}"))
        .collect();

    // One past the documented per-request maximum is refused...
    let over: Vec<(&str, &str, Option<&str>)> = names
        .iter()
        .take(MAX_PARAMETERS_PER_MODIFY + 1)
        .map(|name| (name.as_str(), "1", None))
        .collect();
    expect_error(
        "one parameter past the per-request maximum",
        modify(&router, &group, &over).await,
        400,
        VALIDATION_EXCEPTION,
    );
    assert_eq!(
        reported_for(
            &router,
            "one parameter past the per-request maximum",
            &group,
            Some("user"),
        )
        .await,
        Vec::new(),
        "case {:?}: a refused request must write nothing at all",
        "one parameter past the per-request maximum"
    );

    // ...and the maximum itself is served.
    let at_bound: Vec<(&str, &str, Option<&str>)> = names
        .iter()
        .take(MAX_PARAMETERS_PER_MODIFY)
        .map(|name| (name.as_str(), "1", None))
        .collect();
    expect_ok(
        "the per-request maximum",
        modify(&router, &group, &at_bound).await,
    );
    assert_eq!(
        reported_for(&router, "the per-request maximum", &group, Some("user"))
            .await
            .len(),
        MAX_PARAMETERS_PER_MODIFY,
        "case {:?}: every parameter the request named must be stored",
        "the per-request maximum"
    );

    // The group's own total is bounded too: filling it is allowed, one more is refused.
    for chunk in names[..MAX_PARAMETERS_PER_GROUP].chunks(MAX_PARAMETERS_PER_MODIFY) {
        let settings: Vec<(&str, &str, Option<&str>)> = chunk
            .iter()
            .map(|name| (name.as_str(), "1", None))
            .collect();
        expect_ok("filling the group", modify(&router, &group, &settings).await);
    }
    let last = names
        .last()
        .expect("the generated names are not empty")
        .as_str();
    expect_error(
        "one parameter past the group's total",
        modify(&router, &group, &[(last, "1", None)]).await,
        400,
        VALIDATION_EXCEPTION,
    );
    let stored = reported_for(
        &router,
        "one parameter past the group's total",
        &group,
        Some("user"),
    )
    .await;
    assert_eq!(
        stored.len(),
        MAX_PARAMETERS_PER_GROUP,
        "case {:?}: the refused request must leave the group as it was, got {} entries",
        "one parameter past the group's total",
        stored.len()
    );

    // The model marks `Parameters` required, so a request carrying none is refused at the
    // boundary rather than reaching the handler.
    expect_error(
        "a request carrying no Parameters at all",
        modify(&router, &group, &[]).await,
        400,
        "MalformedInput",
    );
}

// ---------------------------------------------------------------------------
// Regional scope
// ---------------------------------------------------------------------------

/// RDS is regional: one group name holds a different group in each region, so a modify
/// signed for one region writes only that region's group and a name only another region
/// holds is not found at all.
#[tokio::test]
async fn a_modify_writes_only_the_group_its_own_region_holds() {
    let router = emulator();
    let other = "eu-west-1";
    for region in [REGION, other] {
        expect_ok(
            region,
            call_in(
                &router,
                region,
                "CreateDBParameterGroup",
                &[
                    ("DBParameterGroupName", "shared-pg"),
                    ("DBParameterGroupFamily", "mysql8.0"),
                    ("Description", "a group under test"),
                ],
            )
            .await,
        );
    }

    expect_ok(
        "modify in the first region",
        modify_in(
            &router,
            REGION,
            "shared-pg",
            &[("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE))],
        )
        .await,
    );
    assert_eq!(
        reported_for(&router, "the first region's group", "shared-pg", Some("user")).await,
        vec![(
            "autocommit".to_string(),
            Some("0".to_string()),
            Some(APPLY_METHOD_IMMEDIATE.to_string()),
            Some("0,1".to_string()),
        )],
        "case {:?}: expected the parameter the modify wrote",
        "the first region's group"
    );

    // The other region's group of the same name is untouched.
    let elsewhere = expect_ok(
        "the other region's group",
        call_in(
            &router,
            other,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", "shared-pg"),
                ("Source", "user"),
            ],
        )
        .await,
    );
    assert_eq!(
        reported(&elsewhere).len(),
        0,
        "case {:?}: a modify in one region must not write another's group, got {elsewhere}",
        "the other region's group"
    );

    // A group only the first region holds is not there to modify from the other.
    expect_ok(
        "a group only the first region holds",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "only-here-pg"),
                ("DBParameterGroupFamily", "mysql8.0"),
                ("Description", "a group under test"),
            ],
        )
        .await,
    );
    expect_error(
        "modify a group only another region holds",
        modify_in(&router, other, "only-here-pg", &[("autocommit", "1", None)]).await,
        404,
        NOT_FOUND,
    );
}

// ---------------------------------------------------------------------------
// Filters and pagination
// ---------------------------------------------------------------------------

/// `Filters`: "The only supported filter is parameter-name", so a `parameter-name` filter
/// selects exactly the parameters it names, repeated filters intersect, and any other filter
/// name is refused rather than ignored.
#[tokio::test]
async fn the_parameter_name_filter_selects_exactly_what_it_names() {
    let router = emulator();
    let family = "mysql8.0";
    let catalog = published_db_parameters(family).expect("this family publishes a catalog");
    let group = create_group(&router, "filters", "filter-pg", family).await;
    let selected: Vec<&str> = catalog.iter().take(2).map(|spec| spec.name).collect();

    // (case, filters, expected names)
    let rows: Vec<FilterCase> = vec![
        (
            "one name".to_string(),
            vec![
                ("Filters.Filter.1.Name".to_string(), "parameter-name".to_string()),
                (
                    "Filters.Filter.1.Values.Value.1".to_string(),
                    selected[0].to_string(),
                ),
            ],
            vec![selected[0].to_string()],
        ),
        (
            "two names in one filter".to_string(),
            vec![
                ("Filters.Filter.1.Name".to_string(), "parameter-name".to_string()),
                (
                    "Filters.Filter.1.Values.Value.1".to_string(),
                    selected[0].to_string(),
                ),
                (
                    "Filters.Filter.1.Values.Value.2".to_string(),
                    selected[1].to_string(),
                ),
            ],
            {
                let mut names = vec![selected[0].to_string(), selected[1].to_string()];
                names.sort();
                names
            },
        ),
        (
            "repeated filters intersect".to_string(),
            vec![
                ("Filters.Filter.1.Name".to_string(), "parameter-name".to_string()),
                (
                    "Filters.Filter.1.Values.Value.1".to_string(),
                    selected[0].to_string(),
                ),
                (
                    "Filters.Filter.1.Values.Value.2".to_string(),
                    selected[1].to_string(),
                ),
                ("Filters.Filter.2.Name".to_string(), "parameter-name".to_string()),
                (
                    "Filters.Filter.2.Values.Value.1".to_string(),
                    selected[1].to_string(),
                ),
            ],
            vec![selected[1].to_string()],
        ),
        (
            "a name the catalog does not publish".to_string(),
            vec![
                ("Filters.Filter.1.Name".to_string(), "parameter-name".to_string()),
                (
                    "Filters.Filter.1.Values.Value.1".to_string(),
                    "no_such_parameter".to_string(),
                ),
            ],
            Vec::new(),
        ),
    ];

    for (case, filters, expected) in rows {
        let mut params: Vec<(String, String)> = vec![(
            "DBParameterGroupName".to_string(),
            group.clone(),
        )];
        params.extend(filters);
        let body = expect_ok(&case, call(&router, "DescribeDBParameters", &params).await);
        let names: Vec<String> = reported(&body).into_iter().map(|entry| entry.name).collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected {expected:?}, got {names:?}"
        );
    }

    // A filter name this operation does not support is refused rather than ignored.
    expect_error(
        "a filter name the operation does not support",
        call(
            &router,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", group.as_str()),
                ("Filters.Filter.1.Name", "parameter-source"),
                ("Filters.Filter.1.Values.Value.1", "user"),
            ],
        )
        .await,
        400,
        INVALID_PARAMETER_VALUE,
    );
}

/// `MaxRecords` is documented "Default: 100 Constraints: Minimum 20, maximum 100": both
/// published bounds are served and one step past each is refused, a report longer than the
/// page is walked across its `Marker`, and a `Marker` no prior page issued is refused rather
/// than resynchronised to the first page.
#[tokio::test]
async fn the_documented_page_window_is_served_and_walked() {
    let router = emulator();
    // The documented window, as LITERALS from the member's own sentence rather than from the
    // constant the handler reads, so a drift in that constant fails this test.
    let (low, high) = (20, 100);
    assert_eq!(
        (*MAX_RECORDS_RANGE.start(), *MAX_RECORDS_RANGE.end()),
        (low, high),
        "case {:?}: the window this service enforces must be the documented one",
        "the documented window"
    );

    // A family with no published catalog, so the page is exactly what the group holds.
    let group = create_group(&router, "pagination", "page-pg", "db2-se").await;
    let total = low as usize + 5;
    let names: Vec<String> = (0..total).map(|index| format!("p_{index:03}")).collect();
    for chunk in names.chunks(MAX_PARAMETERS_PER_MODIFY) {
        let settings: Vec<(&str, &str, Option<&str>)> = chunk
            .iter()
            .map(|name| (name.as_str(), "1", None))
            .collect();
        expect_ok("filling the group", modify(&router, &group, &settings).await);
    }

    for (case, max_records, served) in [
        ("the published minimum", low, true),
        ("the published maximum", high, true),
        ("one below the minimum", low - 1, false),
        ("one above the maximum", high + 1, false),
    ] {
        let response = describe(
            &router,
            &group,
            &[("MaxRecords", max_records.to_string())],
        )
        .await;
        if !served {
            expect_error(case, response, 400, INVALID_PARAMETER_VALUE);
            continue;
        }
        let body = expect_ok(case, response);
        let page = reported(&body).len();
        assert_eq!(
            page,
            total.min(max_records as usize),
            "case {case:?}: expected a page of at most {max_records} entries, got {body}"
        );
    }

    // The whole report is walked across the marker, in name order, with no entry repeated.
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..4 {
        let mut extra = vec![("MaxRecords", low.to_string())];
        if let Some(marker) = &marker {
            extra.push(("Marker", marker.clone()));
        }
        let body = expect_ok("walking the pages", describe(&router, &group, &extra).await);
        walked.extend(reported(&body).into_iter().map(|entry| entry.name));
        marker = optional_text(&body, "Marker");
        if marker.is_none() {
            break;
        }
    }
    assert_eq!(
        (walked.clone(), marker.clone()),
        (names.clone(), None),
        "case {:?}: expected every stored parameter once, in name order",
        "walking the pages"
    );

    // A `Marker` this operation never issued names no record of the result set.
    expect_error(
        "a Marker no prior page issued",
        describe(
            &router,
            &group,
            &[("Marker", "not-a-marker-this-service-minted".to_string())],
        )
        .await,
        400,
        INVALID_PARAMETER_VALUE,
    );
}
