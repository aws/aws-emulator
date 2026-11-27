// SPDX-License-Identifier: CC0-1.0

//! What the RUN of an image build records about itself, beside the configuration its request
//! named.
//!
//! A build does not only hold the references a caller handed it: it resolves them, runs under
//! an identity, runs a set of workflows and ends up on a concrete operating system. Those are
//! facts of the RUN, so they are observed once, where the build is queued, and held on the
//! build version record ([`super::ImageBuildVersionRecord`]) for every later read to report —
//! rather than being derived afresh by each reader from resources that may since have moved
//! on. One reading serves both paths that queue a build (`CreateImage` and
//! `StartImagePipelineExecution`), through [`ImageBuildRun::observed`].
//!
//! The four facts, each from the modelled member that reports it:
//!
//! * the BASE IMAGE the build ran against. A recipe's `parentImage` is "the base image for
//!   customizations specified in the image recipe" and may name a whole version line rather
//!   than one image (`…:image/<name>/x.x.x`); the build resolves that reference when it
//!   starts, exactly as it resolves the recipe reference itself, so what it records — and what
//!   a read of the image reports in the recipe it reports — is the ONE image version the build
//!   actually built on ([`ImageBuildRun::resolved_parent_image`]);
//! * the OPERATING SYSTEM that image carries, which is `Image`'s `osVersion`: "the operating
//!   system version for instances that launch from this image". It is a property of the base
//!   image, so it is read off the base image the build resolved — the published catalog names
//!   the operating system of each image it holds ([`super::ManagedImage::os_version`]), and an
//!   image this account built carries the one its own build recorded, so a chain of builds
//!   keeps reporting it;
//! * the WORKFLOWS the build was configured with, which is `Image`'s `workflows`: "the build,
//!   test, and distribution workflow configurations that are associated with the image". A
//!   request may name them, and then "they replace the default workflows that Image Builder
//!   otherwise runs for the build" (`CreateImage`'s `workflows`); a request that names none was
//!   configured with those default workflows, which are AWS's own workflow resources
//!   ([`SERVICE_OWNED_WORKFLOWS`]) rather than resources this account created;
//! * the IDENTITY the build assumed, which is `Image`'s `executionRole`: "the name or Amazon
//!   Resource Name (ARN) for the IAM role you create that grants Image Builder access to
//!   perform workflow actions" — and "if you don't provide a role, Image Builder uses the Image
//!   Builder service-linked role in your account" (`CreateImage`'s `executionRole`), so a build
//!   that was given none ran under that role ([`service_linked_execution_role`]).
//!
//! The run also carries the ID of the execution itself, which the wire reports on a built image
//! and the on-disk model does not declare ([`ImageBuildRun::execution_id`]).

use std::time::SystemTime;

use crate::ops::types::WorkflowConfiguration;

use super::{
    managed_image, new_uuid, workflow_stage, BuiltRecipe, ComAmazonawsImagebuilderImagebuilderState,
    ARN_SERVICE, FIRST_BUILD_VERSION, IMAGE_TYPE_AMI, MANAGED_IMAGE_OWNER, WORKFLOW_ARN_PREFIX,
};

/// The name of the service-linked role Image Builder assumes for a build the caller gave no
/// role for.
///
/// `CreateImage`'s `executionRole`: "If you don't provide a role, Image Builder uses the Image
/// Builder service-linked role in your account, and creates it if it doesn't exist." The role
/// AWS publishes for this service is the one named here; the path is the one IAM reserves for
/// a service-linked role, which carries the service principal the role is linked to
/// ([`SERVICE_LINKED_ROLE_PATH`]).
const SERVICE_LINKED_ROLE_NAME: &str = "AWSServiceRoleForImageBuilder";

/// The IAM path a service-linked role of this service is filed under: the prefix IAM reserves
/// for such roles, followed by the service principal the role is linked to.
const SERVICE_LINKED_ROLE_PATH: &str = "role/aws-service-role/imagebuilder.amazonaws.com/";

/// The role an image build runs its workflow actions under when its request named none: the
/// ARN of this account's Image Builder service-linked role.
///
/// An IAM role is a GLOBAL resource, so its ARN carries no Region; the account is this
/// emulator's own, which is the account a build of it runs in.
#[must_use]
pub fn service_linked_execution_role() -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("{SERVICE_LINKED_ROLE_PATH}{SERVICE_LINKED_ROLE_NAME}"),
    )
}

