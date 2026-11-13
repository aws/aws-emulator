// SPDX-License-Identifier: CC0-1.0

//! What the code-signing configuration a function is attached to does to a DEPLOYMENT.
//!
//! A code-signing configuration is a deployment CONTROL, and its policy says what happens
//! to a deployment whose signature checks fail:
//! `CodeSigningPolicies$UntrustedArtifactOnDeployment` is "Code signing configuration
//! policy for deployment validation failure. If you set the policy to `Enforce`, Lambda
//! blocks the deployment request if signature validation checks fail. If you set the policy
//! to `Warn`, Lambda allows the deployment and issues a new Amazon CloudWatch metric
//! (`SignatureValidationErrors`) and also stores the warning in the CloudTrail log."
//!
//! What the checks are AGAINST is the configuration's `AllowedPublishers`: "The Amazon
//! Resource Name (ARN) for each of the signing profiles. A signing profile defines a
//! trusted user who can sign a code package." So a deployment passes only when the artifact
//! it carries bears a signature attesting to one of those signing profile versions.
//!
//! Every operation that DEPLOYS to a function holding an association puts that deployment to
//! [`untrusted_deployment_refusal`] before anything is stored, and answers the modelled
//! `CodeVerificationFailedException` (400, "The code signature failed one or more of the
//! validation checks for signature mismatch or expiry, and the code signing policy is set to
//! ENFORCE. Lambda blocks the deployment.") with the message it hands back: `CreateFunction`
//! and `UpdateFunctionCode` for the function's own deployment package, and
//! `UpdateFunctionConfiguration` for the layer packages a configuration change mounts into
//! the function's execution environment ([`DeployedArtifact`]). Those three are exactly the
//! operations the model binds that error to. The policy is therefore read in ONE place, and
//! a configuration whose policy is `Warn` lets the same deployment land.
//!
//! WHAT AN ARTIFACT ATTESTS TO is not something a deployment request carries. A signature is
//! stamped on an artifact by the signing service that produced it — the service that mints
//! the signing job a signed function reports as `SigningJobArn`, beside the
//! `SigningProfileVersionArn` of the profile that signed it — and travels with that
//! artifact, not with the call that deploys it: no member of any of those operations names a
//! signing profile version. This emulator mounts no signing service, so no artifact handed
//! to this API bears a signature and none of them attests to a publisher, which is what a
//! function stored here reports by carrying neither of those two members. See
//! [`DeployedArtifact::signing_profile_version`].

use super::*;

/// The `CodeSigningPolicy` value that BLOCKS a deployment whose signature checks failed:
/// "If you set the policy to `Enforce`, Lambda blocks the deployment request if signature
/// validation checks fail."
///
/// The enum's other value, `Warn`, is the documented default and the one that lets such a
/// deployment land ([`DEFAULT_CODE_SIGNING_POLICY`]).
pub const CODE_SIGNING_POLICY_ENFORCE: &str = "Enforce";

/// The refusal Lambda words for a deployment its code-signing checks rejected under an
/// `Enforce` policy: two fixed sentences, and the function the deployment addressed as the
/// variable part of the third.
///
/// `resource` is the function the deployment is of, as the operation making it addresses
/// one: the ARN of the function an update changes, and the name a create was given for a
/// function that does not exist yet.
#[must_use]
pub fn code_verification_failed_message(resource: &str) -> String {
    format!(
        "Lambda cannot deploy the function. The function or layer might be signed using a \
         signature that the client is not configured to accept. Check the provided signature \
         for {resource}."
    )
}

/// WHAT a deployment puts on a function: the artifact that bears the signature Lambda's
/// code-signing checks are made against.
#[derive(Debug, Clone, Copy)]
pub enum DeployedArtifact<'a> {
    /// The function's own deployment package, as the create or the code update resolved it.
    Package(&'a CodeRecord),
    /// The layer versions a configuration change mounts: "A list of function layers to add
    /// to the function's execution environment" — each its own package, signed the same way
    /// and verified alongside the function's, which is what the refusal means by "The
    /// function or layer might be signed using a signature that the client is not
    /// configured to accept."
    Layers,
}

impl DeployedArtifact<'_> {
    /// The signing profile version this artifact's signature attests to, when it bears one.
    ///
    /// `None` for every artifact this emulator is handed, and that is a statement about
    /// where a signature comes from rather than about this artifact: the signing service
    /// that produces a signed artifact is what stamps it and what mints the signing job
    /// attributing it to a profile version, no member of a deployment request names either,
    /// and this emulator mounts no such service. A function stored here reports no
    /// `SigningJobArn` and no `SigningProfileVersionArn` for exactly the same reason, so the
    /// two surfaces agree: what it holds is attributed to no publisher.
    #[must_use]
    fn signing_profile_version(&self) -> Option<&str> {
        let _unattributed = self;
        None
    }

    /// Whether code signing VERIFIES this artifact at all.
    ///
    /// A container image is outside the control: the signature a check is made against is
    /// borne by a .zip package, while an image is a reference into a registry that pins the
    /// function to the manifest digest that registry minted for it
    /// ([`super::image_source`]), so there is no package signature to verify and nothing is
    /// refused for one.
    #[must_use]
    fn is_verified(&self) -> bool {
        match self {
            Self::Package(code) => !code.is_image(),
            Self::Layers => true,
        }
    }

    /// Whether `config` counts this artifact as coming from a trusted publisher: whether the
    /// signature it bears attests to one of the signing profile versions the configuration
    /// allows.
    #[must_use]
    fn signed_by_allowed_publisher(&self, config: &CodeSigningConfigRecord) -> bool {
        self.signing_profile_version().is_some_and(|profile| {
            config
                .signing_profile_version_arns
                .iter()
                .any(|allowed| allowed == profile)
        })
    }
}

/// The refusal the code-signing configuration `config` owes the deployment of `artifact` to
/// the function `resource` names, or `None` when the deployment may land.
///
/// The one reading of the policy, made by every operation that deploys to a function the
/// configuration is attached to:
///
/// * an artifact whose signature attests to one of the configuration's allowed publishers
///   passes every check there is to make and lands;
/// * an artifact that does not, under [`CODE_SIGNING_POLICY_ENFORCE`], is the deployment
///   Lambda BLOCKS — the caller gets [`code_verification_failed_message`] and the store is
///   left as it was;
/// * an artifact that does not, under any other policy — `Warn`, the documented default —
///   lands, because that policy "allows the deployment".
#[must_use]
pub fn untrusted_deployment_refusal(
    config: &CodeSigningConfigRecord,
    artifact: DeployedArtifact<'_>,
    resource: &str,
) -> Option<String> {
    if !artifact.is_verified() {
        return None;
    }
    if artifact.signed_by_allowed_publisher(config) {
        return None;
    }
    if config.untrusted_artifact_on_deployment != CODE_SIGNING_POLICY_ENFORCE {
        return None;
    }
    Some(code_verification_failed_message(resource))
}

impl ComAmazonawsLambdaAwsgirapiserviceState {
    /// The code-signing configuration the function `function_name` is attached to, when it
    /// holds an association and this store still holds the configuration it names.
    ///
    /// One function has ONE association ([`FunctionRecord::code_signing_config_arn`]),
    /// however it was set, so every deployment to that function is verified against the
    /// single configuration found here.
    #[must_use]
    pub fn attached_code_signing_config(
        &self,
        function_name: &str,
    ) -> Option<&CodeSigningConfigRecord> {
        let arn = self
            .functions
            .get(function_name)?
            .code_signing_config_arn
            .as_deref()?;
        self.code_signing_configs.get(arn)
    }
}
