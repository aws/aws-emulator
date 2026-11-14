// SPDX-License-Identifier: CC0-1.0

//! The Lambda function version or alias a `lambda` scalable target names, resolved in the
//! service that owns it.
//!
//! A scalable target is a capacity dimension OF A RESOURCE, so registering one has to resolve
//! that resource where it lives: `lambda:function:ProvisionedConcurrency` scales "the
//! provisioned concurrency for a Lambda function", which is allocated to ONE qualified
//! function — "the unique identifier is the function name with a function version or alias
//! name suffix that is not `$LATEST`" (the model's own `ResourceId` documentation) — so a
//! qualifier that names no version and no alias of that function names no concurrency to
//! scale. That is the same rule the DynamoDB and ECS resolvers apply one namespace over
//! ([`super::resolve_dynamodb_capacity`], [`super::resolve_ecs_capacity`]); only WHERE the
//! answer comes from differs.
//!
//! ## Why this one is asked over the wire
//!
//! The qualifier is resolved by CALLING the owning service — `GetFunctionConfiguration`,
//! whose `Qualifier` "specify a version or alias to get details about a published version of
//! the function", so ONE call answers for either kind — in-process over the callback
//! transport, signed the way every other in-process caller signs
//! ([`::service_common::in_process_sigv4`]). The answer is that service's own, so the two can
//! no more disagree than two readers of one store can, and this crate names no Lambda type.
//!
//! ## The capacity such a target scales
//!
//! `lambda:function:ProvisionedConcurrency` scales the provisioned concurrency OF THAT
//! qualifier, which Lambda holds as a configuration on it: the concurrency the configuration
//! asks for is the capacity ([`read_provisioned_concurrency`]), and a qualifier carrying no
//! configuration holds none of it. So the same two calls that read it also move it
//! ([`set_provisioned_concurrency`]) — the configuration is written where it asks for any
//! concurrency at all, and removed where it asks for none, since `PutProvisionedConcurrencyConfig`
//! takes a `ProvisionedConcurrentExecutions` of at least one and holding no configuration is
//! what holding no provisioned concurrency is.
//!
//! An engine on which Lambda is not mounted answers the call with the router's
//! `UnknownService`, which resolves NOTHING rather than refusing: the same answer this
//! service gives for a namespace whose owning store it cannot reach
//! ([`super::keyspaces_tables`]).

/// Lambda's signing name, which is what routes the in-process call to its own handlers, and
/// the host its endpoint is addressed on (that model's `sigv4Name: lambda`,
/// `smithy_query(query="service_info")`). It speaks `restJson1`.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// The media type a `restJson1` request and its answer carry.
const APPLICATION_JSON: &str = "application/json";

/// The error code Lambda answers for a function, version or alias it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of `GetFunctionConfiguration`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code Lambda answers for a qualifier that holds no provisioned-concurrency
/// configuration at all (`ProvisionedConcurrencyConfigNotFoundException`, the modelled 404 of
/// `GetProvisionedConcurrencyConfig`) — which is a qualifier whose provisioned concurrency is
/// none rather than one that could not be read.
const PROVISIONED_CONCURRENCY_CONFIG_NOT_FOUND: &str =
    "ProvisionedConcurrencyConfigNotFoundException";

/// The provisioned concurrency a qualifier that holds no configuration has.
const NO_PROVISIONED_CONCURRENCY: i32 = 0;

/// The member of a provisioned-concurrency configuration that carries the concurrency it asks
/// for: `RequestedProvisionedConcurrentExecutions`, "the amount of provisioned concurrency
/// requested". That is the capacity of the resource — what the configuration is set to —
/// where `Allocated` is how far the owning service has got with initialising it.
const REQUESTED_EXECUTIONS: &str = "RequestedProvisionedConcurrentExecutions";

/// The member `PutProvisionedConcurrencyConfig` takes the concurrency to provision in.
const PROVISIONED_EXECUTIONS: &str = "ProvisionedConcurrentExecutions";

