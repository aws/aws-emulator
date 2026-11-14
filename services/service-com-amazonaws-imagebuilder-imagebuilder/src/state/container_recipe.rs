// SPDX-License-Identifier: CC0-1.0

//! The container recipes this service holds: what a create wrote, which is what every later
//! read reports.
//!
//! A container recipe "defines how images are configured, tested, and assessed" for a
//! container image, so it is a recipe like an image recipe ([`super::recipes`]) and differs
//! in what it adds: the Dockerfile template the build applies, the instance the build and
//! test run on, and the repository the output image is pushed to. The rules a container
//! recipe follows on its own live here:
//!
//! * the recipe carries its Dockerfile template as TEXT, whichever member named it — "You
//!   must specify exactly one of the dockerfileTemplateData or dockerfileTemplateUri
//!   properties", and of the second: "Image Builder reads the object once, when it creates
//!   the recipe, and stores its content in the recipe. Later changes to the S3 object don't
//!   affect the recipe";
//! * it builds for one of the two platforms the modelled `platform` member admits for a
//!   container ([`CONTAINER_RECIPE_PLATFORMS`]);
//! * its ARN carries a normalized form of its name plus its version, like every other
//!   versioned resource of this service, so one key says which name+version pair is taken
//!   where.

use std::collections::BTreeMap;

use crate::ops::types::{
    ComponentConfiguration, ContainerRecipe, CreateContainerRecipeInput, InstanceConfiguration,
    TargetContainerRepository,
};

use super::{
    block_device_mappings_fingerprint, component_configurations_fingerprint, group,
    normalized_name, parent_image_platform, parse_semantic_version, reported_tags, tag_map, text,
    version_admits, ComAmazonawsImagebuilderImagebuilderState, VersionedReference, ABSENT,
    ARN_SERVICE, MAX_IMAGE_RECIPES, PLATFORM_LINUX, PLATFORM_WINDOWS, URI_NOT_FOUND_REQUIREMENT,
};

/// The resource portion a container-recipe ARN carries before its name, from the modelled
/// `ContainerRecipeArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):container-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\.(?:[0-9]+|x)\.(?:[0-9]+|x)$`.
pub const CONTAINER_RECIPE_ARN_PREFIX: &str = "container-recipe/";

/// The key this operation's `clientToken` receipts are filed under. See
/// [`super::CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`].
pub const CREATE_CONTAINER_RECIPE_OPERATION: &str = "CreateContainerRecipe";

/// The most container recipes this emulator will hold. An emulator resource guard, for the
/// same reason as [`MAX_IMAGE_RECIPES`]: the model carries no value for Image Builder's
/// published per-account limit, so the cap sits well above any realistic use and exists only
/// so a caller-driven store cannot grow without bound.
pub const MAX_CONTAINER_RECIPES: usize = MAX_IMAGE_RECIPES;

/// The platforms a container recipe builds for, in the order the modelled `Platform` enum
/// declares them.
///
/// The restriction is the `platform` member's own: "The system platform for the container.
/// Container recipes support only the Linux and Windows platforms", which `platformOverride`
/// repeats — a narrowing of the `Platform` enum (`Windows | Linux | macOS`) that no modelled
/// constraint can state, so a request naming the remaining value names a platform this
/// resource cannot have.
pub const CONTAINER_RECIPE_PLATFORMS: &[&str] = &[PLATFORM_WINDOWS, PLATFORM_LINUX];

/// The member an inline Dockerfile template arrives in, as the API addresses it.
pub const DOCKERFILE_TEMPLATE_DATA_MEMBER: &str = "dockerfileTemplateData";

/// The member the LOCATION of a Dockerfile template arrives in, as the API addresses it.
pub const DOCKERFILE_TEMPLATE_URI_MEMBER: &str = "dockerfileTemplateUri";

/// The member an overriding platform arrives in, as the API addresses it.
pub const PLATFORM_OVERRIDE_MEMBER: &str = "platformOverride";

/// Whether a stored recipe's Dockerfile template data is encrypted at rest, from the modelled
/// `encrypted` member: "Image Builder encrypts all Dockerfile template data at rest, so this
/// value is always true. This field is retained for backward compatibility".
pub const DOCKERFILE_TEMPLATE_DATA_ENCRYPTED: bool = true;

/// The most of the object a `dockerfileTemplateUri` names this service reads.
///
/// The bound is the modelled maximum of the SAME template text named inline
/// (`InlineDockerFileTemplate`, len 1..16000): a template is one value of one recipe whichever
/// member carried it, so what a create will take from an object is what it would have taken
/// in the request body.
pub const MAX_DOCKERFILE_TEMPLATE_BYTES: usize = 16_000;

