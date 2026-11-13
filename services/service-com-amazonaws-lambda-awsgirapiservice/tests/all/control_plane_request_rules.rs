// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the control-plane requests Lambda REFUSES, and for the wording of
//! those refusals.
//!
//! Each test below pins ONE rule over a table of inputs, driving real HTTP requests
//! through the router and sharing the harness in `function_lifecycle.rs`:
//!
//! * a tag write applies the tags it names, so one naming NO tag is refused whatever
//!   resource it addressed;
//! * a tag ARN that qualifies a function with a version or an alias carries no tag set
//!   ("Lambda does not support adding tags to function aliases or versions"), and every
//!   operation on that path says so in one fixed sentence;
//! * the unpublished version exists for as long as the function does, so deleting it on
//!   its own is refused in one fixed sentence;
//! * a layer version this account never published is named by its FULL
//!   `LayerVersionArn` in the refusals of the operations on its policy;
//! * a `FunctionName` that qualifies one version and a `Qualifier` naming another name no
//!   single resource, and the refusal states that and nothing of the request;
//! * the 4 KB environment budget is measured on the variables as ONE serialized object, so
//!   the string the refusal reports is what was counted;
//! * SnapStart can be applied only to a runtime that supports it, and a version it is
//!   applied to cannot also keep provisioned concurrency warm;
//! * a weighted alias splits its invocations between TWO published versions that run as one
//!   execution role;
//! * a pagination `Marker` is the token a previous page answered.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, function_arn, json_call, make_activation_due, make_update_due, package_bytes,
    router_on, zip_file, Answer, ACCOUNT, REGION, ROLE,
};

/// Where `CreateFunction` and the function-scoped operations are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the tag operations are bound (`/2017-03-31/tags/{Resource}`).
const TAGS_URI: &str = "/2017-03-31/tags";

/// Where the capacity-provider operations are bound.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// Where the provisioned-concurrency operations are bound
/// (`PUT /2019-09-30/functions/{FunctionName}/provisioned-concurrency?Qualifier=`).
const PROVISIONED_CONCURRENCY_PREFIX: &str = "/2019-09-30/functions";

/// A second execution role, so a row can name two versions that do NOT run as one
/// principal. Satisfies the model's `RoleArn` pattern exactly as [`ROLE`] does.
const OTHER_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec-two";

/// An operator role for a capacity provider, satisfying the same pattern.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/capacity-operator";

/// The modelled 400 every refusal below is answered as, except where a row says otherwise.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The modelled 404 a resource this account does not hold is answered as.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The body of an answer, for a failure message.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// The `message` of a wire error, however the protocol spells the member.
fn message_of(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// Assert `answer` is the refusal `code` carrying exactly `expected`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str, expected: &str) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (status, code.to_string()),
        "case {case:?}: expected {status} {code}, got {} {}",
        answer.status,
        body_text(answer)
    );
    let reported = message_of(answer);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the message {expected:?}, got {reported:?}"
    );
}

/// Create one function running `runtime` as `role`, with the request members `extras`
/// adds, and answer the whole response so a row can assert a refusal of the create too.
async fn create_named(
    router: &MultiServiceRouter,
    name: &str,
    runtime: &str,
    role: &str,
    extras: &Value,
) -> Answer {
    let mut body = json!({
        "FunctionName": name,
        "Role": role,
        "Runtime": runtime,
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
    });
    if let Some(members) = extras.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    create_function(router, &body).await
}

/// Create one function and assert it was stored, which is what every row below starts from.
async fn create_stored(router: &MultiServiceRouter, name: &str, runtime: &str, role: &str) {
    let answer = create_named(router, name, runtime, role, &json!({})).await;
    assert_eq!(
        answer.status,
        201,
        "setup {name:?}: expected 201 from CreateFunction, got {} {}",
        answer.status,
        body_text(&answer)
    );
}

/// `PublishVersion`, which freezes the function's current configuration as its next
/// version, and answer the version number it minted.
async fn publish_version(router: &MultiServiceRouter, name: &str) -> String {
    let uri = format!("{FUNCTIONS_URI}/{name}/versions");
    let answer = json_call(router, "POST", &uri, &json!({})).await;
    assert_eq!(
        answer.status,
        201,
        "setup {name:?}: expected 201 from PublishVersion, got {} {}",
        answer.status,
        body_text(&answer)
    );
    answer
        .string("Version")
        .expect("a published version answers its Version")
}

