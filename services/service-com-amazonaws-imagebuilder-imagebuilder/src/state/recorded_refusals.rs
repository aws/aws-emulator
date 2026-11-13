// SPDX-License-Identifier: CC0-1.0

//! The refusal classes this service answers that its own Smithy model does not declare.
//!
//! Every other refusal of this service is one of its modelled error shapes, and those are
//! the ones a handler should reach for. Two classes are not in the model at all, and a
//! client branches on the wire code, so they are spelled here once — the code, the HTTP
//! status and the sentence together — and answered through
//! [`crate::scaffolded::Scaffolded::unmodeled`], which puts exactly that code and status
//! on the wire:
//!
//! * [`TAGGING_NOT_FOUND_CODE`] — the resource-tagging layer's own not-found class, which
//!   is what a TAG LISTING answers for an ARN that resolves to no resource, rather than the
//!   `ResourceNotFoundException` this service's own resource reads answer. Its sentence
//!   ([`TAGGING_NOT_FOUND_MESSAGE`]) is fixed text: it names neither the ARN nor the kind
//!   of resource, which is what tells the two classes apart in a body as well as in a code.
//! * [`MISSING_PARAMETERS_CODE`] — how a create reports that the request named NONE of a
//!   group of parameters it had to name one of. The model can require a member, so it can
//!   say nothing about a GROUP: `imageRecipeArn` and `containerRecipeArn` are each optional
//!   on their own and a request has to carry one of them. Its sentence names the members
//!   the request had to choose from, and nothing else of the request.

/// The wire error code the resource-tagging layer's not-found refusal is identified by
/// (`__type`, and the `x-amzn-errortype` header `restJson1` resolves an error shape from).
///
/// A client branches on this string, so it is the tagging layer's own class name and not
/// this service's modelled `ResourceNotFoundException`.
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The HTTP status that refusal carries: a resource the request named that does not exist,
/// reported as the not-found class every such answer of this tree carries.
pub const TAGGING_NOT_FOUND_STATUS: u16 = 404;

/// The sentence that refusal carries: fixed text, naming nothing of the request.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error code a create's missing-parameter-group refusal is identified by.
pub const MISSING_PARAMETERS_CODE: &str = "MissingParametersException";

/// The HTTP status that refusal carries: the request is wrong, so it is a client error.
pub const MISSING_PARAMETERS_STATUS: u16 = 400;

/// The two members a create of an image or a pipeline has to name exactly one of, in the
/// order its input shape declares them (`smithy_query(query="operation",
/// operation="CreateImagePipeline")`): the group a request naming neither is reported
/// missing.
pub const PIPELINE_RECIPE_MEMBERS: [&str; 2] = ["imageRecipeArn", "containerRecipeArn"];

/// Why a request that named none of a group of parameters it had to name one of is refused.
///
/// The sentence quotes the members the request could have named, in the order the input
/// shape declares them, and is otherwise fixed text: nothing of the request's own values
/// appears in it.
#[must_use]
pub fn missing_parameters_message(members: &[&str]) -> String {
    let quoted: Vec<String> = members.iter().map(|member| format!("'{member}'")).collect();
    format!(
        "Required parameters are missing from the request: {}. Verify that you have entered \
         all of the required parameters in your request and try again.",
        quoted.join(" or ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the sentence quotes every member of the group, joined as alternatives, and
    /// carries nothing of the request.
    #[test]
    fn the_missing_parameter_sentence_quotes_the_group_it_names() {
        // (case, the members, the sentence)
        let rows: &[(&str, &[&str], &str)] = &[
            (
                "the two recipe members of a pipeline create",
                &["imageRecipeArn", "containerRecipeArn"],
                "Required parameters are missing from the request: 'imageRecipeArn' or \
                 'containerRecipeArn'. Verify that you have entered all of the required \
                 parameters in your request and try again.",
            ),
            (
                "a group of one",
                &["imageRecipeArn"],
                "Required parameters are missing from the request: 'imageRecipeArn'. Verify \
                 that you have entered all of the required parameters in your request and try \
                 again.",
            ),
            (
                "a group of three",
                &["data", "uri", "kmsKeyId"],
                "Required parameters are missing from the request: 'data' or 'uri' or \
                 'kmsKeyId'. Verify that you have entered all of the required parameters in \
                 your request and try again.",
            ),
        ];
        for (case, members, expected) in rows {
            let actual = missing_parameters_message(members);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
