// SPDX-License-Identifier: CC0-1.0

//! The REGION a layer belongs to, and what a function's `Layers` list has to satisfy.
//!
//! # A layer is a regional resource
//!
//! A layer version is published into ONE Region and every ARN it is named by carries that
//! Region (`arn:<partition>:lambda:<region>:<account>:layer:<name>:<version>`), so the
//! Region is part of the identity of a layer rather than a label on it: the same layer name
//! in two Regions is two layers, each with its own version numbers. That is why the store
//! is keyed by the pair ([`LayerKey`]) and every read goes through
//! [`ComAmazonawsLambdaAwsgirapiserviceState::layer`],
//! [`ComAmazonawsLambdaAwsgirapiserviceState::layer_mut`] or
//! [`ComAmazonawsLambdaAwsgirapiserviceState::layers_in`] — a layer published elsewhere is
//! not held under the Region a request addressed, so a bare name no layer of that Region
//! was ever published under reports as absent.
//!
//! # What a `LayerName` may name
//!
//! Because the Region is part of a layer's identity, a `LayerName` given in its ARN form
//! names a layer of THIS endpoint only when the Region the ARN carries is the endpoint's
//! own: an ARN of any other Region is not the name of a layer here at all, so it is refused
//! as an invalid layer name rather than resolved to whatever this Region happens to hold
//! under the same bare name. [`layer_name_in`] is that one reading of the member, shared by
//! every operation that takes it.
//!
//! # What a function's `Layers` may name
//!
//! `Layers` is "a list of function layers to add to the function's execution environment",
//! and the execution environment it is added to runs in the function's own Region, so a
//! layer version this Region does not hold cannot be in it. Four rules follow, and
//! [`resolve_function_layers`] applies the first three wherever a function's layer list is
//! resolved (`CreateFunction`, `UpdateFunctionConfiguration`):
//!
//! * the list names no more layers than a function may reference
//!   ([`too_many_layers_message`]);
//! * every entry names a layer version of the FUNCTION's Region
//!   ([`foreign_region_layers_message`]) — a layer Lambda itself provides
//!   (`arn:<partition>:lambda:::awslayer:<name>`) belongs to no Region and is exempt;
//! * no two entries name versions of ONE layer ([`same_layer_versions_message`]), because
//!   the environment mounts one copy of a layer and two versions of it are two copies;
//! * and what the whole environment EXPANDS TO — the function's own package plus every layer
//!   it mounts — is under the published unzipped ceiling
//!   ([`combined_unzipped_refusal`]), which the resolved list is what makes measurable.
//!
//! All are stated as the modelled `InvalidParameterValueException` by the handler that
//! calls them, which is the 400 the operations declare for a member they cannot satisfy.

use super::*;

/// The key one layer is stored under: the Region it was published into, then its bare
/// layer name.
///
/// In that order, so a `BTreeMap` of them groups a Region's layers together and orders
/// them by name inside the group — which is the order `ListLayers` pages through.
pub type LayerKey = (String, String);

/// The key `name` has in `region`; see [`LayerKey`].
#[must_use]
pub fn layer_key(region: &str, name: &str) -> LayerKey {
    (region.to_string(), name.to_string())
}

impl ComAmazonawsLambdaAwsgirapiserviceState {
    /// The layer `name` published in `region`, or `None` when this Region holds no layer
    /// under that name — including when ANOTHER Region holds one.
    #[must_use]
    pub fn layer(&self, region: &str, name: &str) -> Option<&LayerRecord> {
        self.layers.get(&layer_key(region, name))
    }

    /// [`Self::layer`] for an operation that CHANGES the layer it resolves.
    #[must_use]
    pub fn layer_mut(&mut self, region: &str, name: &str) -> Option<&mut LayerRecord> {
        self.layers.get_mut(&layer_key(region, name))
    }