/// `UpdateFunctionConfiguration` with the members `body` names.
async fn update_configuration(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("{FUNCTIONS_URI}/{name}/configuration");
    json_call(router, "PUT", &uri, body).await
}

/// `TagResource` on one resource ARN.
async fn tag_resource(router: &MultiServiceRouter, resource: &str, tags: &Value) -> Answer {
    let uri = format!("{TAGS_URI}/{resource}");
    json_call(router, "POST", &uri, &json!({ "Tags": tags })).await
}

/// RULE: `TagResource` applies the tags its request names, so a request naming none
/// applies nothing and is refused — whatever resource the ARN addressed — while a request
/// naming any tag at all is served.
///
/// The rows span what the rule generalises over: how many tags the request names (none,
/// one, several) and whether the ARN names a resource this account holds at all, since the
/// request is judged before anything is looked up.
#[tokio::test]
async fn a_tag_write_naming_no_tag_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_stored(&router, "tagged-fn", "python3.12", ROLE).await;

    // (case, the resource ARN, the tags, the status the write answers)
    let rows: &[(&str, String, Value, u16)] = &[
        (
            "no tag at all, on a function this account holds",
            function_arn("tagged-fn"),
            json!({}),
            400,
        ),
        (
            "no tag at all, on an ARN naming no function here",
            function_arn("no-such-function"),
            json!({}),
            400,
        ),
        (
            "one tag",
            function_arn("tagged-fn"),
            json!({ "team": "payments" }),
            204,
        ),
        (
            "several tags",
            function_arn("tagged-fn"),
            json!({ "team": "payments", "tier": "gold", "owner": "platform" }),
            204,
        ),
    ];

    for (case, resource, tags, status) in rows {
        let answer = tag_resource(&router, resource, tags).await;
        if *status == 400 {
            assert_refused(
                case,
                &answer,
                400,
                INVALID_PARAMETER,
                "An error occurred and the request cannot be processed.",
            );
        } else {
            assert_eq!(
                answer.status,
                *status,
                "case {case:?}: expected {status}, got {} {}",
                answer.status,
                body_text(&answer)
            );
        }
    }

    // ... and the tags the served writes named are the ones the resource now carries.
    let read = json_call(
        &router,
        "GET",
        &format!("{TAGS_URI}/{}", function_arn("tagged-fn")),
        &Value::Null,
    )
    .await;
    assert_eq!(
        read.json()["Tags"],
        json!({ "team": "payments", "tier": "gold", "owner": "platform" }),
        "the served writes must be the tag set a read reports, got {}",
        body_text(&read)
    );
}

