// SPDX-License-Identifier: CC0-1.0

//! The EXECUTION ROLE behind a Lambda@Edge association, and whether the edge can run the
//! function at all under it.
//!
//! A `LambdaFunctionAssociation` names a published function version CloudFront invokes on a
//! cache behaviour's event, and CloudFront invokes it by assuming the function's own
//! execution role as the EDGE principal ([`EDGE_LAMBDA_SERVICE_PRINCIPAL`]) beside the
//! principal Lambda itself assumes it as ([`LAMBDA_SERVICE_PRINCIPAL`]). A role whose trust
//! policy withholds either of those two assumes is therefore a role the association could
//! never fire under, so the configuration is refused on the way IN —
//! `InvalidLambdaFunctionAssociation`, HTTP 400, with the sentence
//! [`edge_lambda_role_trust_message`] words it in — rather than stored as a trigger that
//! can only fail later.
//!
//! Both facts the rule needs belong to OTHER services, so both are asked of the service
//! that holds them, over the emulator's own in-process transport (the operation callback,
//! which opens no socket and reaches nothing outside this process):
//!
//! * WHICH role the function runs as is Lambda's record of that function version
//!   (`FunctionConfiguration$Role`, "the function's execution role"), read with
//!   `GetFunctionConfiguration` ([`execution_role_of`]);
//! * whether that role admits a principal is IAM's record of the role's
//!   `AssumeRolePolicyDocument`, read through the one shared reading of a stored trust
//!   document (`::service_common::iam_role::held_role_is_unassumable`), so no two services
//!   handed the same document answer differently.
//!
//! Only what those services ANSWERED is refused. A configuration is left alone when the
//! association names no published function version at all (that is the ARN rule's own
//! refusal, [`super::lambda_association_without_version`], which answers first), when the
//! ARN names another account's function, when Lambda holds no such function or version,
//! when this account's IAM holds no such role, and when nothing answered the hop — a role
//! and a function are routinely provisioned out of band, so their absence here is evidence
//! about this emulator's stores rather than about the association.
//!
//! ## What ONE request can spend here
//!
//! How many associations a submitted configuration carries is the CALLER's to choose — the
//! default cache behaviour and every ordered one carry a list each — so the reads one
//! request drives are bounded before any of them is made:
//!
//! * the ARNs are DE-DUPLICATED, so a configuration repeating one association pays for it
//!   once, and then TRUNCATED to [`MAX_EDGE_ROLES_RESOLVED_PER_REQUEST`];
//! * the role each resolved ARN names is judged ONCE, so associations sharing an execution
//!   role share its answer;
//! * one Lambda client serves every association of a Region.
//!
//! So one request makes at most [`MAX_EDGE_ROLES_RESOLVED_PER_REQUEST`] reads of Lambda's
//! records and at most two of IAM's per distinct role, whatever the body carried. The
//! handlers answer every refusal that needs no read of another service FIRST, so a request
//! that was going to be refused anyway pays none of these.

use crate::ops::types::DistributionConfig;

/// The service principal LAMBDA assumes a function's execution role as, which every
/// execution role trusts.
pub const LAMBDA_SERVICE_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The service principal the CloudFront EDGE assumes a Lambda@Edge function's execution role
/// as. It is what makes the function invocable from an edge location, and the thing a role
/// that was only ever set up for Lambda itself is missing.
pub const EDGE_LAMBDA_SERVICE_PRINCIPAL: &str = "edgelambda.amazonaws.com";

/// How many Lambda@Edge execution roles ONE request resolves.
///
/// An EMULATOR ceiling, not an AWS quota: each association resolved costs one read of
/// Lambda's records and up to two of IAM's, and a submitted configuration may state a list
/// of associations on the default cache behaviour and on each of its up to
/// [`super::MAX_CACHE_BEHAVIORS`] ordered ones — a count a caller chooses, and one the
/// quantity agreements do not bound — so the work one call performs is bounded here, in the
/// place that performs it, rather than by each factor separately (the same shape bounds the
/// destinations Amazon S3 resolves for one write).
///
/// It is the number of distinct functions a configuration can LEGALLY associate, so no
/// configuration this service stores goes unjudged: CloudFront runs at most one function per
/// cache behaviour per EVENT, and `EventType` declares four events
/// (`smithy_query(query="shape",shape="EventType",model_path="services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json")`),
/// so the most that can ever fire is four per behaviour over the default behaviour and the
/// [`super::MAX_CACHE_BEHAVIORS`] ordered ones — and a configuration carrying more ordered
/// behaviours than that is refused before this is reached
/// ([`super::config_refusal_without_state`]). Past the ceiling the remaining associations
/// are not resolved, exactly as a read nothing answered is not: nothing is refused on
/// evidence this emulator did not gather.
pub const MAX_EDGE_ROLES_RESOLVED_PER_REQUEST: usize = 4 * (super::MAX_CACHE_BEHAVIORS + 1);