    /// Every layer published in `region`, in layer-name order — the layers a request made
    /// in that Region can see at all.
    ///
    /// The Region is the map key's PREFIX ([`LayerKey`]), so the Region's layers are one
    /// contiguous range of the map: it is walked from the Region's first possible key and
    /// stopped at the first key of another Region, rather than scanned whole.
    pub fn layers_in<'s>(&'s self, region: &'s str) -> impl Iterator<Item = &'s LayerRecord> + 's {
        self.layers
            .range(layer_key(region, "")..)
            .take_while(move |((held, _), _)| held == region)
            .map(|(_, record)| record)
    }
}

/// What a `LayerName` NAMES: the bare layer name, and the Region when the value carried
/// one.
///
/// The member is documented as "The name or Amazon Resource Name (ARN) of the layer", and
/// only the ARN form carries a Region — a bare name addresses the Region the request was
/// made in — so the Region is optional here.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LayerReference {
    /// The Region the value named, or `None` when it named none.
    region: Option<String>,
    /// The bare layer name the value carries.
    name: String,
}

/// Split a `LayerName` into the parts that identify a layer, or `None` when the value is
/// neither documented form.
///
/// Private, because the parts are not what an operation resolving the member reads: the
/// Region is part of WHICH layer the value names, so every caller goes through
/// [`layer_name_in`], which is the one reading of it.
fn parse_layer_reference(value: &str) -> Option<LayerReference> {
    let parts: Vec<&str> = value.split(':').collect();
    let (region, name) = if parts.first() == Some(&"arn") {
        // arn:<partition>:lambda:<region>:<account>:layer:<name>. The resource type is
        // required, so an ARN of another Lambda resource is not read as a layer named
        // after its resource type.
        if parts.len() != 7 || parts[2] != ARN_NAMESPACE || parts[5] != "layer" {
            return None;
        }
        (Some(parts[3].to_string()), parts[6])
    } else if parts.len() == 1 {
        (None, parts[0])
    } else {
        return None;
    };
    is_layer_name(name).then(|| LayerReference {
        region,
        name: name.to_string(),
    })
}

/// The refusal a `LayerName` this endpoint cannot read as the name of one of ITS layers
/// gets: the fixed prefix and the value exactly as the request supplied it, because what
/// the caller has to change is that value.
fn invalid_layer_name_message(layer_name: &str) -> String {
    format!("Invalid Layer name: {layer_name}")
}

/// The bare layer name a `LayerName` names for a request made in `region`, or the message
/// refusing the value.
///
/// A layer is a REGIONAL resource (see this module's header), and the ARN form of the
/// member carries the Region the layer was published into, so a `LayerName` names a layer
/// of this endpoint only when it names this endpoint's own Region or names no Region at
/// all. An ARN of another Region therefore names no layer this endpoint can address at
/// all — it is not a layer name HERE — and is refused as an invalid layer name rather
/// than resolved to whatever the same bare name happens to be published as locally.
///
/// ONE resolution, shared by every operation that takes a `LayerName`
/// (`GetLayerVersion`, `ListLayerVersions`, `DeleteLayerVersion`, `PublishLayerVersion`
/// and the layer-version policy operations through
/// [`crate::state::resolve_layer_version`]), so none of them can read the member by a
/// rule of its own.
pub fn layer_name_in(value: &str, region: &str) -> Result<String, String> {
    let reference =
        parse_layer_reference(value).ok_or_else(|| malformed_layer_name_message(value))?;
    if reference
        .region
        .as_deref()
        .is_some_and(|named| named != region)
    {
        return Err(invalid_layer_name_message(value));
    }
    Ok(reference.name)
}

/// The refusal a `LayerName` in neither documented form gets, which states the member and
/// the forms it admits.
fn malformed_layer_name_message(layer_name: &str) -> String {
    format!(
        "1 validation error detected: Value '{layer_name}' at 'layerName' failed to satisfy \
         constraint: Member must satisfy one of the documented layer name formats (name or \
         layer ARN)"
    )
}

/// How many layers one function may reference: AWS's published Lambda quota, "Layers per
/// function: 5" — the same quota the run-time archive cap states (`super::execution`'s
/// `MAX_RUN_LAYER_ARCHIVES`).
pub const MAX_FUNCTION_LAYERS: usize = 5;