/// RULE: an ARN that qualifies a function with a version or an alias names no tag set at
/// all, and every operation on the tag path refuses it with one fixed sentence that carries
/// nothing of the request.
///
/// The rows span what the rule generalises over: the qualifier's kind (a published version
/// number, an alias name) and the operation addressed at it (reading the tags, adding one,
/// removing one).
#[tokio::test]
async fn tags_on_a_qualified_function_arn_are_refused_with_one_sentence() {
    /// The sentence AWS answers, spelled out here rather than read from the crate's
    /// constant so a change to it fails this suite.
    const QUALIFIED_MESSAGE: &str =
        "Tags on function aliases and versions are not supported. Please specify a function ARN.";

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_stored(&router, "qualified-fn", "python3.12", ROLE).await;
    let version = publish_version(&router, "qualified-fn").await;
    let alias = json_call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/qualified-fn/aliases"),
        &json!({ "Name": "live", "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "setup: expected 201 from CreateAlias, got {} {}",
        alias.status,
        body_text(&alias)
    );

    let qualifiers = [("a published version", version.as_str()), ("an alias", "live")];
    for (kind, qualifier) in qualifiers {
        let resource = format!("{}:{qualifier}", function_arn("qualified-fn"));
        let calls: &[(&str, &str, String, Value)] = &[
            (
                "ListTags",
                "GET",
                format!("{TAGS_URI}/{resource}"),
                Value::Null,
            ),
            (
                "TagResource",
                "POST",
                format!("{TAGS_URI}/{resource}"),
                json!({ "Tags": { "team": "payments" } }),
            ),
            (
                "UntagResource",
                "DELETE",
                format!("{TAGS_URI}/{resource}?tagKeys=team"),
                Value::Null,
            ),
        ];
        for (operation, method, uri, body) in calls {
            let case = format!("{operation} on {kind}");
            let answer = json_call(&router, method, uri, body).await;
            assert_refused(&case, &answer, 400, INVALID_PARAMETER, QUALIFIED_MESSAGE);
        }
    }
}

/// RULE: the unpublished version exists for as long as the function does, so a qualified
/// delete naming it is refused with one fixed sentence — however the request spells the
/// qualifier.
#[tokio::test]
async fn deleting_the_unpublished_version_on_its_own_is_refused() {
    /// The sentence AWS answers, trailing stop included.
    const LATEST_MESSAGE: &str = "$LATEST version cannot be deleted without deleting the function.";

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_stored(&router, "latest-fn", "python3.12", ROLE).await;

    // The two ways a request qualifies a name, and both must answer the same sentence.
    let rows: &[(&str, String)] = &[
        (
            "the Qualifier parameter",
            format!("{FUNCTIONS_URI}/latest-fn?Qualifier=$LATEST"),
        ),
        (
            "a qualifier on the name",
            format!("{FUNCTIONS_URI}/latest-fn:$LATEST"),
        ),
        (
            "a qualified function ARN",
            format!("{FUNCTIONS_URI}/{}:$LATEST", function_arn("latest-fn")),
        ),
    ];
    for (case, uri) in rows {
        let answer = json_call(&router, "DELETE", uri, &Value::Null).await;
        assert_refused(case, &answer, 400, INVALID_PARAMETER, LATEST_MESSAGE);
    }

    // ... and the function is still there, because a refused delete removes nothing.
    let read = json_call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/latest-fn"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "a refused delete must leave the function in place, got {} {}",
        read.status,
        body_text(&read)
    );
}

/// RULE: the operations on a layer version's POLICY name the layer version they could not
/// find by its FULL `LayerVersionArn`, and the sentence ends in a stop.
///
/// The rows span what the rule generalises over: why the version is not held (no layer of
/// that name, a version number the layer never had), the form the request names the layer
/// in (a bare name, a layer ARN), and the operation addressed at it.
#[tokio::test]
async fn a_layer_version_nothing_holds_is_named_by_its_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let published = json_call(
        &router,
        "POST",
        "/2018-10-31/layers/held-layer/versions",
        &json!({ "Content": { "ZipFile": zip_file(&package_bytes(72)) } }),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "setup: expected 201 from PublishLayerVersion, got {} {}",
        published.status,
        body_text(&published)
    );

    /// The ARN a layer version carries.
    fn layer_version_arn(name: &str, version: i64) -> String {
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{name}:{version}")
    }

    // (case, the layer as the request names it, the version number, the layer the ARN
    //  in the refusal names)
    let rows: &[(&str, String, i64, &str)] = &[
        (
            "a layer nothing was ever published under",
            "ghost-layer".to_string(),
            1,
            "ghost-layer",
        ),
        (
            "a version number the layer never had",
            "held-layer".to_string(),
            9,
            "held-layer",
        ),
        (
            "a layer addressed by its ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:ghost-layer"),
            4,
            "ghost-layer",
        ),
    ];

    for (case, layer, version, named) in rows {
        let expected = format!(
            "Layer version {} does not exist.",
            layer_version_arn(named, *version)
        );
        let calls: &[(&str, &str, String, Value)] = &[
            (
                "AddLayerVersionPermission",
                "POST",
                format!("/2018-10-31/layers/{layer}/versions/{version}/policy"),
                json!({
                    "StatementId": "share",
                    "Action": "lambda:GetLayerVersion",
                    "Principal": ACCOUNT,
                }),
            ),
            (
                "RemoveLayerVersionPermission",
                "DELETE",
                format!("/2018-10-31/layers/{layer}/versions/{version}/policy/share"),
                Value::Null,
            ),
            (
                "GetLayerVersionPolicy",
                "GET",
                format!("/2018-10-31/layers/{layer}/versions/{version}/policy"),
                Value::Null,
            ),
        ];
        for (operation, method, uri, body) in calls {
            let label = format!("{operation} for {case}");
            let answer = json_call(&router, method, uri, body).await;
            assert_refused(&label, &answer, 404, NOT_FOUND, &expected);
        }
    }
}