/// The sentence `InvalidLambdaFunctionAssociation` carries for an association whose function
/// execution role does not trust both principals the edge needs: the fixed text naming the
/// two principals and the fix, and then the ARN of the role the association resolved to.
///
/// The role ARN is the only variable part, and it comes from the EMULATOR's own records —
/// the role Lambda reports for the function version the request named — because that is the
/// role whose trust policy the caller has to update.
pub fn edge_lambda_role_trust_message(role_arn: &str) -> String {
    format!(
        "The function execution role must be assumable with \
         {EDGE_LAMBDA_SERVICE_PRINCIPAL} as well as {LAMBDA_SERVICE_PRINCIPAL} principals. \
         Update the IAM role and try again. Role: {role_arn}"
    )
}

/// The [`edge_lambda_role_trust_message`] for the FIRST Lambda@Edge association of `config`
/// whose execution role this emulator's records show the edge could not assume, or `None`
/// when no association of it resolves to such a role.
///
/// The associations are read in the order the configuration states them — the default cache
/// behaviour, then each ordered one — so a configuration carrying several of them is refused
/// for the first one that cannot fire.
///
/// A configuration that breaks the ARN rule is left to that rule: it is checked first on
/// every path that stores a configuration, and an association that names no published
/// version has no execution role to resolve.
///
/// Asked BEFORE this service's store is locked, and it writes nothing: it is reads of other
/// services' records, bounded as the module docs state.
pub async fn edge_association_role_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    config: &DistributionConfig,
) -> Option<String> {
    if super::lambda_association_without_version(config).is_some() {
        return None;
    }
    // One Lambda client per Region, held across the associations that name it: the ARNs are
    // read in configuration order, and a configuration's associations are overwhelmingly of
    // one Region, so this builds one client — and at most one per entry of a bounded list in
    // any case.
    let mut lambda: Option<(&str, ::aws_sdk_lambda::Client)> = None;
    // The verdict on each role already judged, so associations sharing an execution role
    // share the IAM reads that judged it.
    let mut judged: Vec<(String, bool)> = Vec::new();
    for function_arn in edge_association_arns(config) {
        let Some(region) = region_of(function_arn) else {
            continue;
        };
        if lambda.as_ref().is_none_or(|(held, _)| *held != region) {
            lambda = Some((region, lambda_client(ctx, region)));
        }
        let Some((_, client)) = lambda.as_ref() else {
            continue;
        };
        let Some(role_arn) = execution_role_of(client, function_arn).await else {
            continue;
        };
        let withholds = match judged.iter().find(|(held, _)| *held == role_arn) {
            Some((_, withholds)) => *withholds,
            None => {
                let withholds = role_withholds_edge_invocation(ctx, region, &role_arn).await;
                judged.push((role_arn.clone(), withholds));
                withholds
            }
        };
        if withholds {
            return Some(edge_lambda_role_trust_message(&role_arn));
        }
    }
    None
}

/// The `LambdaFunctionARN`s of `config` an execution role is resolved for: the ones the
/// cache behaviours associate that name a published function version of THIS account, in the
/// order the configuration states them, each once, and at most
/// [`MAX_EDGE_ROLES_RESOLVED_PER_REQUEST`] of them.
///
/// This emulator holds one account's resources, and Lambda keys its functions by Region and
/// name, so an ARN naming another account's function names nothing this emulator can ask
/// about: it is skipped rather than resolved against a like-named function of this account.
///
/// The de-duplication and the ceiling are what keep the reads one request drives bounded
/// however many associations its body states (see the module docs).
fn edge_association_arns(config: &DistributionConfig) -> Vec<&str> {
    let mut arns: Vec<&str> = Vec::new();
    let stated = super::behavior_views(config)
        .into_iter()
        .filter_map(|behavior| behavior.lambda_function_associations)
        .flat_map(|associations| associations.items.as_deref().unwrap_or_default())
        .map(|association| association.lambda_function_arn.as_str())
        .filter(|arn| names_this_accounts_function_version(arn));
    for arn in stated {
        if arns.len() >= MAX_EDGE_ROLES_RESOLVED_PER_REQUEST {
            break;
        }
        if !arns.contains(&arn) {
            arns.push(arn);
        }
    }
    arns
}