/// ONE build of a workflow AWS itself publishes: the name AWS publishes it under, and the
/// semantic version of the build this emulator reports of it.
///
/// A workflow AWS owns is versioned exactly as one an account creates is, and AWS publishes a
/// new build of EACH of its workflows as that workflow's own document changes — so the lines
/// move independently and no one version describes the set. WHICH versions AWS has published
/// is AWS's to say and nothing here can derive one, so — exactly as the published image
/// catalog does for its own dated builds ([`super::MANAGED_IMAGE_VERSION`]) — this catalogue
/// records ONE build of each published workflow, each carrying its OWN version, and says so
/// rather than implying a catalog it does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedWorkflow {
    /// The name AWS publishes the workflow under.
    pub name: &'static str,
    /// The semantic version of the build of it this catalogue records.
    pub version: &'static str,
}

/// One workflow AWS itself owns: the stage it runs in, and the build AWS publishes of it for
/// each kind of output a build produces.
///
/// The names are AWS's own service-owned workflow resources, which the Image Builder user
/// guide lists per output kind: a build that produces an AMI runs the image workflows, and one
/// that produces a container image runs the container workflows. Each of those is a workflow
/// resource in its own right, so each carries its own published build
/// ([`PublishedWorkflow`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOwnedWorkflow {
    /// The value of the modelled `WorkflowType` enum this workflow belongs to, which its ARN
    /// carries as its stage ([`super::workflow_stage`]).
    pub workflow_type: &'static str,
    /// The build AWS publishes of it for a build whose output is an AMI.
    pub image: PublishedWorkflow,
    /// The build AWS publishes of it for a build whose output is a container image.
    pub container: PublishedWorkflow,
}

/// The workflows a build runs when its request named none, in the order a build runs them:
/// the build stage first, then the test stage.
///
/// The distribution stage is not among them: this emulator distributes through no workflow of
/// its own, so a build that ran none reports none rather than naming one nothing ran.
pub const SERVICE_OWNED_WORKFLOWS: &[ServiceOwnedWorkflow] = &[
    ServiceOwnedWorkflow {
        workflow_type: "BUILD",
        image: PublishedWorkflow {
            name: "build-image",
            version: "1.0.1",
        },
        container: PublishedWorkflow {
            name: "build-container",
            version: "1.0.1",
        },
    },
    ServiceOwnedWorkflow {
        // The value of the modelled `WorkflowType` enum the TEST stage carries
        // (`smithy_query(query="shape",shape="WorkflowType")`).
        workflow_type: "TEST",
        image: PublishedWorkflow {
            name: "test-image",
            version: "1.0.3",
        },
        container: PublishedWorkflow {
            name: "test-container",
            version: "1.0.1",
        },
    },
];

impl ServiceOwnedWorkflow {
    /// The build AWS publishes of this workflow for a build whose output is `image_type`.
    #[must_use]
    pub fn published(&self, image_type: &str) -> PublishedWorkflow {
        if image_type == IMAGE_TYPE_AMI {
            self.image
        } else {
            self.container
        }
    }

    /// The build version ARN of this workflow in `region`, as a configuration that ran it
    /// names it.
    ///
    /// The owner field is AWS's own rather than this account
    /// ([`MANAGED_IMAGE_OWNER`]) — the workflow is a resource AWS owns, not one this account
    /// created — the version is the one this catalogue records that workflow's own published
    /// build at, and it is fully assigned, down to the build node, since the modelled
    /// `WorkflowConfiguration.workflowArn` admits a wildcard version or a build version and
    /// what RAN was one build.
    #[must_use]
    pub fn build_version_arn(&self, partition: &str, region: &str, image_type: &str) -> String {
        let published = self.published(image_type);
        format!(
            "arn:{partition}:{ARN_SERVICE}:{region}:{MANAGED_IMAGE_OWNER}:{WORKFLOW_ARN_PREFIX}{}/\
             {}/{}/{FIRST_BUILD_VERSION}",
            workflow_stage(self.workflow_type),
            published.name,
            published.version,
        )
    }
}

/// The workflows a build of `image_type` in `region` ran when its request named none: one
/// configuration per service-owned workflow of that output kind, in stage order.
///
/// EVERY stage of the default set is associated with the image, whatever the build's test
/// settings say: `Image`'s `workflows` is "the build, test, and distribution workflow
/// configurations that are associated with the image", and `imageTestsConfiguration`'s
/// `imageTestsEnabled` "specifies whether tests run after building the image" — it governs
/// whether the test stage RUNS, not which workflows the build is configured with, so a build
/// that skips its tests is still the default workflow set.
///
/// Each configuration names the workflow and nothing else: the parameters, the parallel group
/// and the failure action are what a CALLER sets on a workflow it configures, and a default
/// run set none.
#[must_use]
pub fn default_build_workflows(region: &str, image_type: &str) -> Vec<WorkflowConfiguration> {
    let partition = super::listing_partition(region);
    SERVICE_OWNED_WORKFLOWS
        .iter()
        .map(|workflow| WorkflowConfiguration {
            workflow_arn: workflow.build_version_arn(&partition, region, image_type),
            parameters: None,
            parallel_group: None,
            on_failure: None,
        })
        .collect()
}

