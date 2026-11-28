// SPDX-License-Identifier: CC0-1.0

//! WHAT AN `ImageId` OF THE `resolve:ssm:` FORM RESOLVES TO, asked of the service that OWNS
//! Systems Manager parameters.
//!
//! "Alternatively, you can specify a Systems Manager parameter, using one of the following
//! formats. The Systems Manager parameter WILL RESOLVE TO AN AMI ID ON LAUNCH"
//! (`RequestLaunchTemplateData$ImageId`): the value is a reference to a parameter, so the
//! parameter it names is the one Parameter Store holds — the account's own and the ones AWS
//! publishes alike — and not a second catalogue kept here. Which parameter the value names is
//! Parameter Store's own reading of "the name or Amazon Resource Name (ARN) of the parameter",
//! selector included, so every documented form — `<name>`, `<name>:<version>`,
//! `<name>:<label>` and the parameter ARN in each of those three shapes — resolves wherever
//! that service resolves it, and nothing here enumerates them.
//!
//! The ask is `ssm:GetParameter`, made over the in-process callback every emulated service
//! reaches another through ([`smithy_server_runtime::OperationContext::callback`]): this
//! crate names no other service's crate, no socket is opened and nothing outside this process
//! is reachable. The endpoint URL is built by the ONE construction of that client this
//! workspace has ([`::service_common::ssm_client`]), whose only variable part is the Region
//! the request was signed for; no caller-supplied value becomes a destination, and the
//! caller's `Authorization` header is never forwarded — the credentials are the emulator's
//! own.
//!
//! An emulator assembled WITHOUT a Systems Manager can resolve nothing through it, and the
//! transport's own refusal says nothing about the parameter. Such a call answers
//! [`AliasResolution::Unknown`], and the caller falls back to the published image catalog this
//! service itself projects into that store
//! ([`super::ComAmazonawsEc2Amazonec2State::public_ssm_parameter_values`]) — so the published
//! names resolve either way and no reference is refused on evidence this emulator does not
//! have.
//!
//! A READ throughout: resolving an alias reads a parameter and writes nothing, so a launch
//! that resolves one changes nothing a later caller of either service sees.

use ::aws_sdk_ssm::error::ProvideErrorMetadata;

/// The error codes the in-process TRANSPORT answers when this emulator holds no Systems
/// Manager at the endpoint that was called, or when more than one of its services claims that
/// request. Either way the call reached no Parameter Store, so it says nothing about the
/// parameter.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// What the service that owns parameters answered about one `resolve:ssm:` reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasResolution {
    /// The value the parameter the reference names holds.
    Value(String),
    /// Parameter Store produced no value for the reference: nothing of that name at this
    /// endpoint, or a selector the parameter does not carry.
    Unresolved,
    /// Nothing was said about the parameter at all, because no Systems Manager answered.
    Unknown,
}

/// The value the Systems Manager parameter `reference` names in `region`, asked of the
/// service that holds it.
///
/// `reference` is the text an `ImageId` carries after [`super::SSM_IMAGE_ALIAS_PREFIX`] — a
/// name or an ARN, either optionally carrying a `:<version>` or `:<label>` selector — handed
/// over as the `Name` of the read, so the form is read by the service that owns the parameter
/// rather than a second time here.
pub async fn resolved_image_alias(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    reference: &str,
) -> AliasResolution {
    let client = ::service_common::ssm_client::ssm_client(ctx.callback(), region);
    match client.get_parameter().name(reference).send().await {
        Ok(answer) => match answer
            .parameter()
            .and_then(|parameter| parameter.value())
            .filter(|value| !value.is_empty())
        {
            Some(value) => AliasResolution::Value(value.to_string()),
            // A parameter with no value at all resolves to no AMI, which is the same answer
            // as no parameter: there is nothing behind the reference either way.
            None => AliasResolution::Unresolved,
        },
        Err(error) => {
            let Some(code) = error.code() else {
                // No answer was reached at all, so nothing is known about the parameter.
                ::tracing::debug!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "Systems Manager did not answer what this image reference resolves to"
                );
                return AliasResolution::Unknown;
            };
            if TRANSPORT_REFUSALS.contains(&code) {
                ::tracing::debug!(
                    code,
                    "no Systems Manager answered this image reference, so the published \
                     image catalog resolves it"
                );
                return AliasResolution::Unknown;
            }
            AliasResolution::Unresolved
        }
    }
}
