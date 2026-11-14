// SPDX-License-Identifier: CC0-1.0

//! The rules BOTH kinds of recipe this service holds follow.
//!
//! An image recipe and a container recipe are two kinds of one thing: each names the
//! components to apply, each builds for ONE platform, and each is what a build takes its
//! image's name, version, platform and kind from. So the rules that hold over a recipe
//! whichever kind it is live here once, rather than in each create and each build path:
//!
//! * [`ComAmazonawsImagebuilderImagebuilderState::resolve_recipe_components`] — every
//!   `componentArn` a recipe includes has to name a stored component of the calling Region,
//!   each component is named "only one time in a recipe", and the components of one recipe
//!   all target one platform, which is then the recipe's own;
//! * [`ComAmazonawsImagebuilderImagebuilderState::resolve_built_recipe`] — what a build takes
//!   from the recipe its reference names, with the kind of recipe deciding the kind of image:
//!   "For an image recipe, this is always AMI" (the modelled `type` member of `ImageRecipe`),
//!   and a container recipe's images are container images.

use std::collections::BTreeSet;

use crate::ops::types::ComponentConfiguration;

use super::{
    duplicate_recipe_component_message, mixed_component_platforms_message, normalized_name,
    ComAmazonawsImagebuilderImagebuilderState, ContainerRecipeRecord, ImageRecipeRecord,
    IMAGE_TYPE_AMI, IMAGE_TYPE_DOCKER,
};

/// What the components one create listed resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeComponents {
    /// Every reference resolved: the ONE platform they target, or `None` for a recipe that
    /// lists no component at all, whose platform comes from its parent image instead
    /// ([`super::image_recipe_platform`]).
    Target(Option<String>),
    /// One reference names nothing this Region holds, carrying the ARN the request gave: a
    /// reference that resolves to nothing is reported MISSING rather than stored.
    Missing(String),
    /// The list is not one a recipe may include; the sentence states why.
    Refused(String),
}

/// What a build takes from the recipe it builds.
///
/// The image a build produces takes its name and its three assigned version nodes from that
/// recipe, builds for the recipe's platform, and is of the kind the recipe's own kind
/// produces — so one reading serves a build a caller asks for by hand and a build a pipeline
/// execution queues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltRecipe {
    /// The recipe's name, which is the image's.
    pub name: String,
    /// The recipe's three assigned version nodes, which are the image's.
    pub version: String,
    /// The ONE platform the recipe builds for.
    pub platform: String,
    /// The IMAGE recipe this build is of, when it is of one: the modelled `imageRecipe`
    /// member of `Image` is "the image recipe that Image Builder used to create the image",
    /// so a build of a CONTAINER recipe carries none.
    pub image_recipe_arn: Option<String>,
    /// Whether the build produces an AMI or a container image, from the kind of recipe it
    /// builds.
    pub image_type: &'static str,
}

impl From<&ImageRecipeRecord> for BuiltRecipe {
    fn from(recipe: &ImageRecipeRecord) -> Self {
        Self {
            name: recipe.name.clone(),
            version: recipe.version.clone(),
            platform: recipe.platform.clone(),
            // The reference is resolved at the START of the build, so the build records the
            // recipe version it actually built rather than the wildcard reference it was
            // reached by.
            image_recipe_arn: Some(recipe.arn.clone()),
            image_type: IMAGE_TYPE_AMI,
        }
    }
}

impl From<&ContainerRecipeRecord> for BuiltRecipe {
    fn from(recipe: &ContainerRecipeRecord) -> Self {
        Self {
            name: recipe.name.clone(),
            version: recipe.version.clone(),
            platform: recipe.platform.clone(),
            image_recipe_arn: None,
            image_type: IMAGE_TYPE_DOCKER,
        }
    }
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// What the `components` a recipe of `request_region` includes resolve to.
    ///
    /// Every reference is resolved the way every other component reference of this service is
    /// ([`Self::resolve_component`]), so a reference naming nothing — or a component of
    /// another Region — is reported missing rather than stored. "You can specify each
    /// component only one time in a recipe", regardless of version, so a name named twice is
    /// refused; and the components of one recipe all target one platform, so a list spanning
    /// two of them names no platform the recipe could report.
    #[must_use]
    pub fn resolve_recipe_components(
        &self,
        components: &[ComponentConfiguration],
        request_region: &str,
    ) -> RecipeComponents {
        let mut target: Option<String> = None;
        let mut included: BTreeSet<String> = BTreeSet::new();
        for configuration in components {
            let Some(component) =
                self.resolve_component(&configuration.component_arn, request_region)
            else {
                return RecipeComponents::Missing(configuration.component_arn.clone());
            };
            if !included.insert(normalized_name(&component.name)) {
                return RecipeComponents::Refused(duplicate_recipe_component_message(
                    &component.name,
                ));
            }
            match target.as_deref() {
                None => target = Some(component.platform.clone()),
                Some(named) if named == component.platform => {}
                Some(named) => {
                    return RecipeComponents::Refused(mixed_component_platforms_message(
                        named,
                        &component.platform,
                    ))
                }
            }
        }
        RecipeComponents::Target(target)
    }

    /// What a build of the recipe the ARN `reference` names takes from it, when one is stored
    /// in `request_region` — whichever of the two kinds of recipe that reference names.
    ///
    /// Each kind is resolved by its own resolution rule, so a reference whose trailing version
    /// nodes are `x` wildcards reaches the newest matching recipe of that kind at the instant
    /// the build starts.
    #[must_use]
    pub fn resolve_built_recipe(
        &self,
        reference: &str,
        request_region: &str,
    ) -> Option<BuiltRecipe> {
        if let Some(recipe) = self.resolve_image_recipe(reference, request_region) {
            return Some(BuiltRecipe::from(recipe));
        }
        self.resolve_container_recipe(reference, request_region)
            .map(BuiltRecipe::from)
    }
}
