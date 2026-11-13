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
    let Some((function_name, qualifier)) = super::split_lambda_resource_id(resource_id)
        .filter(|(name, qualifier)| is_lambda_name(name) && is_lambda_qualifier(qualifier))
    else {
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
    let unreachable = || Refused {
        code: "InternalFailure".to_string(),
    };
    let call = ::http::Request::builder()
        .method("GET")
        .uri(format!(
            "/2015-03-31/functions/{function_name}/configuration?Qualifier={qualifier}"
        ))
        .header(
            "host",
            format!("{LAMBDA_SIGNING_NAME}.{region}.amazonaws.com"),
        )
        .header("accept", APPLICATION_JSON)
        .body(Vec::new())
        .map_err(|_| unreachable())?;
    let answered = super::in_process_calls::call(callback, LAMBDA_SIGNING_NAME, region, call)
        .await
        .ok_or_else(unreachable)?;
    if answered.is_success() {
        return Ok(());
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
}