/// Whether `arn` names a published version of a function of THIS account.
fn names_this_accounts_function_version(arn: &str) -> bool {
    super::names_lambda_function_version(arn)
        && ::service_common::arn::parse_of(arn, LAMBDA_ARN_NAMESPACE).is_some_and(|arn| {
            arn.account.is_empty() || arn.account == ::engine::emulator_account_id()
        })
}

/// The vendor field of a Lambda ARN, which is what says the association names a Lambda
/// resource at all.
const LAMBDA_ARN_NAMESPACE: &str = "lambda";

/// The execution role LAMBDA reports for the function version `function_arn` names, or
/// `None` when nothing was learned: Lambda holds no such function or version, it reports the
/// version without a role, or no Lambda answered the hop at all.
/// `client` is addressed to the Region the association's own ARN names, which is the Region
/// the function lives in; the version qualifier travels in the ARN, so the record read is
/// the one the association invokes.
async fn execution_role_of(
    client: &::aws_sdk_lambda::Client,
    function_arn: &str,
) -> Option<String> {
    match client
        .get_function_configuration()
        .function_name(function_arn)
        .send()
        .await
    {
        Ok(answered) => answered
            .role()
            .map(::std::string::ToString::to_string)
            .filter(|role| !role.is_empty()),
        Err(error) => {
            ::tracing::debug!(
                function = %function_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "no execution role was learned for a Lambda@Edge association"
            );
            None
        }
    }
}

/// Whether this account's IAM ANSWERED that `role_arn` is a role the EDGE could not invoke
/// the function under: IAM holds that role, and the trust policy it carries withholds one of
/// the two assumes a Lambda@Edge invocation makes of it.
///
/// Each assume is put to the one shared reading of a stored trust document, so a role held
/// by this emulator's IAM is judged here exactly as every other service judges it; `false`
/// for every answer that is not IAM reporting such a role, for the reason the module docs
/// give.
/// IAM is partition-global, so the `region` this hop is addressed to names no records of its
/// own; it is the Region of the resources being validated.
async fn role_withholds_edge_invocation(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    for principal in [EDGE_LAMBDA_SERVICE_PRINCIPAL, LAMBDA_SERVICE_PRINCIPAL] {
        if ::service_common::iam_role::held_role_is_unassumable(ctx, region, role_arn, principal)
            .await
        {
            return true;
        }
    }
    false
}

/// The Region a Lambda function ARN names, through the crate's one ARN parse.
fn region_of(function_arn: &str) -> Option<&str> {
    ::service_common::arn::parse_of(function_arn, LAMBDA_ARN_NAMESPACE)
        .map(|arn| arn.region)
        .filter(|region| !region.is_empty())
}

