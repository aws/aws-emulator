// SPDX-License-Identifier: CC0-1.0

//! @op ListAccessPolicies
//! IMPLEMENTED. Reports the access-policy catalogue Amazon EKS publishes —
//! [`crate::state::ACCESS_POLICY_NAMES`], the single place this service holds it —
//! each entry with its name and its service-owned ARN
//! ([`crate::state::access_policy_arn`]), paged with an opaque `nextToken` derived
//! from the last name returned.
//!
//! An access policy is not a per-account resource: no API creates, updates or
//! deletes one, so this listing reads no per-account state and is the same in
//! every account and Region. It writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListAccessPolicies`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists the available access policies.
//!
//! ## Input
//!
//! * `maxResults` (ListAccessPoliciesRequestMaxResults, range 1..100) — The maximum number of
//!   results, returned in paginated output. You receive maxResults in a single page, along
//!   with a nextToken response element. You can see the remaining results of the initial
//!   request by sending another request with the returned nextToken value. This value can be
//!   between 1 and 100. If you don't use this parameter, 100 results and a nextToken value,
//!   if applicable, are returned.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//!
//! ## Output
//!
//! * `accessPolicies` (AccessPoliciesList) — The list of available access policies. You can't
//!   view the contents of an access policy using the API. To view the contents, see Access
//!   policy permissions in the Amazon EKS User Guide.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//!
//! ## Errors
//!
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, ACCESS_POLICIES_LISTING, ACCESS_POLICY_NAMES, DEFAULT_LIST_PAGE_SIZE};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListAccessPoliciesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAccessPoliciesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `InvalidParameterException` is widened in through `extra_errors`: this operation's
// own error list carries only `ServerException`, but a `nextToken` that is not one
// this listing minted IS a bad parameter, and the SERVICE models that shape ("The
// specified parameter is invalid. Review the available parameters for the API
// request.") — so the whole service answers a foreign page token with one code
// (`ListClusters` already does) instead of a synthetic one only this operation uses.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ListAccessPolicies",
    extra_errors = ["InvalidParameterException"]
)]
#[derive(Debug)]
pub enum ListAccessPoliciesErrorModeled {
    InvalidParameterException(InvalidParameterException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_access_policies::ListAccessPoliciesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAccessPoliciesError = crate::scaffolded::Scaffolded<ListAccessPoliciesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAccessPoliciesErrorModeled> for ListAccessPoliciesError {
    fn from(modeled: ListAccessPoliciesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAccessPolicies` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAccessPolicies<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAccessPolicies<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAccessPolicies<Eng>
{
    type Input = ListAccessPoliciesInput;
    type Output = ListAccessPoliciesOutput;
    type Error = ListAccessPoliciesError;

    const NAME: &'static str = "ListAccessPolicies";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ACCESS_POLICIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ACCESS_POLICIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Neither the engine nor the request's metadata takes part in the answer: the
        // catalogue this reports is published by the SERVICE, so it is the same in
        // every account and Region and no API can change it. Nothing here is written,
        // and `ctx`'s credential scope would only name a Region the answer does not
        // depend on.
        let _ = (ctx, &self.engine);

        // A `nextToken` names the policy to resume AFTER. One this listing did not
        // issue is a bad parameter, not an empty page — refused in the wording the
        // ACCESS-ENTRY plane this listing belongs to states such a member in
        // ([`state::ACCESS_ENTRY_INVALID_NEXT_TOKEN_MESSAGE`]).
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(ACCESS_POLICIES_LISTING, token).ok_or_else(
                    || {
                        ListAccessPoliciesError::from(
                            ListAccessPoliciesErrorModeled::InvalidParameterException(
                                InvalidParameterException {
                                    message: Some(
                                        state::ACCESS_ENTRY_INVALID_NEXT_TOKEN_MESSAGE
                                            .to_string(),
                                    ),
                                    ..Default::default()
                                },
                            ),
                        )
                    },
                )?,
            ),
        };

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED by
        // schema validation before this handler runs; only the absent case gets a
        // value, and "If you don't use this parameter, 100 results and a nextToken
        // value, if applicable, are returned" (`maxResults`) — a whole page.
        let page_size = usize::try_from(input.max_results.unwrap_or(DEFAULT_LIST_PAGE_SIZE))
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE as usize);

        let mut remaining = ACCESS_POLICY_NAMES
            .iter()
            .copied()
            .filter(|name| resume_after.as_deref().is_none_or(|after| *name > after));
        let page: Vec<&str> = remaining.by_ref().take(page_size).collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = match (remaining.next(), page.last()) {
            (Some(_), Some(last)) => {
                Some(state::encode_scoped_page_token(ACCESS_POLICIES_LISTING, last))
            }
            _ => None,
        };

        Ok(ListAccessPoliciesOutput {
            access_policies: Some(
                page.into_iter()
                    .map(|name| AccessPolicy {
                        name: Some(name.to_string()),
                        arn: Some(state::access_policy_arn(name)),
                    })
                    .collect(),
            ),
            next_token,
        })
    }
}