/// Resolve the function version or alias a `lambda` scalable target names, in Lambda itself.
///
/// * `Ok(())` — the qualified function exists, or this emulator could not reach the owning
///   service at all, which resolves nothing rather than refusing;
/// * `Err(message)` — the qualifier names no version and no alias of that function, as the
///   `ValidationException` message naming the resource, the dimension and the ARN that was
///   looked for.
pub async fn resolve_lambda_qualifier(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<(), String> {
    // An id whose halves are outside the character set Lambda's own `FunctionName` and
    // `Qualifier` members admit names no function of that service at all, so there is nothing
    // to resolve it against; the id's FORM is checked where the form is owned
    // (`super::check_resource_id_form`).
    let Some((function_name, qualifier)) = resolvable_qualifier(resource_id) else {
        return Ok(());
    };
    match get_function_configuration(callback, region, function_name, qualifier).await {
        Ok(()) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => {
            // The refusal names the resource and the dimension the registration asked for,
            // and states as its reason the qualified function ARN that was looked for — which
            // is the ARN of the alias, or of the published version, the qualifier names.
            Err(super::resource_validation_failed(
                resource_id,
                scalable_dimension,
                &format!(
                    "Cannot find {} arn: {}",
                    qualifier_kind(qualifier),
                    qualified_function_arn(region, function_name, qualifier)
                ),
            ))
        }
        // Anything else is a fact about this process rather than about the request — the
        // owning service is not mounted on this engine, or its answer could not be read — so
        // the resource is UNRESOLVED and the registration is answered on its own terms.
        Err(_) => Ok(()),
    }
}

/// What a qualifier names: a PUBLISHED VERSION when it is a version number
/// ("`Version`: `^(\$LATEST|[0-9]+)$`"), and otherwise an alias, whose own name is kept out
/// of the version-number namespace by `Alias`'s pattern.
fn qualifier_kind(qualifier: &str) -> &'static str {
    if !qualifier.is_empty() && qualifier.bytes().all(|byte| byte.is_ascii_digit()) {
        "version"
    } else {
        "alias"
    }
}

/// The ARN of one qualified Lambda function:
/// `arn:aws:lambda:<region>:<account>:function:<name>:<qualifier>`, the form `FunctionArn`
/// declares for a function carrying "a version number or alias suffix".
fn qualified_function_arn(region: &str, function_name: &str, qualifier: &str) -> String {
    ::emulator_server::emulator_arn(
        LAMBDA_SIGNING_NAME,
        region,
        &format!("function:{function_name}:{qualifier}"),
    )
}

/// Whether `value` is within the character set Lambda's `FunctionName` admits for a bare
/// function name (`[a-zA-Z0-9-_]+`, per that member's own pattern).
fn is_lambda_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Whether `value` is within the character set a qualifier takes: an alias name
/// (`Alias`: `[a-zA-Z0-9-_]+`) or a published version number (`Version`: `[0-9]+`), which is
/// the same set.
fn is_lambda_qualifier(value: &str) -> bool {
    is_lambda_name(value)
}

/// The provisioned concurrency the qualifier a `lambda` scalable target names holds right
/// now, read in Lambda itself — `None` when this emulator could not read it at all.
///
/// A qualifier Lambda holds NO configuration for holds no provisioned concurrency, which is
/// [`NO_PROVISIONED_CONCURRENCY`] and not an unknown: it is the capacity a registered window is
/// then enforced on, so registering a floor above it is what raises it.
pub async fn read_provisioned_concurrency(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
) -> Option<i32> {
    let (function_name, qualifier) = resolvable_qualifier(resource_id)?;
    match call(
        callback,
        region,
        "GET",
        &provisioned_concurrency_path(function_name, qualifier),
        Vec::new(),
    )
    .await
    {
        Ok(answered) => requested_executions(&answered),
        Err(refusal) if refusal.code == PROVISIONED_CONCURRENCY_CONFIG_NOT_FOUND => {
            Some(NO_PROVISIONED_CONCURRENCY)
        }
        // Anything else is a fact about this process or about a qualifier that has since gone,
        // so there is no capacity to compare a window against.
        Err(_) => None,
    }
}

/// Move the provisioned concurrency of the qualifier a `lambda` scalable target names, in
/// Lambda itself, answering whether that qualifier took the change.
///
/// Any concurrency at all is the configuration asking for it; none is the configuration not
/// being there, which is what Lambda's own minimum of one for
/// `PutProvisionedConcurrencyConfig$ProvisionedConcurrentExecutions` leaves as the way to hold
/// nothing. Either way the qualifier ends up holding exactly `executions`, which is what a
/// client reads back there.
pub async fn set_provisioned_concurrency(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
    executions: i32,
) -> bool {
    let Some((function_name, qualifier)) = resolvable_qualifier(resource_id) else {
        return false;
    };
    let path = provisioned_concurrency_path(function_name, qualifier);
    if executions <= NO_PROVISIONED_CONCURRENCY {
        return call(callback, region, "DELETE", &path, Vec::new())
            .await
            .is_ok();
    }
    let Ok(body) = ::serde_json::to_vec(&::serde_json::json!({
        PROVISIONED_EXECUTIONS: executions,
    })) else {
        return false;
    };
    call(callback, region, "PUT", &path, body).await.is_ok()
}