/// The refusal a function's `Layers` gets for naming more layers than a function may have.
///
/// Fixed text around the quota itself: the list as a whole is what is too long, and the
/// caller sent every entry of it, so nothing of the request is named back.
///
/// Recorded behaviour: corpus `simparity-wire-aws-v1`, fixture
/// `lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling`, step `task#6`, where a
/// `CreateFunction` naming more entries than the quota is answered
/// `400 InvalidParameterValueException` with `message` exactly this — and answered it for
/// the COUNT even though the same list breaks the one-version-per-layer rule as well, which
/// is why the count is judged first.
#[must_use]
pub fn too_many_layers_message() -> String {
    format!("Cannot reference more than {MAX_FUNCTION_LAYERS} layers.")
}

/// The refusal a function whose whole execution environment expands past the published
/// unzipped ceiling gets, or `None` when it is under it.
///
/// "The unzipped deployment package size" Lambda measures against
/// [`CODE_SIZE_UNZIPPED_LIMIT`] ("250 MB (unzipped)") is what the environment holds, which
/// is the function's own package TOGETHER WITH every layer mounted into it — that is why a
/// layer entry reports an `UncompressedCodeSize` of its own, "so a client can sum its layers
/// and see how much of that limit is left". So the sum is what is judged, and either half
/// alone being under the ceiling is not enough.
///
/// The sentence states the ceiling and the size the request measured to, because those are
/// the two numbers a caller has to compare; the sizes are measured off the archives this
/// emulator holds ([`unzipped_bytes`] and the stored layer versions' own measurements), so
/// nothing of the request is echoed back.
///
/// Recorded behaviour: corpus `simparity-wire-aws-v1`, fixture
/// `lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling`, step `task#7`, where a
/// `CreateFunction` whose package and layers together expand past the ceiling is answered
/// `400 InvalidParameterValueException` with `message` of exactly this form.
#[must_use]
pub fn combined_unzipped_refusal(
    code_zip_bytes: Option<&[u8]>,
    layers: &[FunctionLayer],
) -> Option<String> {
    let total = layers.iter().fold(unzipped_bytes(code_zip_bytes), |sum, layer| {
        sum.saturating_add(layer.uncompressed_code_size)
    });
    (total > CODE_SIZE_UNZIPPED_LIMIT).then(|| {
        format!(
            "Function code combined with layers exceeds the maximum allowed size of \
             {CODE_SIZE_UNZIPPED_LIMIT} bytes. The actual size is {total} bytes."
        )
    })
}

/// The refusal a function's `Layers` gets for an entry naming a layer version of another
/// Region.
///
/// Fixed text but for the Region, which is the FUNCTION's own — the Region the request was
/// made in, which is the Region the function being created or updated lives in — because
/// that is the Region its layers are expected to be in. The entry itself is not named: the
/// sentence is about the list as a whole, and the caller sent it.
#[must_use]
pub fn foreign_region_layers_message(region: &str) -> String {
    format!(
        "Layers are not in the same region as the function. Layers are expected to be in \
         region {region}."
    )
}

/// The refusal a function's `Layers` gets for two entries naming versions of one layer.
///
/// The two conflicting `LayerVersionArn`s as the request SUBMITTED them, in the order the
/// list carried them, because what the caller has to change is those two entries.
#[must_use]
pub fn same_layer_versions_message(first: &str, second: &str) -> String {
    format!(
        "Two different versions of the same layer are not allowed to be referenced in the \
         same function. {first} and {second} are versions of the same layer."
    )
}