/// RULE: a `FunctionName` that qualifies one version and a `Qualifier` naming another name
/// no single resource, and the refusal is AWS's fixed sentence for that condition — it
/// carries neither the name, the derived qualifier nor the specified one.
///
/// The rows span what the rule generalises over: the two qualifiers' kinds (a version
/// number, the unpublished version, an alias name), the form the name is given in (a bare
/// `name:qualifier`, a qualified ARN) and the operation that resolved the pair.
#[tokio::test]
async fn a_name_and_a_qualifier_that_disagree_are_refused_with_one_sentence() {
    /// The sentence AWS answers for this condition.
    const DERIVED_MESSAGE: &str =
        "The derived qualifier from the function name does not match the specified qualifier.";

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_stored(&router, "pair-fn", "python3.12", ROLE).await;
    publish_version(&router, "pair-fn").await;

    // (case, the FunctionName as the request spells it, the Qualifier beside it)
    let pairs: &[(&str, String, &str)] = &[
        ("a version against the unpublished version", "pair-fn:1".to_string(), "$LATEST"),
        ("two different version numbers", "pair-fn:1".to_string(), "2"),
        ("a version against an alias name", "pair-fn:1".to_string(), "live"),
        (
            "a qualified ARN against another version",
            format!("{}:1", function_arn("pair-fn")),
            "3",
        ),
    ];

    for (case, name, qualifier) in pairs {
        let calls: &[(&str, &str, String)] = &[
            (
                "GetFunction",
                "GET",
                format!("{FUNCTIONS_URI}/{name}?Qualifier={qualifier}"),
            ),
            (
                "GetFunctionConfiguration",
                "GET",
                format!("{FUNCTIONS_URI}/{name}/configuration?Qualifier={qualifier}"),
            ),
            (
                "DeleteFunction",
                "DELETE",
                format!("{FUNCTIONS_URI}/{name}?Qualifier={qualifier}"),
            ),
            (
                "Invoke",
                "POST",
                format!("{FUNCTIONS_URI}/{name}/invocations?Qualifier={qualifier}"),
            ),
        ];
        for (operation, method, uri) in calls {
            let label = format!("{operation} with {case}");
            let answer = json_call(&router, method, uri, &Value::Null).await;
            assert_refused(&label, &answer, 400, INVALID_PARAMETER, DERIVED_MESSAGE);
        }
    }
}

/// The variables of an environment whose SERIALIZED form measures exactly `measured`
/// bytes, spread over `count` variables.
///
/// The budget is on the set as one object, so a row states the size it wants and this
/// builds a set of that size: the keys are plain, so the serialized form is the entries
/// and the punctuation between them and nothing is escaped.
fn environment_of(count: usize, measured: usize) -> BTreeMap<String, String> {
    let mut variables: BTreeMap<String, String> = (0..count)
        .map(|index| (format!("VAR_{index}"), String::new()))
        .collect();
    let empty = serde_json::to_string(&variables).expect("a map of strings serializes");
    let padding = measured
        .checked_sub(empty.len())
        .expect("the requested size must fit the keys");
    let last = variables
        .keys()
        .next_back()
        .expect("at least one variable")
        .clone();
    variables.insert(last, "v".repeat(padding));
    let rendered = serde_json::to_string(&variables).expect("a map of strings serializes");
    assert_eq!(
        rendered.len(),
        measured,
        "the built environment must measure exactly {measured} bytes"
    );
    variables
}