/// What one run of an image build observed about itself, recorded on the build version the run
/// filed.
#[derive(Debug, Clone)]
pub struct ImageBuildRun {
    /// The id of this build execution, which the wire reports on a built image.
    ///
    /// The VALUE is per-run and is not a requirement — nothing can predict it and no caller may
    /// depend on a particular one — so it is minted the way every other id of this service is
    /// ([`super::new_uuid`]), and what it must do is name ONE execution, which that mint
    /// guarantees.
    pub execution_id: String,
    /// The role the build assumed: the one its request named, or this account's service-linked
    /// role when it named none ([`service_linked_execution_role`]).
    pub execution_role: String,
    /// The workflows the build was configured with: the ones its request named, which "replace
    /// the default workflows that Image Builder otherwise runs for the build", or those default
    /// workflows ([`default_build_workflows`]).
    pub workflows: Vec<WorkflowConfiguration>,
    /// The ONE base image version the build resolved its recipe's `parentImage` to.
    pub resolved_parent_image: String,
    /// The operating system version that base image carries, when the base image is one this
    /// service knows the operating system of.
    pub os_version: Option<String>,
}

impl ImageBuildRun {
    /// What a build of `built` in `region`, queued at `at`, records about its own run.
    ///
    /// `requested_workflows` and `requested_role` are what the request (or the pipeline whose
    /// execution queued the build) named.
    #[must_use]
    pub fn observed(
        store: &ComAmazonawsImagebuilderImagebuilderState,
        built: &BuiltRecipe,
        region: &str,
        requested_workflows: Option<&[WorkflowConfiguration]>,
        requested_role: Option<&str>,
        at: SystemTime,
    ) -> Self {
        let base = store.resolved_parent_image(&built.parent_image, region, at);
        Self {
            execution_id: new_uuid(),
            execution_role: requested_role
                .map_or_else(service_linked_execution_role, str::to_string),
            workflows: requested_workflows.map_or_else(
                || default_build_workflows(region, built.image_type),
                <[WorkflowConfiguration]>::to_vec,
            ),
            resolved_parent_image: base.reference,
            os_version: base.os_version,
        }
    }
}

/// The ONE base image a `parentImage` reference reaches, as the build that resolved it recorded
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedParentImage {
    /// The reference the build recorded: the resolved image version when the value named an
    /// Image Builder image, and the value itself otherwise.
    pub reference: String,
    /// The operating system version that image carries, when this service knows it.
    pub os_version: Option<String>,
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// The base image the `parentImage` value `reference` reaches at the instant `at`, asked in
    /// `request_region`.
    ///
    /// Of the identifier forms `parentImage` admits
    /// ([`super::is_supported_parent_image`]) exactly one names an image resource of THIS
    /// service — an Image Builder image ARN — so exactly one is resolved here, by the two
    /// readings a reference to an image has
    /// ([`Self::image_answer`]): an image AWS publishes, which the catalog holds one dated
    /// build of, and an image this account built, which is a record of this store. Both resolve
    /// a wildcard version node to the ONE build the reference reaches, which is the build the
    /// run built on.
    ///
    /// Every other form — an AMI id, a Systems Manager parameter, a Marketplace product id —
    /// names a catalog no part of this emulator resolves an identifier against, so the value
    /// stands as the build recorded it and nothing is claimed about the operating system it
    /// carries.
    #[must_use]
    pub fn resolved_parent_image(
        &self,
        reference: &str,
        request_region: &str,
        at: SystemTime,
    ) -> ResolvedParentImage {
        if let Some((published, partition)) = managed_image(reference, request_region) {
            return ResolvedParentImage {
                reference: published.arn(partition, request_region),
                os_version: Some(published.os_version.to_string()),
            };
        }
        if let Some(built) = self.resolve_image(reference, request_region, at) {
            return ResolvedParentImage {
                reference: built.arn.clone(),
                os_version: built.os_version.clone(),
            };
        }
        ResolvedParentImage {
            reference: reference.to_string(),
            os_version: None,
        }
    }
}