/// One stored container recipe: what a create wrote, which is what every later read reports.
#[derive(Debug, Clone)]
pub struct ContainerRecipeRecord {
    /// This recipe's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    /// The kind of container the recipe builds, from the modelled `ContainerType` enum.
    pub container_type: String,
    /// The ONE platform the recipe builds for ([`container_recipe_platform`]).
    pub platform: String,
    pub owner: String,
    /// The component configurations, in the order the create submitted them — empty for a
    /// recipe created without any.
    pub components: Vec<ComponentConfiguration>,
    /// The build and test instance options the create named, when it named any.
    pub instance_configuration: Option<InstanceConfiguration>,
    /// The Dockerfile template this recipe carries, whichever member named it: the inline
    /// text, or the snapshot the create read from the object store.
    pub dockerfile_template_data: String,
    pub kms_key_id: Option<String>,
    pub parent_image: String,
    pub working_directory: Option<String>,
    /// Where the output container image is pushed to.
    pub target_repository: TargetContainerRepository,
    pub tags: Option<BTreeMap<String, String>>,
    pub date_created: String,
}

impl ContainerRecipeRecord {
    /// This record as the modelled `ContainerRecipe` a read reports.
    #[must_use]
    pub fn view(&self) -> ContainerRecipe {
        ContainerRecipe {
            arn: Some(self.arn.clone()),
            container_type: Some(self.container_type.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            platform: Some(self.platform.clone()),
            owner: Some(self.owner.clone()),
            version: Some(self.version.clone()),
            components: Some(self.components.clone()),
            instance_configuration: self.instance_configuration.clone(),
            dockerfile_template_data: Some(self.dockerfile_template_data.clone()),
            kms_key_id: self.kms_key_id.clone(),
            // "Image Builder encrypts all Dockerfile template data at rest, so this value is
            // always true."
            encrypted: Some(DOCKERFILE_TEMPLATE_DATA_ENCRYPTED),
            parent_image: Some(self.parent_image.clone()),
            date_created: Some(self.date_created.clone()),
            tags: Some(reported_tags(self.tags.as_ref())),
            working_directory: self.working_directory.clone(),
            target_repository: Some(self.target_repository.clone()),
        }
    }
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// The container recipe `arn` names, but only when it belongs to `request_region`.
    ///
    /// A regional endpoint serves only its own Region's resources, so a recipe of another
    /// Region answers as if it did not exist.
    #[must_use]
    pub fn container_recipe(
        &self,
        arn: &str,
        request_region: &str,
    ) -> Option<&ContainerRecipeRecord> {
        self.container_recipes
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// The container recipe the ARN `reference` names, when one is stored in
    /// `request_region`.
    ///
    /// `reference` may name an exact version, or carry trailing `x` wildcards — "Version ARNs
    /// resolve to the latest available matching" record — so the greatest matching version is
    /// the one answered, exactly as an image recipe's reference resolves
    /// ([`Self::resolve_image_recipe`]).
    #[must_use]
    pub fn resolve_container_recipe(
        &self,
        reference: &str,
        request_region: &str,
    ) -> Option<&ContainerRecipeRecord> {
        let reference = VersionedReference::parse(reference, CONTAINER_RECIPE_ARN_PREFIX)?;
        if reference.region != request_region || reference.build_version.is_some() {
            return None;
        }
        self.container_recipes
            .values()
            .filter(|record| {
                // A regional endpoint serves only its own Region's resources, so a recipe of
                // another Region is reached by no reference of this one — whatever name and
                // version it carries.
                record.region == request_region
                    && reference.name == normalized_name(&record.name)
                    && version_admits(&reference.version, &record.version)
            })
            .max_by_key(|record| parse_semantic_version(&record.version).unwrap_or_default())
    }

    /// Store a newly created container recipe together with the receipt for the request that
    /// created it.
    pub fn insert_container_recipe(
        &mut self,
        record: ContainerRecipeRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_CONTAINER_RECIPE_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.container_recipes.insert(record.arn.clone(), record);
    }

    /// Whether the store is already holding as many container recipes as this emulator will
    /// keep ([`MAX_CONTAINER_RECIPES`]).
    #[must_use]
    pub fn container_recipe_store_is_full(&self) -> bool {
        self.container_recipes.len() >= MAX_CONTAINER_RECIPES
    }

    /// The first stored container recipe whose component list reaches the component `arn`
    /// names, if any does — the container-recipe half of
    /// [`Self::recipe_depending_on_component`], which reads the same rule over image recipes.
    #[must_use]
    pub fn container_recipe_depending_on_component(
        &self,
        arn: &str,
    ) -> Option<&ContainerRecipeRecord> {
        let component = self.components.get(arn)?;
        self.container_recipes.values().find(|recipe| {
            recipe
                .components
                .iter()
                .filter_map(|configuration| {
                    super::ComponentReference::parse(&configuration.component_arn)
                })
                .any(|reference| reference.matches(component))
        })
    }
}

/// The ARN of the container recipe `name` names at `version` in `region`.
#[must_use]
pub fn container_recipe_arn(region: &str, name: &str, version: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{CONTAINER_RECIPE_ARN_PREFIX}{}/{version}",
            normalized_name(name)
        ),
    )
}