/// RULE: the published 4 KB environment budget is measured on the variables as ONE
/// serialized object — a set measuring exactly 4096 bytes is accepted and one measuring
/// 4097 is refused, with the string that was measured in the refusal — and both write paths
/// apply it.
///
/// The rows span what the rule generalises over: the measured size (just under, exactly at,
/// one byte over and far over the budget) and how many variables carry it, which changes
/// the punctuation the measure counts.
#[tokio::test]
async fn the_environment_budget_is_measured_on_the_serialized_variables() {
    /// The published quota, in bytes ("Environment variables: 4 KB").
    const BUDGET: usize = 4 * 1024;

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_stored(&router, "env-fn", "python3.12", ROLE).await;
    // A function is not modifiable until it has finished being created, so the rows start
    // from the settled record a caller that waited would see.
    make_update_due(&engine);

    // (case, how many variables, the size their serialized form measures)
    let rows: &[(&str, usize, usize)] = &[
        ("one variable, well inside the budget", 1, 512),
        ("one variable, exactly at the budget", 1, BUDGET),
        ("one variable, one byte over the budget", 1, BUDGET + 1),
        ("three variables, exactly at the budget", 3, BUDGET),
        ("three variables, one byte over the budget", 3, BUDGET + 1),
        ("eight variables, far over the budget", 8, BUDGET * 2),
    ];

    for (index, (case, count, measured)) in rows.iter().enumerate() {
        let variables = environment_of(*count, *measured);
        let rendered = serde_json::to_string(&variables).expect("a map of strings serializes");
        let environment = json!({ "Variables": variables });
        let refused = *measured > BUDGET;
        // Each row starts from a settled function, whatever the row before it changed.
        make_update_due(&engine);

        // The same budget on the operation that CREATES the configuration...
        let name = format!("env-row-{index}");
        let create = create_named(
            &router,
            &name,
            "python3.12",
            ROLE,
            &json!({ "Environment": environment.clone() }),
        )
        .await;
        // ... and on the one that updates it.
        let update = update_configuration(
            &router,
            "env-fn",
            &json!({ "Environment": environment.clone() }),
        )
        .await;

        if refused {
            let expected = format!(
                "Lambda was unable to configure your environment variables because the \
                 environment variables you have provided exceeded the 4KB limit. String \
                 measured: {rendered}"
            );
            assert_refused(
                &format!("CreateFunction with {case}"),
                &create,
                400,
                INVALID_PARAMETER,
                &expected,
            );
            assert_refused(
                &format!("UpdateFunctionConfiguration with {case}"),
                &update,
                400,
                INVALID_PARAMETER,
                &expected,
            );
        } else {
            assert_eq!(
                (create.status, update.status),
                (201, 200),
                "case {case:?}: expected the create and the update to be served, got {} {} / \
                 {} {}",
                create.status,
                body_text(&create),
                update.status,
                body_text(&update)
            );
            assert_eq!(
                update.json()["Environment"]["Variables"],
                json!(variables),
                "case {case:?}: expected the stored variables to be the ones sent, got {}",
                body_text(&update)
            );
        }
    }
}