/// The Lambda client this module's reads are made with.
///
/// The transport is the emulator's own, in process, so the call is served by the Lambda this
/// emulator mounts and no connection is opened; the credentials are the ones the callback
/// hands over, which sign this call and are never reported or logged. There is nothing to
/// retry, time out or watch for a stalled stream on that transport, and each of those would
/// need a sleep implementation this client is not given.
fn lambda_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The message states the rule and then the role it resolved, and the role ARN is its
    /// ONLY variable part.
    #[test]
    fn the_refusal_names_the_two_principals_and_the_resolved_role() {
        // The rows differ along what the message generalises over: the role's path, its
        // name and the partition its ARN carries.
        for role_arn in [
            "arn:aws:iam::000000000000:role/edge-greeter",
            "arn:aws:iam::000000000000:role/service-role/edge-greeter",
            "arn:aws-us-gov:iam::000000000000:role/edge.greeter_2",
        ] {
            let message = edge_lambda_role_trust_message(role_arn);
            assert_eq!(
                message,
                format!(
                    "The function execution role must be assumable with \
                     edgelambda.amazonaws.com as well as lambda.amazonaws.com principals. \
                     Update the IAM role and try again. Role: {role_arn}"
                ),
                "case {role_arn:?}: the refusal states the rule and names the role, got \
                 {message:?}"
            );
        }
    }

    /// Only an association that names a published function version of THIS account has an
    /// execution role to resolve: everything else is another rule's, or another account's.
    #[test]
    fn only_this_accounts_published_versions_are_resolved() {
        // The rows differ along what the filter generalises over: the qualifier, the
        // account field, the vendor and the form of the value.
        let rows: [(&str, &str, bool); 7] = [
            (
                "a published version of this account",
                "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:1",
                true,
            ),
            (
                "a many-digit version of this account",
                "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:1042",
                true,
            ),
            (
                "the unqualified function ARN",
                "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter",
                false,
            ),
            (
                "the unpublished version",
                "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:$LATEST",
                false,
            ),
            (
                "an alias",
                "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:live",
                false,
            ),
            (
                "another account's published version",
                "arn:aws:lambda:us-east-1:210987654321:function:edge-greeter:1",
                false,
            ),
            (
                "another service's versioned ARN",
                "arn:aws:states:us-east-1:000000000000:stateMachine:edge-greeter:1",
                false,
            ),
        ];
        for (case, arn, expected) in rows {
            let resolved = names_this_accounts_function_version(arn);
            assert_eq!(
                resolved, expected,
                "case {case:?}: expected {expected}, got {resolved} for {arn}"
            );
        }
    }

    /// RULE: however many associations a submitted configuration states, the ARNs an
    /// execution role is resolved for are each read ONCE and never more than
    /// [`MAX_EDGE_ROLES_RESOLVED_PER_REQUEST`] of them — the bound on what one request can
    /// make this emulator read of Lambda's and IAM's records.
    #[test]
    fn one_request_resolves_each_arn_once_and_at_most_the_ceiling() {
        /// The version ARN of the function `index`.
        fn version(index: usize) -> String {
            format!(
                "arn:aws:lambda:us-east-1:{}:function:edge-{index}:1",
                ::engine::emulator_account_id()
            )
        }

        /// The configuration `arns` states on its default cache behaviour.
        fn config(arns: &[String]) -> DistributionConfig {
            DistributionConfig {
                caller_reference: "ref".to_string(),
                comment: "a configuration".to_string(),
                enabled: true,
                origins: crate::ops::types::Origins {
                    quantity: 1,
                    items: vec![crate::ops::types::Origin {
                        id: "primary".to_string(),
                        domain_name: "origin.example.com".to_string(),
                        ..crate::ops::types::Origin::default()
                    }],
                },
                default_cache_behavior: crate::ops::types::DefaultCacheBehavior {
                    target_origin_id: "primary".to_string(),
                    viewer_protocol_policy: "allow-all".to_string(),
                    lambda_function_associations: Some(
                        crate::ops::types::LambdaFunctionAssociations {
                            quantity: i32::try_from(arns.len()).unwrap_or(i32::MAX),
                            items: Some(
                                arns.iter()
                                    .map(|arn| crate::ops::types::LambdaFunctionAssociation {
                                        lambda_function_arn: arn.clone(),
                                        event_type: "viewer-request".to_string(),
                                        include_body: Some(false),
                                    })
                                    .collect(),
                            ),
                        },
                    ),
                    ..crate::ops::types::DefaultCacheBehavior::default()
                },
                ..DistributionConfig::default()
            }
        }

        // The rows differ along what the bound generalises over: how many associations the
        // configuration states, and how many of them name the same function.
        let few: Vec<String> = (0..3).map(version).collect();
        let repeated: Vec<String> = (0..MAX_EDGE_ROLES_RESOLVED_PER_REQUEST + 8)
            .map(|_| version(0))
            .collect();
        let distinct: Vec<String> = (0..MAX_EDGE_ROLES_RESOLVED_PER_REQUEST + 8)
            .map(version)
            .collect();
        let rows: [(&str, &[String], usize); 3] = [
            ("three distinct associations", &few, 3),
            ("more repeats of one association than the ceiling", &repeated, 1),
            (
                "more distinct associations than the ceiling",
                &distinct,
                MAX_EDGE_ROLES_RESOLVED_PER_REQUEST,
            ),
        ];
        for (case, arns, expected) in rows {
            let submitted = config(arns);
            let resolved = edge_association_arns(&submitted);
            assert_eq!(
                resolved.len(),
                expected,
                "case {case:?}: expected {expected} ARNs to be resolved, got {resolved:?}"
            );
            let mut seen: Vec<&str> = Vec::new();
            for arn in &resolved {
                assert!(
                    !seen.contains(arn),
                    "case {case:?}: {arn} is resolved twice in {resolved:?}"
                );
                seen.push(arn);
            }
        }
    }
}