/// The ONE platform the container recipe a create describes builds for.
///
/// Where it comes from, in the order the request settles it:
///
/// * `platformOverride`, when the request names one: "Specifies the operating system platform
///   when you use a custom base image";
/// * otherwise the components the recipe includes, each carrying the platform its own create
///   named, since a recipe's components all target one platform
///   ([`ComAmazonawsImagebuilderImagebuilderState::resolve_recipe_components`]) — and a create
///   refuses a component set targeting a platform outside
///   [`CONTAINER_RECIPE_PLATFORMS`], so what reaches here is one of the two;
/// * otherwise the base image the recipe builds from, read by the ONE reading of a parent
///   image this service has ([`super::parent_image_platform`]) and NARROWED to the two
///   platforms a container recipe supports ([`CONTAINER_RECIPE_PLATFORMS`]) — so a base image
///   that names neither builds for the remaining one.
#[must_use]
pub fn container_recipe_platform(
    platform_override: Option<&str>,
    components_target: Option<&str>,
    parent_image: &str,
) -> String {
    if let Some(named) = platform_override.or(components_target) {
        return named.to_string();
    }
    match parent_image_platform(parent_image).as_str() {
        PLATFORM_WINDOWS => PLATFORM_WINDOWS.to_string(),
        _ => PLATFORM_LINUX.to_string(),
    }
}