/// RULE: SnapStart asks Lambda to snapshot an initialised execution environment when a
/// version is published, which only the runtime families that carry the snapshot/restore
/// machinery can do — Java 11 and later, Python 3.12 and later, .NET 8 and later — so a
/// request applying it to any other runtime is refused, on both operations that write a
/// configuration.
///
/// The rows are each supported family at its published minimum, a release just below two of
/// those minima, and families that support it in no version at all. Source for the set:
/// AWS's SnapStart documentation, "Supported runtimes".
#[tokio::test]
async fn snap_start_is_refused_on_a_runtime_that_does_not_support_it() {
    let rows: &[(&str, bool)] = &[
        ("java11", true),
        ("java21", true),
        ("python3.12", true),
        ("python3.13", true),
        ("dotnet8", true),
        ("java8.al2", false),
        ("python3.11", false),
        ("dotnet6", false),
        ("nodejs22.x", false),
        ("ruby3.4", false),
    ];

    for (runtime, supported) in rows {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let expected = format!("SnapStart is not supported for the runtime {runtime}");

        // Applied by the create...
        let created = create_named(
            &router,
            "snap-at-create",
            runtime,
            ROLE,
            &json!({ "SnapStart": { "ApplyOn": "PublishedVersions" } }),
        )
        .await;
        // ... and applied by an update to a function created without it.
        create_stored(&router, "snap-at-update", runtime, ROLE).await;
        make_update_due(&engine);
        let updated = update_configuration(
            &router,
            "snap-at-update",
            &json!({ "SnapStart": { "ApplyOn": "PublishedVersions" } }),
        )
        .await;

        if *supported {
            assert_eq!(
                (created.status, updated.status),
                (201, 200),
                "case {runtime:?}: expected both writes to be served, got {} {} / {} {}",
                created.status,
                body_text(&created),
                updated.status,
                body_text(&updated)
            );
            assert_eq!(
                updated.json()["SnapStart"]["ApplyOn"],
                json!("PublishedVersions"),
                "case {runtime:?}: expected the applied setting to be stored, got {}",
                body_text(&updated)
            );
        } else {
            assert_refused(
                &format!("CreateFunction on {runtime}"),
                &created,
                400,
                INVALID_PARAMETER,
                &expected,
            );
            assert_refused(
                &format!("UpdateFunctionConfiguration on {runtime}"),
                &updated,
                400,
                INVALID_PARAMETER,
                &expected,
            );
            // Turning SnapStart OFF is no application of it, so it is served on every
            // runtime.
            make_update_due(&engine);
            let off = update_configuration(
                &router,
                "snap-at-update",
                &json!({ "SnapStart": { "ApplyOn": "None" } }),
            )
            .await;
            assert_eq!(
                off.status,
                200,
                "case {runtime:?}: expected ApplyOn None to be served, got {} {}",
                off.status,
                body_text(&off)
            );
        }
    }
}

/// RULE: provisioned concurrency keeps initialised environments of a version warm and
/// SnapStart restores each environment from a snapshot instead, so one version cannot be
/// served both ways: `PutProvisionedConcurrencyConfig` on a version whose configuration
/// applies SnapStart is refused, and the same call on a version that does not is served.
#[tokio::test]
async fn provisioned_concurrency_and_snap_start_are_exclusive() {
    /// The sentence the refusal carries.
    const EXCLUSIVE_MESSAGE: &str = "Provisioned concurrency cannot be configured on a function \
                                     version with SnapStart enabled";

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // One function with SnapStart applied and one without, each with a published version.
    let snap = create_named(
        &router,
        "snap-pc",
        "python3.12",
        ROLE,
        &json!({ "SnapStart": { "ApplyOn": "PublishedVersions" } }),
    )
    .await;
    assert_eq!(
        snap.status,
        201,
        "setup: expected 201 from CreateFunction, got {} {}",
        snap.status,
        body_text(&snap)
    );
    create_stored(&router, "plain-pc", "python3.12", ROLE).await;
    make_update_due(&engine);

    for (case, function, refused) in [
        ("a version with SnapStart applied", "snap-pc", true),
        ("a version without it", "plain-pc", false),
    ] {
        let version = publish_version(&router, function).await;
        let answer = json_call(
            &router,
            "PUT",
            &format!(
                "{PROVISIONED_CONCURRENCY_PREFIX}/{function}/provisioned-concurrency\
                 ?Qualifier={version}"
            ),
            &json!({ "ProvisionedConcurrentExecutions": 1 }),
        )
        .await;
        if refused {
            assert_refused(case, &answer, 409, "ResourceConflictException", EXCLUSIVE_MESSAGE);
            // ... and nothing was allocated, so the read reports no configuration.
            let read = json_call(
                &router,
                "GET",
                &format!(
                    "{PROVISIONED_CONCURRENCY_PREFIX}/{function}/provisioned-concurrency\
                     ?Qualifier={version}"
                ),
                &Value::Null,
            )
            .await;
            assert_eq!(
                read.status,
                404,
                "case {case:?}: a refused allocation must leave no configuration, got {} {}",
                read.status,
                body_text(&read)
            );
        } else {
            assert_eq!(
                answer.status,
                202,
                "case {case:?}: expected 202, got {} {}",
                answer.status,
                body_text(&answer)
            );
        }
    }
}