/// The function name and qualifier of an id this resolver can ask Lambda about: one of the
/// documented form whose halves are within the character set that service's own `FunctionName`
/// and `Qualifier` members admit, since anything else names no function of it at all.
fn resolvable_qualifier(resource_id: &str) -> Option<(&str, &str)> {
    super::split_lambda_resource_id(resource_id)
        .filter(|(name, qualifier)| is_lambda_name(name) && is_lambda_qualifier(qualifier))
}

/// The path one qualifier's provisioned-concurrency configuration is addressed on, which is
/// the same for reading it, writing it and removing it
/// (`/2019-09-30/functions/{FunctionName}/provisioned-concurrency?Qualifier=…`).
fn provisioned_concurrency_path(function_name: &str, qualifier: &str) -> String {
    format!("/2019-09-30/functions/{function_name}/provisioned-concurrency?Qualifier={qualifier}")
}

/// The concurrency an answered provisioned-concurrency configuration asks for.
fn requested_executions(answered: &::serde_json::Value) -> Option<i32> {
    let requested = answered
        .get(REQUESTED_EXECUTIONS)
        .and_then(::serde_json::Value::as_i64)?;
    i32::try_from(requested).ok()
}

/// One refusal Lambda answered: the code a client branches on.
struct Refused {
    code: String,
}

/// Call Lambda's `GetFunctionConfiguration` for one qualified function, in-process.
///
/// The request is the one that api binds (`GET
/// /2015-03-31/functions/{FunctionName}/configuration?Qualifier=…`); everything a call of
/// another emulated service takes beyond that — the signature, the dispatch and the reading of
/// a refusal's own code — is the one in-process caller this service has
/// ([`super::in_process_calls`]).
///
/// Only WHETHER it resolved matters here — the configuration it answers is Lambda's own to
/// report — so the answer's document is not read.
async fn get_function_configuration(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    function_name: &str,
    qualifier: &str,
) -> Result<(), Refused> {
    call(
        callback,
        region,
        "GET",
        &format!("/2015-03-31/functions/{function_name}/configuration?Qualifier={qualifier}"),
        Vec::new(),
    )
    .await
    .map(|_| ())
}