/// A deterministic rendering of the creation parameters one `CreateContainerRecipe` request
/// carries. See [`super::infrastructure_configuration_request_fingerprint`] for what such a
/// rendering leaves out and why.
#[must_use]
pub fn container_recipe_request_fingerprint(input: &CreateContainerRecipeInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert(
        "semanticVersion",
        text(Some(input.semantic_version.as_str())),
    );
    fields.insert("containerType", text(Some(input.container_type.as_str())));
    fields.insert("parentImage", text(Some(input.parent_image.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert("workingDirectory", text(input.working_directory.as_deref()));
    fields.insert(
        DOCKERFILE_TEMPLATE_DATA_MEMBER,
        text(input.dockerfile_template_data.as_deref()),
    );
    fields.insert(
        DOCKERFILE_TEMPLATE_URI_MEMBER,
        text(input.dockerfile_template_uri.as_deref()),
    );
    fields.insert(
        PLATFORM_OVERRIDE_MEMBER,
        text(input.platform_override.as_deref()),
    );
    fields.insert(
        "imageOsVersionOverride",
        text(input.image_os_version_override.as_deref()),
    );
    fields.insert("kmsKeyId", text(input.kms_key_id.as_deref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert(
        "components",
        component_configurations_fingerprint(input.components.as_deref()),
    );
    fields.insert(
        "instanceConfiguration",
        instance_configuration_fingerprint(input.instance_configuration.as_ref()),
    );
    fields.insert(
        "targetRepository",
        group(&[
            text(Some(input.target_repository.service.as_str())),
            text(Some(input.target_repository.repository_name.as_str())),
        ]),
    );
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// The build and test instance options one request named, as a fingerprint renders them.
fn instance_configuration_fingerprint(configuration: Option<&InstanceConfiguration>) -> String {
    configuration.map_or_else(
        || ABSENT.to_string(),
        |configuration| {
            group(&[
                text(configuration.image.as_deref()),
                block_device_mappings_fingerprint(configuration.block_device_mappings.as_deref()),
            ])
        },
    )
}

/// Why a create naming neither Dockerfile template, or both, is refused.
///
/// The rule is the operation's own, which each of the two members repeats: "You must specify
/// exactly one of the dockerfileTemplateData or dockerfileTemplateUri properties." A
/// requirement over the two members TOGETHER rather than a constraint on either value, so it
/// is stated the way this service states every refused parameter, naming both members:
/// neither one's value is wrong on its own.
#[must_use]
pub fn dockerfile_template_data_or_uri_message() -> String {
    ::service_common::validation_message::parameters_value_mismatch(
        &[
            DOCKERFILE_TEMPLATE_DATA_MEMBER,
            DOCKERFILE_TEMPLATE_URI_MEMBER,
        ],
        "You must specify exactly one of the dockerfileTemplateData or dockerfileTemplateUri \
         properties.",
    )
}

/// Why a create naming a template URI that is not an S3 URL is refused.
///
/// `dockerfileTemplateUri` is "The Amazon S3 URI for the Dockerfile template", so a value
/// that is not one addresses no object at all — a value of that member this service cannot
/// accept.
#[must_use]
pub fn dockerfile_template_uri_message() -> String {
    requirement_of_the_template_uri("dockerfileTemplateUri must be an S3 URL (s3://bucket/key)")
}

/// Why a create naming a template URI the object store serves no object at is refused.
///
/// Image Builder "reads the object once, when it creates the recipe", so a URI naming no
/// object — or one the read is not permitted for — is a value of that member this service
/// cannot accept, stated with the clause this class carries.
#[must_use]
pub fn dockerfile_template_uri_not_found_message() -> String {
    requirement_of_the_template_uri(URI_NOT_FOUND_REQUIREMENT)
}

/// Why a create naming a template object this service cannot take a template from is refused:
/// one larger than the template a request may carry, or bytes that are not text.
#[must_use]
pub fn dockerfile_template_object_message() -> String {
    requirement_of_the_template_uri(&format!(
        "The dockerfile template must be at most {MAX_DOCKERFILE_TEMPLATE_BYTES} bytes of text."
    ))
}

/// Why a create naming a platform a container recipe cannot build for is refused.
///
/// The set is the one the `platform` member documents for a container
/// ([`CONTAINER_RECIPE_PLATFORMS`]), stated as this service states every value outside an
/// enumerated set.
#[must_use]
pub fn unsupported_container_platform_message() -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        PLATFORM_OVERRIDE_MEMBER,
        &::service_common::validation_message::must_be_one_of(
            PLATFORM_OVERRIDE_MEMBER,
            &CONTAINER_RECIPE_PLATFORMS
                .iter()
                .map(|platform| (*platform).to_string())
                .collect::<Vec<_>>(),
        ),
    )
}

/// Why a create is refused once the container-recipe store is full.
#[must_use]
pub fn container_recipe_store_full_message() -> String {
    format!("This account already holds the maximum of {MAX_CONTAINER_RECIPES} container recipes.")
}

/// Why a create whose components target a platform a container recipe cannot build for is
/// refused.
///
/// The rule is the `platform` member's own — "Container recipes support only the Linux and
/// Windows platforms" — read over the components, since a recipe's platform is the platform
/// they target. A requirement over the components LIST rather than one value, so it is stated
/// like the list's other requirements, naming the platform the request's components target.
#[must_use]
pub fn unsupported_container_component_platform_message(named: &str) -> String {
    format!(
        "The components of a container recipe must target one of {}; this request names \
         {named}.",
        CONTAINER_RECIPE_PLATFORMS.join(", ")
    )
}

/// This service's parameter-value sentence about the member a template location arrives in,
/// over the requirement `why` its value broke.
fn requirement_of_the_template_uri(why: &str) -> String {
    ::service_common::validation_message::parameter_value_mismatch(
        DOCKERFILE_TEMPLATE_URI_MEMBER,
        why,
    )
}

/// One row of the platform table: what a failing assertion calls it, the `platformOverride`
/// the request named, the platform its components target, the base image, and the platform
/// the recipe builds for.
#[cfg(test)]
type PlatformRow = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
    &'static str,
);

#[cfg(test)]
mod tests {
    use super::*;

    /// A create body carrying only what the model requires, plus whatever `mutate` changes.
    fn request(mutate: impl FnOnce(&mut CreateContainerRecipeInput)) -> CreateContainerRecipeInput {
        let mut input = CreateContainerRecipeInput {
            container_type: "DOCKER".to_string(),
            name: "Hello World".to_string(),
            semantic_version: "1.0.0".to_string(),
            parent_image: "amazonlinux:latest".to_string(),
            dockerfile_template_data: Some("FROM {{{ imagebuilder:parentImage }}}".to_string()),
            target_repository: TargetContainerRepository {
                service: "ECR".to_string(),
                repository_name: "team-a/my-repo".to_string(),
            },
            client_token: "token".to_string(),
            ..CreateContainerRecipeInput::default()
        };
        mutate(&mut input);
        input
    }

    /// RULE: the ARN of a container recipe carries a NORMALIZED form of its name plus its
    /// version, so two names differing only in case, spaces or underscores are one name.
    #[test]
    fn the_arn_carries_a_normalized_name_and_the_version() {
        // (case, the submitted name, the version, the resource portion the ARN carries)
        let rows: &[(&str, &str, &str, &str)] = &[
            ("a plain name", "hello", "1.0.0", "container-recipe/hello/1.0.0"),
            (
                "spaces and case",
                "Hello World",
                "1.0.0",
                "container-recipe/hello-world/1.0.0",
            ),
            (
                "underscores",
                "hello_world",
                "2.11.0",
                "container-recipe/hello-world/2.11.0",
            ),
            (
                "the documented upper bound of a node",
                "hello",
                "1073741823.0.0",
                "container-recipe/hello/1073741823.0.0",
            ),
        ];
        for (case, name, version, resource) in rows {
            let actual = container_recipe_arn("us-west-2", name, version);
            let expected = format!("arn:aws:imagebuilder:us-west-2:000000000000:{resource}");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the platform is the override when the request names one, else the platform its
    /// components target, else the one the base image names — and only ever one of the two
    /// platforms a container recipe supports.
    #[test]
    fn the_platform_follows_the_override_then_the_components_then_the_base_image() {
        // (case, platformOverride, the components' platform, parentImage, the platform)
        let rows: &[PlatformRow] = &[
            (
                "an override wins",
                Some("Windows"),
                Some("Linux"),
                "amazonlinux:latest",
                "Windows",
            ),
            (
                "the components, with no override",
                None,
                Some("Windows"),
                "amazonlinux:latest",
                "Windows",
            ),
            (
                "a base image naming no operating system",
                None,
                None,
                "amazonlinux:latest",
                "Linux",
            ),
            (
                "a base image naming Windows",
                None,
                None,
                "mcr.microsoft.com/windows/servercore:ltsc2019",
                "Windows",
            ),
            (
                "a base image an Image Builder ARN names",
                None,
                None,
                "arn:aws:imagebuilder:us-west-2:aws:image/amazon-linux-2-x86/x.x.x",
                "Linux",
            ),
        ];
        for (case, platform_override, components_target, parent_image, expected) in rows {
            let actual =
                container_recipe_platform(*platform_override, *components_target, parent_image);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: two requests render alike only when every creation parameter is the same, which
    /// is what tells a retry of one request from a different request carrying one token.
    #[test]
    fn the_fingerprint_separates_two_different_creates() {
        let original = container_recipe_request_fingerprint(&request(|_| {}));
        // (case, what the second request changed)
        let rows: Vec<(&str, CreateContainerRecipeInput)> = vec![
            (
                "the same parameters, a different token",
                request(|input| input.client_token = "another-token".to_string()),
            ),
            (
                "another template",
                request(|input| {
                    input.dockerfile_template_data = Some("FROM scratch".to_string());
                }),
            ),
            (
                "the template named by location instead",
                request(|input| {
                    input.dockerfile_template_data = None;
                    input.dockerfile_template_uri = Some("s3://templates/Dockerfile".to_string());
                }),
            ),
            (
                "another repository",
                request(|input| {
                    input.target_repository.repository_name = "team-b/my-repo".to_string();
                }),
            ),
            (
                "a build instance image",
                request(|input| {
                    input.instance_configuration = Some(InstanceConfiguration {
                        image: Some("ami-0123456789abcdef0".to_string()),
                        block_device_mappings: None,
                    });
                }),
            ),
            (
                "one component",
                request(|input| {
                    input.components = Some(vec![ComponentConfiguration {
                        component_arn: "arn:aws:imagebuilder:us-west-2:000000000000:component/prep/1.0.0/1"
                            .to_string(),
                        parameters: None,
                    }]);
                }),
            ),
        ];
        for (case, changed) in rows {
            let rendered = container_recipe_request_fingerprint(&changed);
            let same_request = case == "the same parameters, a different token";
            assert_eq!(
                rendered == original,
                same_request,
                "case {case:?}: expected the renderings to {} match, got {rendered:?} against \
                 {original:?}",
                if same_request { "" } else { "not" }
            );
        }
    }
}