/// RULE: an alias that splits its invocations shifts traffic between TWO snapshots of the
/// function that run as ONE execution role: at most one additional version, never the
/// unpublished version as the version it invokes, and no second version whose configuration
/// names another role. Both write paths apply it.
///
/// The rows span what the rule generalises over: which half of the routing configuration is
/// wrong (the count, the primary version, the roles) and the acceptable shape beside them.
#[tokio::test]
async fn a_weighted_alias_routes_between_two_versions_of_one_role() {
    /// The three sentences AWS answers, spelled out here so a change to the crate's
    /// constants fails this suite.
    const TOO_MANY: &str = "Number of items in AdditionalVersionWeights cannot be greater than 1";
    const ON_LATEST: &str = "Weighted alias cannot be mapped to $LATEST";
    const ROLE_MISMATCH: &str =
        "Weighted alias cannot be mapped to versions with different execution roles";

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_stored(&router, "weighted-fn", "python3.12", ROLE).await;
    make_update_due(&engine);
    // Versions 1 and 2 run as one role; version 3 is published after the role changed, so
    // it runs as another. A publish freezes the CURRENT configuration, so each one follows
    // a change to it — publishing an unchanged function answers the version it already
    // published.
    let first = publish_version(&router, "weighted-fn").await;
    let described = update_configuration(
        &router,
        "weighted-fn",
        &json!({ "Description": "the second snapshot" }),
    )
    .await;
    assert_eq!(
        described.status,
        200,
        "setup: expected 200 from UpdateFunctionConfiguration, got {} {}",
        described.status,
        body_text(&described)
    );
    make_update_due(&engine);
    let second = publish_version(&router, "weighted-fn").await;
    let moved = update_configuration(&router, "weighted-fn", &json!({ "Role": OTHER_ROLE })).await;
    assert_eq!(
        moved.status,
        200,
        "setup: expected 200 from UpdateFunctionConfiguration, got {} {}",
        moved.status,
        body_text(&moved)
    );
    let third = publish_version(&router, "weighted-fn").await;
    assert_ne!(
        (first.as_str(), second.as_str()),
        (second.as_str(), third.as_str()),
        "setup: the three snapshots must be three different versions"
    );

    // (case, the version the alias invokes, its routing weights, the refusal or `None`)
    let rows: &[(&str, &str, Value, Option<&str>)] = &[
        (
            "two additional versions",
            first.as_str(),
            json!({ second.clone(): 0.3, third.clone(): 0.2 }),
            Some(TOO_MANY),
        ),
        (
            "the unpublished version as the version it invokes",
            "$LATEST",
            json!({ second.clone(): 0.5 }),
            Some(ON_LATEST),
        ),
        (
            "a second version running as another role",
            first.as_str(),
            json!({ third.clone(): 0.5 }),
            Some(ROLE_MISMATCH),
        ),
        (
            "one additional version of the same role",
            first.as_str(),
            json!({ second.clone(): 0.5 }),
            None,
        ),
    ];

    for (index, (case, primary, weights, refusal)) in rows.iter().enumerate() {
        let alias = format!("weighted-{index}");
        let body = json!({
            "Name": alias,
            "FunctionVersion": primary,
            "RoutingConfig": { "AdditionalVersionWeights": weights },
        });
        let created = json_call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/weighted-fn/aliases"),
            &body,
        )
        .await;

        // The same rule on UpdateAlias, applied to an alias that already exists.
        let held = format!("held-{index}");
        let setup = json_call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/weighted-fn/aliases"),
            &json!({ "Name": held, "FunctionVersion": first }),
        )
        .await;
        assert_eq!(
            setup.status,
            201,
            "case {case:?}: expected 201 from the alias the update acts on, got {} {}",
            setup.status,
            body_text(&setup)
        );
        let updated = json_call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/weighted-fn/aliases/{held}"),
            &json!({
                "FunctionVersion": primary,
                "RoutingConfig": { "AdditionalVersionWeights": weights },
            }),
        )
        .await;

        match refusal {
            Some(expected) => {
                assert_refused(
                    &format!("CreateAlias with {case}"),
                    &created,
                    400,
                    INVALID_PARAMETER,
                    expected,
                );
                assert_refused(
                    &format!("UpdateAlias with {case}"),
                    &updated,
                    400,
                    INVALID_PARAMETER,
                    expected,
                );
                // Nothing was created for the refused alias.
                let read = json_call(
                    &router,
                    "GET",
                    &format!("{FUNCTIONS_URI}/weighted-fn/aliases/{alias}"),
                    &Value::Null,
                )
                .await;
                assert_eq!(
                    read.status,
                    404,
                    "case {case:?}: a refused create must leave no alias, got {} {}",
                    read.status,
                    body_text(&read)
                );
            }
            None => {
                assert_eq!(
                    (created.status, updated.status),
                    (201, 200),
                    "case {case:?}: expected both writes to be served, got {} {} / {} {}",
                    created.status,
                    body_text(&created),
                    updated.status,
                    body_text(&updated)
                );
                assert_eq!(
                    created.json()["RoutingConfig"]["AdditionalVersionWeights"],
                    *weights,
                    "case {case:?}: expected the stored weights to be the ones sent, got {}",
                    body_text(&created)
                );
            }
        }
    }
}