/// Resolve a function's `Layers` — "A list of function layers to add to the function's
/// execution environment. Specify each layer by its ARN, including the version" — against
/// the layer versions `region` holds.
///
/// `region` is the FUNCTION's Region: the one the request was made in, which is the one its
/// ARNs carry.
///
/// The error carries the message of the refusal: a list longer than a function may reference
/// ([`too_many_layers_message`]), an entry that is not a layer version ARN, an entry naming
/// another Region's layer ([`foreign_region_layers_message`]), two entries naming one layer
/// ([`same_layer_versions_message`]), or an entry naming a version this Region holds no layer
/// for. Each rule is judged over the WHOLE list before the next one is, so a list breaking
/// two of them is refused for the first rule it breaks rather than for whichever entry
/// happens to come first.
///
/// A layer Lambda itself provides belongs to no account and to no Region, so it is taken as
/// named: this emulator holds no archive for it and therefore reports the `CodeSize`
/// member's own default for it, exactly as it does for a deployment package whose bytes it
/// does not hold.
pub fn resolve_function_layers(
    store: &ComAmazonawsLambdaAwsgirapiserviceState,
    region: &str,
    layers: &[String],
) -> Result<Vec<FunctionLayer>, String> {
    // How MANY layers the list names is a property of the list itself, settled before
    // anything is read out of any entry.
    if layers.len() > MAX_FUNCTION_LAYERS {
        return Err(too_many_layers_message());
    }

    // Every entry has to BE a layer version ARN before anything can be said about which
    // layer or which Region it names.
    let named: Vec<(&String, LayerVersionReference)> = layers
        .iter()
        .map(|arn| {
            let reference = parse_layer_version_arn(arn).ok_or_else(|| {
                format!(
                    "1 validation error detected: Value '{arn}' at 'layers' failed to satisfy \
                     constraint: Member must be the ARN of a layer version, including the version"
                )
            })?;
            Ok((arn, reference))
        })
        .collect::<Result<_, String>>()?;

    if let Some(message) = foreign_region_entry(&named, region) {
        return Err(message);
    }
    if let Some(message) = two_versions_of_one_layer(&named) {
        return Err(message);
    }

    named
        .into_iter()
        .map(|(arn, reference)| match reference {
            LayerVersionReference::AwsManaged => Ok(FunctionLayer {
                arn: arn.clone(),
                code_size: 0,
                uncompressed_code_size: 0,
                zip_bytes: None,
            }),
            LayerVersionReference::Published { name, version, .. } => {
                // Region-scoped: the Region was settled above, so a name this Region holds
                // no layer under is a version that does not exist here.
                let published = store
                    .layer(region, &name)
                    .and_then(|layer| Some((layer, layer.versions.get(&version)?)))
                    .ok_or_else(|| format!("Layer version {arn} does not exist."))?;
                // The ARN and both sizes come out of the stored version, so nothing of the
                // request is echoed back as a resolved layer.
                Ok(FunctionLayer {
                    arn: published.0.version_arn(version),
                    code_size: published.1.code_size,
                    uncompressed_code_size: published.1.unzipped_code_size(),
                    zip_bytes: published
                        .1
                        .archive
                        .zip_bytes
                        .clone()
                        .map(::std::sync::Arc::new),
                })
            }
        })
        .collect()
}

/// The refusal for the first entry of `named` that names a layer version of a Region other
/// than `region`, or `None` when every entry names one of it.
fn foreign_region_entry(
    named: &[(&String, LayerVersionReference)],
    region: &str,
) -> Option<String> {
    named
        .iter()
        .filter_map(|(_, reference)| match reference {
            // A layer Lambda provides names no Region at all, so there is no Region of it
            // to disagree with the function's.
            LayerVersionReference::AwsManaged => None,
            LayerVersionReference::Published { region: named, .. } => Some(named),
        })
        .any(|named| named != region)
        .then(|| foreign_region_layers_message(region))
}

/// The refusal for the first two entries of `named` that name versions of ONE layer, or
/// `None` when every entry names a different layer.
///
/// Two entries name one layer when their ARNs agree on everything but the version — the
/// Region and the layer name, which is what a layer IS — so the pair is found by grouping
/// on that, in the order the list carried the entries.
fn two_versions_of_one_layer(named: &[(&String, LayerVersionReference)]) -> Option<String> {
    let mut seen: BTreeMap<LayerKey, &String> = BTreeMap::new();
    for (arn, reference) in named {
        let LayerVersionReference::Published { region, name, .. } = reference else {
            continue;
        };
        match seen.insert(layer_key(region, name), arn) {
            Some(first) => return Some(same_layer_versions_message(first, arn)),
            None => continue,
        }
    }
    None
}