/// Call one Lambda operation in-process and read the document it answers.
///
/// The request is the one that api binds — the method and path the operation declares, with
/// its input document as the body where it takes one — and everything a call of another
/// emulated service takes beyond that (the signature, the dispatch, the reading of a refusal's
/// own code) is the one in-process caller this service has ([`super::in_process_calls`]).
///
/// An answer that carries no document at all (a `204`) answers `Null`: what it says is that
/// the call succeeded.
async fn call(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<::serde_json::Value, Refused> {
    let unreachable = || Refused {
        code: "InternalFailure".to_string(),
    };
    let call = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header(
            "host",
            format!("{LAMBDA_SIGNING_NAME}.{region}.amazonaws.com"),
        )
        .header("accept", APPLICATION_JSON);
    // A call that carries no input document carries no media type for one either, exactly as
    // the request an operation with no body binds does.
    let call = if body.is_empty() {
        call
    } else {
        call.header("content-type", APPLICATION_JSON)
    };
    let call = call.body(body).map_err(|_| unreachable())?;
    let answered = super::in_process_calls::call(callback, LAMBDA_SIGNING_NAME, region, call)
        .await
        .ok_or_else(unreachable)?;
    if answered.is_success() {
        return Ok(answered.document().unwrap_or(::serde_json::Value::Null));
    }
    Err(Refused {
        code: answered.code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a qualifier that is a version NUMBER names a published version, and every other
    /// qualifier names an alias — which is what the refusal of an unresolvable one says it
    /// looked for.
    #[test]
    fn a_numeric_qualifier_names_a_version_and_any_other_an_alias() {
        /// (case, the qualifier, what it names)
        type Row<'a> = (&'a str, &'a str, &'a str);
        let rows: &[Row<'_>] = &[
            ("a published version", "3", "version"),
            ("a version past one digit", "127", "version"),
            ("an alias", "prod", "alias"),
            ("an alias carrying digits", "blue2", "alias"),
            ("an alias of punctuation and digits", "v1-0_x", "alias"),
            ("nothing at all", "", "alias"),
        ];
        for (case, qualifier, expected) in rows {
            let actual = qualifier_kind(qualifier);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the refusal looks for the ARN of the QUALIFIED function — the function name and
    /// the qualifier the request named, in the Region the request was addressed to.
    #[test]
    fn the_arn_looked_for_is_the_qualified_functions_own() {
        /// (case, region, function name, qualifier)
        type Row<'a> = (&'a str, &'a str, &'a str, &'a str);
        let rows: &[Row<'_>] = &[
            ("an alias", "us-west-2", "ingest", "never-created"),
            ("a published version", "eu-west-1", "web-api", "7"),
            ("an underscored name", "ap-south-1", "my_fn", "prod"),
        ];
        for (case, region, function_name, qualifier) in rows {
            let actual = qualified_function_arn(region, function_name, qualifier);
            let expected = format!(
                "arn:aws:lambda:{region}:{}:function:{function_name}:{qualifier}",
                ::emulator_server::EMULATOR_ACCOUNT_ID
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: only an id whose halves are within the character set Lambda's own members admit
    /// is one this resolver asks that service about.
    #[test]
    fn only_a_name_lambda_can_hold_is_resolved() {
        /// (case, the value, whether Lambda can hold it)
        type Row<'a> = (&'a str, &'a str, bool);
        let rows: &[Row<'_>] = &[
            ("a plain name", "ingest", true),
            ("hyphens and underscores", "my-fn_2", true),
            ("a version number", "12", true),
            ("a path separator", "ingest/green", false),
            ("a space", "my fn", false),
            ("a query delimiter", "ingest?x=1", false),
            ("a dollar", "$LATEST", false),
            ("nothing at all", "", false),
        ];
        for (case, value, expected) in rows {
            let actual = is_lambda_name(value);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the provisioned concurrency of a qualifier is read, written and removed on the
    /// one path Lambda binds that configuration to, with the qualifier named as the query
    /// parameter the operations declare.
    #[test]
    fn one_path_addresses_a_qualifiers_provisioned_concurrency() {
        /// (case, function name, qualifier)
        type Row<'a> = (&'a str, &'a str, &'a str);
        let rows: &[Row<'_>] = &[
            ("an alias", "ingest", "BLUE"),
            ("a published version", "web-api", "7"),
            ("an underscored name", "my_fn", "prod-1"),
        ];
        for (case, function_name, qualifier) in rows {
            let actual = provisioned_concurrency_path(function_name, qualifier);
            let expected = format!(
                "/2019-09-30/functions/{function_name}/provisioned-concurrency?Qualifier={qualifier}"
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the capacity of such a resource is the concurrency the configuration ASKS FOR,
    /// whatever the owning service has allocated of it so far; an answer carrying no such
    /// member carries no capacity.
    #[test]
    fn the_capacity_is_the_concurrency_the_configuration_asks_for() {
        /// (case, the answered document, the capacity it reports)
        type Row<'a> = (&'a str, ::serde_json::Value, Option<i32>);
        let rows: &[Row<'_>] = &[
            (
                "an allocation still under way",
                ::serde_json::json!({
                    "RequestedProvisionedConcurrentExecutions": 4,
                    "AllocatedProvisionedConcurrentExecutions": 0,
                    "Status": "IN_PROGRESS",
                }),
                Some(4),
            ),
            (
                "an allocation that is ready",
                ::serde_json::json!({
                    "RequestedProvisionedConcurrentExecutions": 9,
                    "AllocatedProvisionedConcurrentExecutions": 9,
                    "Status": "READY",
                }),
                Some(9),
            ),
            (
                "a document naming no requested concurrency",
                ::serde_json::json!({"Status": "READY"}),
                None,
            ),
            ("no document at all", ::serde_json::Value::Null, None),
        ];
        for (case, answered, expected) in rows {
            let actual = requested_executions(answered);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a `lambda` resource id this resolver can ask Lambda about is one of the documented
    /// form whose BOTH halves are names that service can hold.
    #[test]
    fn a_resolvable_id_names_a_function_and_a_qualifier_lambda_can_hold() {
        /// (case, the resource id, the halves it resolves to)
        type Row<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
        let rows: &[Row<'_>] = &[
            (
                "an alias",
                "function:ingest:BLUE",
                Some(("ingest", "BLUE")),
            ),
            (
                "a published version",
                "function:web-api:7",
                Some(("web-api", "7")),
            ),
            ("the unpublished version", "function:ingest:$LATEST", None),
            ("no qualifier", "function:ingest", None),
            ("another namespace's id", "table/orders", None),
            ("a space in the name", "function:my fn:prod", None),
            ("nothing at all", "", None),
        ];
        for (case, resource_id, expected) in rows {
            let actual = resolvable_qualifier(resource_id);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