/// RULE: a pagination `Marker` is the token a previous page of the SAME listing answered —
/// the qualified ARN of its last entry — so a value in any other form names no position in
/// the order and is refused, while the token a page answered walks on from it.
#[tokio::test]
async fn a_version_listing_refuses_a_marker_it_did_not_mint() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let provider = "fleet-marker";
    let created = json_call(
        &router,
        "POST",
        CAPACITY_PROVIDERS_URI,
        &json!({
            "CapacityProviderName": provider,
            "VpcConfig": { "SubnetIds": ["subnet-10000000"], "SecurityGroupIds": [] },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "setup: expected 202 from CreateCapacityProvider, got {} {}",
        created.status,
        body_text(&created)
    );
    let provider_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{provider}");
    let function = create_named(
        &router,
        "marker-fn",
        "python3.12",
        ROLE,
        &json!({
            "CapacityProviderConfig": {
                "LambdaManagedInstancesCapacityProviderConfig": {
                    "CapacityProviderArn": provider_arn,
                },
            },
        }),
    )
    .await;
    assert_eq!(
        function.status,
        201,
        "setup: expected 201 from CreateFunction, got {} {}",
        function.status,
        body_text(&function)
    );
    make_activation_due(&engine);
    let first = publish_version(&router, "marker-fn").await;
    let described = update_configuration(
        &router,
        "marker-fn",
        &json!({ "Description": "the second snapshot" }),
    )
    .await;
    assert_eq!(
        described.status,
        200,
        "setup: expected 200 from UpdateFunctionConfiguration, got {} {}",
        described.status,
        body_text(&described)
    );
    make_update_due(&engine);
    publish_version(&router, "marker-fn").await;

    let listing_uri = |marker: Option<&str>| {
        let base = format!("{CAPACITY_PROVIDERS_URI}/{provider}/function-versions");
        match marker {
            Some(marker) => format!("{base}?Marker={marker}"),
            None => base,
        }
    };

    // The whole listing, which is what a marker walks on from.
    let whole = json_call(&router, "GET", &listing_uri(None), &Value::Null).await;
    assert_eq!(
        whole.status,
        200,
        "the listing must be served, got {} {}",
        whole.status,
        body_text(&whole)
    );
    assert_eq!(
        whole.json()["FunctionVersions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        2,
        "both published versions must be reported, got {}",
        body_text(&whole)
    );

    // (case, the marker, whether the listing serves it)
    let minted = format!("{}:{first}", function_arn("marker-fn"));
    let rows: &[(&str, String, bool)] = &[
        ("the ARN of a page's last entry", minted.clone(), true),
        ("a bare function name", "marker-fn".to_string(), false),
        (
            "an unqualified function ARN",
            function_arn("marker-fn"),
            false,
        ),
        ("a token this listing never answered", "page-2".to_string(), false),
    ];

    for (case, marker, served) in rows {
        let answer = json_call(&router, "GET", &listing_uri(Some(marker)), &Value::Null).await;
        if *served {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected 200, got {} {}",
                answer.status,
                body_text(&answer)
            );
            assert_eq!(
                answer.json()["FunctionVersions"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                1,
                "case {case:?}: expected the page after the marker, got {}",
                body_text(&answer)
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, INVALID_PARAMETER.to_string()),
                "case {case:?}: expected 400 {INVALID_PARAMETER}, got {} {}",
                answer.status,
                body_text(&answer)
            );
        }
    }
}
