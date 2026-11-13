// SPDX-License-Identifier: CC0-1.0

//! The inherent methods of [`super::ComAmazonawsIotsitewiseAwsiotsitewiseState`]:
//! the resolution, materialisation and propagation every handler asks the store
//! for.

use super::*;

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The interface relationship between two STORED asset model ids, if one was
    /// applied. Resolve both labels with [`Self::resolve_asset_model_id`] first.
    pub fn interface_relationship(
        &self,
        asset_model_id: &str,
        interface_asset_model_id: &str,
    ) -> Option<&InterfaceRelationshipRecord> {
        self.interface_relationships
            .get(&interface_relationship_key(asset_model_id, interface_asset_model_id))
    }

    /// Which property of which interface asset model each property of
    /// `asset_model_id` answers for, keyed by that model's own property id.
    ///
    /// A property of a model belongs to an interface exactly while the
    /// application of that interface to the model maps one of the interface's
    /// properties to it — the pairing
    /// [`InterfaceRelationshipRecord::property_mappings`] holds — so this is a
    /// read of the stored relationships rather than a second derivation of
    /// them. One model may implement several interfaces, so one of its
    /// properties may answer for a property of each; a property no application
    /// mapped belongs to none and is absent from the map. Each list is ordered
    /// by the interface and then its property, so two reads of one store report
    /// one order.
    pub fn interface_properties_by_asset_model_property(
        &self,
        asset_model_id: &str,
    ) -> HashMap<String, Vec<MappedInterfaceProperty>> {
        let mut mapped: HashMap<String, Vec<MappedInterfaceProperty>> = HashMap::new();
        for relationship in self
            .interface_relationships
            .values()
            .filter(|relationship| relationship.asset_model_id == asset_model_id)
        {
            for mapping in &relationship.property_mappings {
                mapped
                    .entry(mapping.asset_model_property_id.clone())
                    .or_default()
                    .push(MappedInterfaceProperty {
                        interface_asset_model_id: relationship.interface_asset_model_id.clone(),
                        interface_asset_model_property_id: mapping
                            .interface_asset_model_property_id
                            .clone(),
                    });
            }
        }
        for interfaces in mapped.values_mut() {
            interfaces.sort();
        }
        mapped
    }

    /// Drop every mapping of every stored interface relationship that names a
    /// property or a hierarchy `asset_model_id` no longer declares.
    ///
    /// A mapping pairs one member of the interface asset model with one member of
    /// the model the interface is applied to, so it is addressable only while BOTH
    /// models still declare their half of it: a mapping naming a member the model
    /// has since dropped would name nothing, exactly as a composite-model link
    /// pointing at a composite model its asset model no longer holds does. The
    /// model stands on either side of a relationship — it may implement
    /// interfaces and, being an interface, be implemented by other models — so
    /// both halves are checked against it. Asked after a call rewrites this
    /// model's definition; a model that is GONE takes its relationships whole
    /// ([`Self::forget_asset_model`]).
    ///
    /// Only the mappings are pruned: the relationship itself is the application of
    /// one model to another, which no change to a definition undoes, and mapping
    /// nothing is a state an application can already resolve to.
    pub fn prune_interface_relationship_mappings(&mut self, asset_model_id: &str) {
        let Some(record) = self.asset_models.get(asset_model_id) else {
            return;
        };
        let declared_properties: Vec<String> = record
            .properties
            .iter()
            .filter_map(|property| property.id.clone())
            .collect();
        let declared_hierarchies: Vec<String> = record
            .hierarchies
            .iter()
            .filter_map(|hierarchy| hierarchy.id.clone())
            .collect();
        for ((applied_to, interface), relationship) in self.interface_relationships.iter_mut() {
            let applied_to_side = applied_to == asset_model_id;
            let interface_side = interface == asset_model_id;
            if !applied_to_side && !interface_side {
                continue;
            }
            relationship.property_mappings.retain(|mapping| {
                (!applied_to_side
                    || declared_properties.contains(&mapping.asset_model_property_id))
                    && (!interface_side
                        || declared_properties
                            .contains(&mapping.interface_asset_model_property_id))
            });
            relationship.hierarchy_mappings.retain(|mapping| {
                (!applied_to_side
                    || declared_hierarchies.contains(&mapping.asset_model_hierarchy_id))
                    && (!interface_side
                        || declared_hierarchies
                            .contains(&mapping.interface_asset_model_hierarchy_id))
            });
        }
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to a
    /// stored `assetModelId`.
    pub fn resolve_asset_model_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self.asset_model_external_ids.get(external).cloned(),
            None => self.asset_models.contains_key(label).then(|| label.to_string()),
        }
    }

    /// The asset model, other than `except`, that already holds `name`.
    ///
    /// `assetModelName` is documented "A unique name for the asset model", so a
    /// name one asset model holds is not free for another: a create may not take
    /// it, and neither may a rename. `except` is the model the request is about —
    /// a model is never in conflict with the name it already carries — and is
    /// `None` for a create, which is about no stored model yet.
    ///
    /// Answers the RECORD, so the refusal names the holder by the id and the ARN
    /// that model carries rather than by anything rebuilt from the refused
    /// request. One place, so the create and the rename cannot read one rule two
    /// ways.
    pub fn asset_model_holding_name(
        &self,
        name: &str,
        except: Option<&str>,
    ) -> Option<&AssetModelRecord> {
        self.asset_models
            .values()
            .find(|record| record.name == name && Some(record.id.as_str()) != except)
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to a
    /// stored `assetId`.
    pub fn resolve_asset_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self.asset_external_ids.get(external).cloned(),
            None => self.assets.contains_key(label).then(|| label.to_string()),
        }
    }

    /// Make `alias` the one alias the `(asset_id, property_id)` property carries,
    /// or remove the property's alias with `None`.
    ///
    /// A binding lives in TWO places — the stored property's `alias` member, which
    /// `DescribeAsset` reports, and [`Self::property_alias_index`], which the alias
    /// form of a property lookup resolves through — so both move together here and
    /// nowhere else. The alias the property carried before is dropped from the
    /// index (only when the index still attributes it to THIS property, so a
    /// binding another property has since taken over survives).
    ///
    /// `asset_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_id`] first. `property_id` takes either `CustomID`
    /// form. Answers `false` when no stored asset holds that property.
    ///
    /// A caller that binds an alias another property already carries displaces it:
    /// that property's own `alias` member is cleared here too, so no two stored
    /// properties can ever report one alias while the index names only the newer.
    /// `UpdateAssetProperty` refuses such a request with the modelled conflict
    /// before reaching this helper; this keeps the store consistent for any caller
    /// whose own rule allows the rebind.
    ///
    /// An asset's `last_update_date` is the instant of the last accepted write to
    /// it, and a property's alias binding is part of the asset that carries that
    /// property — so an accepted change to the binding moves the owning asset's
    /// instant to `now`, here and not in each caller, and the asset a displaced
    /// binding is taken FROM moves with it. A call that leaves the binding as it
    /// stood writes nothing, so a request that changes nothing cannot move it.
    pub fn set_property_alias(
        &mut self,
        asset_id: &str,
        property_id: &str,
        alias: Option<String>,
        now: SystemTime,
    ) -> bool {
        let Some(asset) = self.assets.get_mut(asset_id) else {
            return false;
        };
        // Every instance of the property that id names, so a definition two
        // composite models contribute cannot end up reporting the binding under
        // one of them and none under the other: the id names one property and
        // one stream, hence one binding.
        let positions = asset.property_positions(property_id);
        let Some(&first) = positions.first() else {
            return false;
        };
        let property = &asset.properties[first];
        let resolved_property_id = property.id.clone();
        let property_data_type = property.data_type.clone();
        let property_data_type_spec = property.data_type_spec.clone();
        let previous = property.alias.clone();
        for at in positions {
            asset.properties[at].alias = alias.clone();
        }
        if previous != alias {
            asset.last_update_date = now;
        }

        if let Some(previous) = previous {
            let ours = self
                .property_alias_index
                .get(&previous)
                .is_some_and(|(bound_asset, bound_property)| {
                    bound_asset == asset_id && *bound_property == resolved_property_id
                });
            if ours {
                self.property_alias_index.remove(&previous);
            }
        }
        if let Some(alias) = alias {
            // An alias names ONE stream: binding it to a property makes that
            // stream associated, so a disassociated one standing under the same
            // alias is that same stream — it stops being listed apart from the
            // property that now owns it, and the samples it holds become that
            // property's history again, merged through the quality-aware
            // single-value-per-timestamp write.
            //
            // Only a stream of the property's OWN data type is that property's
            // data: AWS requires the two to match to associate a time series with
            // an asset property, and every write path here checks a TQV's variant
            // against the property's `PropertyDataType`, so samples of another type
            // are never carried into a history that may not hold them.
            // A stream whose data type is not this property's is NOT destroyed
            // here: it stays where it is, samples and all. `UpdateAssetProperty`
            // refuses such a request with the modelled refusal before reaching
            // this helper — the same shape as the alias-conflict case above — so
            // the only way to arrive here is a caller whose own rule allows the
            // rebind, and nothing a caller stored is dropped on its behalf.
            let same_type = self
                .disassociated_time_series
                .get(&alias)
                .is_some_and(|series| {
                    series.data_type == property_data_type
                        && series.data_type_spec == property_data_type_spec
                });
            if same_type {
                if let Some(series) = self.disassociated_time_series.remove(&alias) {
                    for value in series.values {
                        self.record_property_value(asset_id, &resolved_property_id, value);
                    }
                }
            }
            // Whoever held this alias loses it, record and index together.
            if let Some((held_by_asset, held_by_property)) =
                self.property_alias_index.insert(
                    alias,
                    (asset_id.to_string(), resolved_property_id.clone()),
                )
            {
                let displaced = held_by_asset != asset_id
                    || held_by_property != resolved_property_id;
                if displaced {
                    if let Some(asset) = self.assets.get_mut(&held_by_asset) {
                        let positions = asset.property_positions(&held_by_property);
                        if !positions.is_empty() {
                            for at in positions {
                                asset.properties[at].alias = None;
                            }
                            asset.last_update_date = now;
                        }
                    }
                }
            }
        }
        true
    }

    /// The `(dataType, dataTypeSpec)` of the disassociated stream `alias` names,
    /// when one stands under it.
    ///
    /// What an operation that binds an alias to an asset property compares with
    /// that property's own data type: a time series is associated with an asset
    /// property only where the two match, so a binding that would mix another
    /// type's samples into the property's history is refused by the operation
    /// rather than resolved by dropping stored data.
    pub fn disassociated_stream_type(&self, alias: &str) -> Option<(&str, Option<&str>)> {
        self.disassociated_time_series.get(alias).map(|series| {
            (
                series.data_type.as_str(),
                series.data_type_spec.as_deref(),
            )
        })
    }

    /// Detach the time series the `(asset_id, property_id)` property's alias
    /// names from that property, and keep the series as a disassociated one.
    ///
    /// The alias leaves the property's own `alias` member and
    /// [`Self::property_alias_index`] through [`Self::set_property_alias`], so
    /// nothing resolves the property by it afterwards, while the stream itself
    /// survives as a [`DisassociatedTimeSeriesRecord`] addressable by that alias —
    /// what IoT SiteWise reports as a `DISASSOCIATED` time series, and what
    /// `DeleteTimeSeries` rather than this call forgets.
    ///
    /// The SAMPLES go with the stream, not with the property: a time series is the
    /// data stream, and an asset property reports the values of the stream it is
    /// associated with, so a property the stream has left reports no history until
    /// a stream is associated with it again. They are not dropped — re-binding the
    /// alias to an asset property brings them back ([`Self::set_property_alias`]).
    /// The detached stream gets its own `TimeSeriesId`, since the property's ids
    /// still name a series of their own.
    ///
    /// `asset_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_id`] first. `property_id` takes either `CustomID`
    /// form. Answers `false` — writing nothing — when no stored asset holds that
    /// property or the property carries no alias, so there is no association to
    /// break.
    pub fn disassociate_time_series(
        &mut self,
        asset_id: &str,
        property_id: &str,
        now: SystemTime,
    ) -> bool {
        let Some(asset) = self.assets.get(asset_id) else {
            return false;
        };
        let Some(property) = asset.property(property_id) else {
            return false;
        };
        let Some(alias) = property.alias.clone() else {
            return false;
        };
        let resolved_property_id = property.id.clone();
        let record = DisassociatedTimeSeriesRecord {
            alias,
            time_series_id: new_id(),
            data_type: property.data_type.clone(),
            data_type_spec: property.data_type_spec.clone(),
            creation_date: asset.creation_date,
            last_update_date: now,
            values: self
                .property_values
                .get_mut(asset_id)
                .and_then(|properties| properties.remove(&resolved_property_id))
                .unwrap_or_default(),
        };
        self.set_property_alias(asset_id, property_id, None, now);
        self.keep_disassociated_time_series(record);
        true
    }

    /// Store `record` under its alias, bounded at
    /// [`MAX_DISASSOCIATED_TIME_SERIES`] by dropping the least recently updated
    /// series.
    ///
    /// `DeleteTimeSeries` is what a caller uses to forget one, so the eviction is
    /// the bound on the path that has no limit error to refuse with — detaching a
    /// stream from its property, which
    /// `DisassociateTimeSeriesFromAssetProperty` declares none for. The ingest
    /// path, whose operation DOES declare one, refuses past the cap instead and so
    /// never reaches the eviction.
    pub(super) fn keep_disassociated_time_series(&mut self, record: DisassociatedTimeSeriesRecord) {
        self.disassociated_time_series
            .insert(record.alias.clone(), record);
        while self.disassociated_time_series.len() > MAX_DISASSOCIATED_TIME_SERIES {
            let Some(oldest) = self
                .disassociated_time_series
                .values()
                .min_by_key(|series| series.last_update_date)
                .map(|series| series.alias.clone())
            else {
                break;
            };
            self.disassociated_time_series.remove(&oldest);
        }
    }

    /// Whether ANY stored asset model holds a composite model under `id`.
    ///
    /// A composite model id is globally unique, so both the collision check on a
    /// caller-supplied id and the mint draw ask this one question.
    pub fn composite_model_id_taken(&self, id: &str) -> bool {
        self.asset_model_holding_composite_model(id).is_some()
    }

    /// The stored asset model that holds the composite model `id`, if any does.
    ///
    /// A composite model is addressable only through the asset model that holds
    /// it, and its id is globally unique, so this is how a caller holding only
    /// that id reaches the record it lives on.
    pub fn asset_model_holding_composite_model(&self, id: &str) -> Option<&AssetModelRecord> {
        self.asset_models
            .values()
            .find(|model| model.composite_model(id).is_some())
    }

    /// The property definitions composite model `composite_id` of `model`
    /// contributes: its OWN definitions, or, for a component-model-based
    /// composite model, the properties of the `COMPONENT_MODEL` it reuses.
    ///
    /// A component-model-based composite model stores no properties of its own —
    /// the component model it names provides them, and they are read from that
    /// model's stored record rather than copied at create time, so a later change
    /// to the component model shows through. One definition of "the properties of
    /// a composite model", so every view of one (the model's own, and an asset's
    /// instance of it) answers from the same place.
    pub fn composite_model_properties(
        &self,
        model: &AssetModelRecord,
        composite_id: &str,
    ) -> Vec<AssetModelProperty> {
        self.composite_model_property_slice(model, composite_id)
            .to_vec()
    }

    /// The same definitions [`Self::composite_model_properties`] answers with,
    /// BORROWED from wherever they are stored: the composite model's own list, or
    /// the `COMPONENT_MODEL`'s record for a component-model-based one.
    ///
    /// The primitive that one clones from, so there is still one definition of
    /// "the properties of a composite model": a caller that only has to LOOK at
    /// them — a membership question over a model's exposed properties — asks here
    /// and copies nothing.
    pub fn composite_model_property_slice<'a>(
        &'a self,
        model: &'a AssetModelRecord,
        composite_id: &str,
    ) -> &'a [AssetModelProperty] {
        match model.composed_asset_model_id(composite_id) {
            Some(composed) => self
                .asset_models
                .get(composed)
                .map(|component| component.properties.as_slice())
                .unwrap_or_default(),
            None => model
                .composite_model(composite_id)
                .and_then(|composite| composite.properties.as_deref())
                .unwrap_or_default(),
        }
    }

    /// The property of asset `asset_id` that a `CustomID` label names — a raw
    /// property id, or `externalId:<externalId>` — over EVERY property that asset
    /// carries: the ones materialised from its model's own definitions and the ones
    /// the model's composite models contribute.
    ///
    /// A property a composite model of the asset's model declares is a property OF
    /// THAT ASSET, addressed by the same `(assetId, propertyId)` pair as one
    /// declared on the model directly, so this is the ONE definition of "the
    /// property that pair names" and every operation addressed that way resolves
    /// here. BOTH kinds are materialised onto the asset's own list by
    /// [`Self::materialise_asset_properties`], so the property a listing reports
    /// and the one a data-plane read resolves are the same STORED property, with
    /// the alias and the notification `UpdateAssetProperty` set on it.
    ///
    /// Owned rather than borrowed, so a caller can go on using the store while it
    /// holds the answer.
    pub fn resolve_asset_property(&self, asset_id: &str, label: &str) -> Option<AssetProperty> {
        self.assets.get(asset_id)?.property(label).cloned()
    }

    /// The asset-model property definition an asset property of a model-`model`
    /// asset was materialised from: one of the model's own top-level
    /// definitions, or one a composite model of it contributes.
    ///
    /// The inverse of [`Self::materialise_asset_properties`], asked by every
    /// operation that reports or defaults a member the asset property does not
    /// carry itself (a property's `type`, the `unit` a definition declares), so
    /// a composite model's property answers those from its definition exactly as
    /// a base property does.
    pub fn asset_model_property_definition(
        &self,
        model: &AssetModelRecord,
        property_id: &str,
    ) -> Option<AssetModelProperty> {
        if let Some(found) = model
            .properties
            .iter()
            .find(|property| property.id.as_deref() == Some(property_id))
        {
            return Some(found.clone());
        }
        model
            .composite_models
            .iter()
            .filter_map(|composite| composite.id.as_deref())
            .flat_map(|composite_id| self.composite_model_properties(model, composite_id))
            .find(|property| property.id.as_deref() == Some(property_id))
    }

    /// Every asset property an asset created from `model` carries, in the order
    /// a reader enumerates them, together with the value each one starts its
    /// series with.
    ///
    /// An asset instantiated from an asset model carries an INSTANCE of every
    /// composite model that model holds, so the list is the model's own
    /// top-level property definitions followed by the definitions each composite
    /// model contributes — through [`Self::composite_model_properties`], so a
    /// component-model-based composite model contributes the properties of the
    /// component model it reuses. One FLAT list, so every operation that
    /// addresses an asset property by id reaches a composite model's property
    /// exactly as it reaches a base one; which of the two a property is is read
    /// off the `path` it carries (see [`composite_model_of_property`]).
    ///
    /// Two composite models may reuse the same component model, so one
    /// definition can appear once per composite model that contributes it, each
    /// instance under its own path and all of them sharing the definition's id
    /// — hence the one time series that id addresses
    /// ([`AssetRecord::time_series_properties`]).
    ///
    /// Shared by the create that materialises an asset and the propagation that
    /// re-materialises it, so the two cannot disagree about what an asset of a
    /// model carries.
    pub fn materialise_asset_properties(
        &self,
        model: &AssetModelRecord,
        now: SystemTime,
    ) -> MaterialisedAssetProperties {
        let mut materialised = MaterialisedAssetProperties::default();
        let mut seen: HashSet<String> = HashSet::new();
        for definition in &model.properties {
            materialise_one(definition, None, now, &mut seen, &mut materialised);
        }
        // `composite_models` is the FLAT list of every composite model the asset
        // model holds, at any nesting depth — a nested one is linked to its
        // parent through `composite_model_links` rather than held inside it — so
        // walking it once reaches every composite model's properties, and the
        // path prefix carries the whole chain each sits in.
        for composite in &model.composite_models {
            let Some(composite_id) = composite.id.as_deref() else {
                continue;
            };
            let prefix: Vec<AssetPropertyPathSegment> = model
                .composite_model_chain(composite_id)
                .into_iter()
                .map(|segment| AssetPropertyPathSegment {
                    id: segment.id,
                    name: segment.name,
                })
                .collect();
            for definition in self.composite_model_properties(model, composite_id) {
                materialise_one(
                    &definition,
                    Some(&prefix),
                    now,
                    &mut seen,
                    &mut materialised,
                );
            }
        }
        materialised
    }

    /// Append `value` to the `(asset_id, property_id)` series, keeping it sorted
    /// ascending by timestamp.
    ///
    /// The insert itself is [`record_sample`], shared with the write path an
    /// alias-addressed ingest of a disassociated stream takes, so the
    /// quality-aware single-value-per-timestamp rule and the
    /// [`MAX_SAMPLES_PER_SERIES`] bound hold for every stream alike.
    pub fn record_property_value(
        &mut self,
        asset_id: &str,
        property_id: &str,
        value: AssetPropertyValue,
    ) {
        let series = self
            .property_values
            .entry(asset_id.to_string())
            .or_default()
            .entry(property_id.to_string())
            .or_default();
        record_sample(series, value);
    }

    /// Append `value` to the stream `key` names, answering `false` when no stream
    /// does.
    ///
    /// The one write path for a time series, so an alias-addressed ingest reaches
    /// a disassociated stream on exactly the terms
    /// [`Self::record_property_value`] writes an associated one on — the
    /// quality-aware single-value-per-timestamp rule and the
    /// [`MAX_SAMPLES_PER_SERIES`] bound.
    pub fn record_stream_value(&mut self, key: &SeriesKey, value: AssetPropertyValue) -> bool {
        match key {
            SeriesKey::Property(asset_id, property_id) => {
                let asset_id = asset_id.clone();
                let property_id = property_id.clone();
                self.record_property_value(&asset_id, &property_id, value);
                true
            }
            SeriesKey::Disassociated(alias) => {
                match self.disassociated_time_series.get_mut(alias) {
                    Some(series) => {
                        record_sample(&mut series.values, value);
                        true
                    }
                    None => false,
                }
            }
        }
    }

    /// The stream a `propertyAlias` names: the asset property it is associated
    /// with, or the disassociated series standing under it. `None` when no stream
    /// answers to that alias at all.
    ///
    /// An alias names ONE data stream whether or not an asset property is
    /// associated with it — IoT SiteWise reports a series as `ASSOCIATED` or
    /// `DISASSOCIATED` and `DeleteTimeSeries` identifies a disassociated one "if
    /// the time series isn't associated with an asset property, specify the alias
    /// of the time series" — so every operation addressed by an alias resolves it
    /// here and reaches the same stream.
    pub fn resolve_alias_series(&self, alias: &str) -> Option<SeriesKey> {
        if let Some((asset_id, property_id)) = self.property_alias_index.get(alias) {
            return Some(SeriesKey::Property(asset_id.clone(), property_id.clone()));
        }
        self.disassociated_time_series
            .contains_key(alias)
            .then(|| SeriesKey::Disassociated(alias.to_string()))
    }

    /// The samples of the stream `key` names, oldest first. Empty for a stream
    /// nothing has been written to, and for one `key` names no longer.
    pub fn series(&self, key: &SeriesKey) -> &[AssetPropertyValue] {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .property_values
                .get(asset_id)
                .and_then(|properties| properties.get(property_id))
                .map_or(&[][..], Vec::as_slice),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get(alias)
                .map_or(&[][..], |series| series.values.as_slice()),
        }
    }

    /// The `PropertyDataType` of the stream `key` names: the declared type of the
    /// asset property that owns it, or the type the disassociated record kept.
    /// `None` when the store holds no such stream.
    ///
    /// The counterpart of [`Self::series`] for a reader that needs the stream's
    /// TYPE rather than its samples: one data stream reports one data type whether
    /// an asset property is associated with it or not, so every reader takes it
    /// from here instead of reaching only into [`Self::assets`]. The property is
    /// resolved through [`Self::resolve_asset_property`], so a stream owned by a
    /// property one of the model's composite models declares reports its type too.
    pub fn series_data_type(&self, key: &SeriesKey) -> Option<String> {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .resolve_asset_property(asset_id, property_id)
                .map(|property| property.data_type),
            SeriesKey::Disassociated(alias) => self
                .disassociated_stream_type(alias)
                .map(|(data_type, _)| data_type.to_string()),
        }
    }

    /// The `PropertyAlias` the stream `key` names answers to: the alias carried by
    /// the asset property that owns it, or the alias a disassociated stream is
    /// itself keyed by. `None` when the store holds no such stream, and for a
    /// stream whose owning asset property carries no alias at all.
    ///
    /// The counterpart of [`Self::series_data_type`] for a reader that needs the
    /// stream's ALIAS: one data stream answers to one alias whether an asset
    /// property is associated with it or not, so every reader takes it from here
    /// rather than reaching into [`Self::assets`] or
    /// [`Self::disassociated_time_series`] on its own.
    pub fn series_alias(&self, key: &SeriesKey) -> Option<&str> {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .assets
                .get(asset_id)
                .and_then(|asset| asset.property(property_id))
                .and_then(|property| property.alias.as_deref()),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get(alias)
                .map(|series| series.alias.as_str()),
        }
    }

    /// The highest-timestamp value stored for `(asset_id, property_id)`, if any.
    ///
    /// The property's OWN series, borrowing the two ids the way
    /// [`Self::property_values`] is nested for. A caller that wants the stream an
    /// alias names, associated or not, resolves it with
    /// [`Self::resolve_alias_series`] and reads [`Self::series`].
    pub fn latest_property_value(
        &self,
        asset_id: &str,
        property_id: &str,
    ) -> Option<&AssetPropertyValue> {
        self.property_values.get(asset_id)?.get(property_id)?.last()
    }

    /// Create the DISASSOCIATED time series an alias no asset property carries
    /// addresses, answering `false` when a stream already answers to that alias.
    ///
    /// This is what makes alias-addressed ingestion work: IoT SiteWise accepts a
    /// `propertyAlias` entry before any asset property carries that alias and
    /// implicitly creates the data stream it names, which `DescribeTimeSeries` and
    /// `ListTimeSeries` then report and `AssociateTimeSeriesToAssetProperty` binds.
    /// `data_type` is the type of the value that created it — the stream has no
    /// asset property to take one from — and a later value of another type does not
    /// change it.
    ///
    /// The caller checks [`MAX_DISASSOCIATED_TIME_SERIES`] first and refuses past
    /// it; the insert itself goes through the same bounded keeper the detach path
    /// uses, so the collection cannot grow past the cap either way.
    pub fn create_disassociated_time_series(
        &mut self,
        alias: &str,
        data_type: &str,
        now: SystemTime,
    ) -> bool {
        if self.resolve_alias_series(alias).is_some() {
            return false;
        }
        self.keep_disassociated_time_series(DisassociatedTimeSeriesRecord {
            alias: alias.to_string(),
            time_series_id: new_id(),
            data_type: data_type.to_string(),
            data_type_spec: None,
            creation_date: now,
            last_update_date: now,
            values: Vec::new(),
        });
        true
    }

    /// Forget the disassociated stream `alias` names, answering the record that was
    /// removed.
    ///
    /// What `DeleteTimeSeries` does to a stream no asset property owns: the stream
    /// goes, samples and all, and the alias names nothing afterwards.
    pub fn forget_disassociated_time_series(
        &mut self,
        alias: &str,
    ) -> Option<DisassociatedTimeSeriesRecord> {
        self.disassociated_time_series.remove(alias)
    }

    /// Forget the series an asset property owns: its alias binding and its stored
    /// samples, leaving the property itself in place.
    ///
    /// What `DeleteTimeSeries` does to an ASSOCIATED stream — "If you delete a time
    /// series that's associated with an asset property, the asset property still
    /// exists, but the time series will no longer be associated with this asset
    /// property" — so a series removed by its alias and the same series removed by
    /// its `(assetId, propertyId)` pair leave the store in one state.
    ///
    /// Dropping the binding is an accepted write to the asset that carries the
    /// property, so `now` is the instant its `last_update_date` moves to (see
    /// [`Self::set_property_alias`]).
    pub fn forget_property_series(&mut self, asset_id: &str, property_id: &str, now: SystemTime) {
        self.set_property_alias(asset_id, property_id, None, now);
        if let Some(properties) = self.property_values.get_mut(asset_id) {
            properties.remove(property_id);
        }
    }

    /// Store `segment` on the dataset `dataset_id`, answering `false` — writing
    /// nothing — when the store cannot hold it: a bound of its span is not an
    /// instant the modelled `TimeInNanos` can express, the span covers no time
    /// (its start is not before its end), or the dataset already holds
    /// [`MAX_DATA_SEGMENTS_PER_DATASET`] segments.
    ///
    /// This is the PRODUCER-side entry point — the one writer of a dataset's
    /// segments, which `BatchDeleteDatasetDataSegments` then removes from:
    /// `BatchAssociateDataSegmentsToDataset` stores every segment it accepts
    /// here, [`Self::ingest_data_segments`] stores the ones a bulk import
    /// brings into a session dataset, and a test seeding a dataset directly
    /// reaches the same writer, so everything a read or a deletion can find was
    /// written here and nowhere else.
    ///
    /// The log stays sorted by [`data_segment_key`] and single-valued on it:
    /// storing the same series' same span again REPLACES the entry held for it
    /// rather than holding the segment twice.
    pub fn put_dataset_data_segment(
        &mut self,
        dataset_id: &str,
        segment: DataSegmentRecord,
    ) -> bool {
        if !is_modelled_time(&segment.start_timestamp)
            || !is_modelled_time(&segment.end_timestamp)
        {
            return false;
        }
        if timestamp_key(&segment.start_timestamp) >= timestamp_key(&segment.end_timestamp) {
            return false;
        }
        let held = self.dataset_data_segments.entry(dataset_id.to_string()).or_default();
        let key = data_segment_key(&segment);
        match held.binary_search_by(|stored| data_segment_key(stored).cmp(&key)) {
            Ok(at) => held[at] = segment,
            Err(at) => {
                // Past the cap the write is REFUSED rather than evicting a
                // segment: a deletion addresses a segment by its own span, so a
                // silently dropped one would read as a segment that never
                // existed. The cap is at least one, so a dataset reaching it
                // already held segments and no empty entry is left behind here.
                if held.len() >= MAX_DATA_SEGMENTS_PER_DATASET {
                    return false;
                }
                held.insert(at, segment);
            }
        }
        true
    }

    /// Record the data segments a bulk import into the SESSION dataset
    /// `dataset_id` brings in from `files`, answering how many it stored.
    ///
    /// The PRODUCER of a session dataset's own segments. A session dataset
    /// "contains data segments of time series data" (the `CreateDataset` model
    /// doc) and an import names it as "the session dataset to ingest data into"
    /// (the `CreateBulkImportJob` doc), so an accepted import is what puts
    /// segments on one: each file names the time series its `alias` identifies
    /// ("the alias associated with the file's time series") and the instant its
    /// data begins ("the nanosecond-precision start time for the file data"),
    /// which is the span of that stream the dataset comes to hold.
    ///
    /// The files themselves are in Amazon S3, and a `csv` or `parquet` file's
    /// SAMPLES are not read from it, so no sample is invented (see
    /// [`BulkImportJobRecord`]): the data a segment covers is the data this
    /// store really holds for the stream the file names. A file naming no
    /// alias, an alias no stored stream answers to, or an instant past every
    /// sample that stream holds therefore brings in nothing and records no
    /// segment. (An `mp4` file carries media rather than samples, and what it
    /// brings in is video read from the object itself —
    /// [`Self::ingest_capture_segments`].)
    ///
    /// A span overlapping a segment this store already holds of that series
    /// records nothing either — "Data segments that belong to the same time
    /// series can't overlap in time, regardless of which dataset they belong
    /// to" (the `BatchAssociateDataSegmentsToDataset` doc) — which is also what
    /// makes a second import of the same file add no second segment.
    pub fn ingest_data_segments(&mut self, dataset_id: &str, files: &[File]) -> usize {
        let mut stored = 0usize;
        for file in files {
            let Some(segment) = self.imported_data_segment(dataset_id, file) else {
                continue;
            };
            if self
                .overlapping_data_segment(
                    &segment.time_series_id,
                    &segment.start_timestamp,
                    &segment.end_timestamp,
                )
                .is_some()
            {
                continue;
            }
            if self.put_dataset_data_segment(dataset_id, segment) {
                stored += 1;
            }
        }
        stored
    }

    /// The segment the import of `file` into `dataset_id` brings in, or `None`
    /// when it brings in nothing at all; see [`Self::ingest_data_segments`] for
    /// the rule.
    ///
    /// The span is HALF-OPEN, as every segment's span is, so it ends ONE
    /// nanosecond past the last sample it covers — the smallest instant the
    /// modelled `TimeInNanos` can express past it, so the segment covers that
    /// sample and no further data. Its start is the instant the file states, and
    /// the first covered sample's own instant for a file that states none.
    pub(super) fn imported_data_segment(&self, dataset_id: &str, file: &File) -> Option<DataSegmentRecord> {
        let stream = self.resolve_alias_series(file.alias.as_deref()?)?;
        let time_series_id = self.series_time_series_id(&stream)?;
        let samples = self.series(&stream);
        let from = file.start_time.as_ref().map(timestamp_key);
        // The samples are kept sorted ascending, so the first one inside the
        // window is the one the span starts at and the series' last sample is
        // the one it ends after.
        let first = samples
            .iter()
            .map(|sample| &sample.timestamp)
            .find(|at| from.is_none_or(|from| timestamp_key(at) >= from))?;
        let start = match &file.start_time {
            Some(start) => start.clone(),
            None => first.clone(),
        };
        let end = next_nanosecond(&samples.last()?.timestamp)?;
        Some(DataSegmentRecord {
            // A session dataset's own ingested data came from the dataset that
            // holds it, which is what tells it apart from a segment another
            // dataset's data was selected into.
            source_dataset_id: dataset_id.to_string(),
            time_series_id,
            start_timestamp: start,
            end_timestamp: end,
        })
    }

    /// Every data segment of `time_series_id` this store holds whose span
    /// shares an instant with the half-open span `[start, end)`, paired with
    /// the dataset holding it.
    ///
    /// "Data segments that belong to the same time series can't overlap in
    /// time, regardless of which dataset they belong to" (the
    /// `BatchAssociateDataSegmentsToDataset` model doc), so the search spans
    /// EVERY dataset's segments rather than the one a request names.
    pub(super) fn overlapping_data_segments<'a, 'k>(
        &'a self,
        time_series_id: &'k str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> impl Iterator<Item = (&'a str, &'a DataSegmentRecord)> + 'k
    where
        'a: 'k,
    {
        let (start, end) = (timestamp_key(start), timestamp_key(end));
        self.dataset_data_segments
            .iter()
            .flat_map(|(dataset_id, held)| {
                held.iter()
                    .map(move |segment| (dataset_id.as_str(), segment))
            })
            .filter(move |(_, segment)| {
                segment.time_series_id == time_series_id
                    // Two half-open spans overlap exactly when each starts
                    // before the other ends, so spans that merely touch at one
                    // instant — the end of one, the start of the next — do not.
                    && timestamp_key(&segment.start_timestamp) < end
                    && timestamp_key(&segment.end_timestamp) > start
            })
    }

    /// The data segment of `time_series_id` ANY dataset holds whose span shares
    /// an instant with the half-open span `[start, end)`: the dataset that
    /// holds it, paired with the segment itself, or `None` when none does.
    ///
    /// What a PRODUCER of a segment reads the no-overlap rule through — a
    /// session dataset's own ingested data included, which is why this one asks
    /// about every segment and [`Self::overlapping_data_segment_association`]
    /// asks only about the associations. Answered in dataset-then-span order,
    /// so one store answers one way whatever order its map iterates in.
    pub fn overlapping_data_segment(
        &self,
        time_series_id: &str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> Option<(&str, &DataSegmentRecord)> {
        self.overlapping_data_segments(time_series_id, start, end)
            .min_by(|(left_id, left), (right_id, right)| {
                (*left_id, data_segment_key(left)).cmp(&(*right_id, data_segment_key(right)))
            })
    }

    /// The ASSOCIATION of `time_series_id` whose span shares an instant with
    /// the half-open span `[start, end)`: the dataset that holds it, paired with
    /// the segment itself, or `None` when no dataset holds one.
    ///
    /// The no-overlap rule [`Self::overlapping_data_segment`] reads, narrowed to
    /// the segments an association put there: a segment whose data came from the
    /// dataset that HOLDS it is a source dataset's own data — what an
    /// association selects a slice of (see
    /// [`DataSegmentRecord::source_dataset_id`]) — rather than an association
    /// of it, so it is not one of the segments this rule compares.
    ///
    /// Answered in dataset-then-span order, so one store answers one way
    /// whatever order its map iterates in.
    pub fn overlapping_data_segment_association(
        &self,
        time_series_id: &str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> Option<(&str, &DataSegmentRecord)> {
        self.overlapping_data_segments(time_series_id, start, end)
            .filter(|(dataset_id, segment)| segment.source_dataset_id != *dataset_id)
            .min_by(|(left_id, left), (right_id, right)| {
                (*left_id, data_segment_key(left)).cmp(&(*right_id, data_segment_key(right)))
            })
    }

    /// A data segment of `time_series_id` any dataset of this store holds,
    /// paired with the dataset holding it, or `None` when none does.
    ///
    /// The dependency `DeleteTimeSeries` refuses to break: "You can't delete a
    /// time series until all of its data segments have been deleted from session
    /// datasets" (its model doc). EVERY segment naming the series counts,
    /// whichever dataset holds it and whichever dataset its data came from
    /// ([`DataSegmentRecord::source_dataset_id`]) — a session dataset's own
    /// segment covers that stream's data and a curated dataset's association
    /// covers the same data from elsewhere, so either one left behind would
    /// name a stream the store no longer holds. That is the wider question
    /// [`Self::overlapping_data_segment_association`] does not ask: it compares
    /// one span against the ASSOCIATIONS of a series, while this asks whether
    /// anything at all still covers it.
    ///
    /// Answered in dataset-then-span order, so one store answers one way
    /// whatever order its map iterates in.
    pub fn data_segment_of_time_series(
        &self,
        time_series_id: &str,
    ) -> Option<(&str, &DataSegmentRecord)> {
        self.dataset_data_segments
            .iter()
            .flat_map(|(holder, held)| {
                held.iter().map(move |segment| (holder.as_str(), segment))
            })
            .filter(|(_, segment)| segment.time_series_id == time_series_id)
            .min_by(|(left_id, left), (right_id, right)| {
                (*left_id, data_segment_key(left)).cmp(&(*right_id, data_segment_key(right)))
            })
    }

    /// The stream among `time_series_ids` a dataset still holds a data segment
    /// of, as `(timeSeriesId, datasetId)`, or `None` when no dataset holds a
    /// segment of any of them.
    ///
    /// [`Self::data_segment_of_time_series`] asked of a GROUP of streams: a
    /// call that would drop SEVERAL streams at once — every property stream of
    /// an asset it removes, the streams of the properties a model update takes
    /// away — breaks the same dependency as a call that drops one, so it is
    /// refused by the same question rather than by a rule of its own. Answered
    /// in stream-then-dataset order, so one store answers one way whatever
    /// order its maps iterate in and whatever order the caller offers the
    /// streams in.
    ///
    /// One pass over the segment index, like the single-stream question: the
    /// streams offered become a set and each segment is tested against it once,
    /// so asking about many streams costs the same walk as asking about one
    /// rather than one walk per stream.
    pub fn data_segment_of_any_time_series<I>(
        &self,
        time_series_ids: I,
    ) -> Option<(String, String)>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let covered: HashSet<String> = time_series_ids
            .into_iter()
            .map(|id| id.as_ref().to_string())
            .collect();
        if covered.is_empty() {
            return None;
        }
        self.blocking_data_segment(|time_series_id| covered.contains(time_series_id))
    }

    /// The first segment, in stream-then-dataset order, whose covered stream
    /// `covers` accepts, as `(timeSeriesId, datasetId)`.
    ///
    /// The single walk of [`Self::dataset_data_segments`] every "is this stream
    /// still depended on" question takes, so each of them costs one pass over
    /// the index whatever shape its set of streams has.
    pub(super) fn blocking_data_segment(
        &self,
        covers: impl Fn(&str) -> bool,
    ) -> Option<(String, String)> {
        let mut blocked: Option<(&str, &str)> = None;
        for (holder, held) in &self.dataset_data_segments {
            for segment in held {
                if !covers(&segment.time_series_id) {
                    continue;
                }
                let candidate = (segment.time_series_id.as_str(), holder.as_str());
                if blocked.is_none_or(|held| candidate < held) {
                    blocked = Some(candidate);
                }
            }
        }
        blocked.map(|(series, holder)| (series.to_string(), holder.to_string()))
    }

    /// The `TimeSeriesId` of every stream asset `asset_id` owns: one per
    /// property it carries, which is what the `(assetId, propertyId)` pair
    /// names ([`time_series_id`]).
    ///
    /// The streams a call that removes the asset would drop, so the streams it
    /// has to ask [`Self::data_segment_of_any_time_series`] about. Empty for an
    /// asset this store does not hold, and derived from the properties rather
    /// than from the samples written to them: a segment names the pair, which
    /// resolves through the property, not through the data behind it.
    pub fn asset_property_time_series_ids(&self, asset_id: &str) -> Vec<String> {
        self.assets
            .get(asset_id)
            .map(|asset| {
                asset
                    .properties
                    .iter()
                    .map(|property| time_series_id(asset_id, &property.id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The stream a dataset still holds a data segment of among the streams the
    /// assets of `asset_model_id` would lose were that model's properties to
    /// become `kept`, as `(timeSeriesId, datasetId)`.
    ///
    /// The same dependency, asked of the removal a model update propagates: "If
    /// you remove a property from an asset model, IoT SiteWise deletes all
    /// previous data for that property" (`UpdateAssetModel`'s doc), so a
    /// property the new definition drops takes the stream of every asset of
    /// that model with it — and a stream a dataset still holds a segment of
    /// cannot go. `None` when the new definition drops no property, when no
    /// asset was created from the model, or when nothing holds a segment of the
    /// streams it would drop; asked BEFORE the new definition is written, which
    /// is why the kept properties are passed in rather than read back.
    ///
    /// Answered from the SEGMENTS rather than from the streams, in the one pass
    /// [`Self::blocking_data_segment`] makes: a segment names a stream by the
    /// pair that derives it, so each one is resolved back to its asset and
    /// property and tested against the dropped set, which never materialises
    /// the product of the model's assets and its dropped properties.
    ///
    /// Every asset the propagation reaches counts, not only those created
    /// directly from this model: a composite model that reuses this one as a
    /// component model contributes its definitions, under their own ids, to the
    /// assets of the model holding it, so a property dropped here takes those
    /// assets' streams too — the same set of models
    /// [`Self::models_inheriting_definitions_of`] walks.
    pub fn data_segment_of_dropped_model_properties(
        &self,
        asset_model_id: &str,
        kept: &[AssetModelProperty],
    ) -> Option<(String, String)> {
        let dropped: HashSet<&str> = self
            .asset_models
            .get(asset_model_id)?
            .properties
            .iter()
            .filter_map(|property| property.id.as_deref())
            .filter(|id| !id.is_empty())
            .filter(|id| !kept.iter().any(|kept| kept.id.as_deref() == Some(*id)))
            .collect();
        if dropped.is_empty() {
            return None;
        }
        let inheriting = self.models_inheriting_definitions_of(asset_model_id);
        self.blocking_data_segment(|time_series_id| {
            let Some((asset_id, property_id)) =
                time_series_id.split_once(TIME_SERIES_ID_SEPARATOR)
            else {
                return false;
            };
            if !dropped.contains(property_id) {
                return false;
            }
            // The stream goes only if the asset really carries that property and
            // its own model inherits the definition being dropped — which is
            // exactly when the propagation re-materialises the asset without it.
            self.assets.get(asset_id).is_some_and(|asset| {
                inheriting.contains(&asset.asset_model_id)
                    && asset.property(property_id).is_some()
            })
        })
    }

    /// Forget the segment of `dataset_id` that `time_series_id` and the span
    /// `[start, end)` name EXACTLY, answering whether one was held.
    ///
    /// What `BatchDeleteDatasetDataSegments` does to one entry: a segment is
    /// addressed by its time series and its own span, so a span no segment of
    /// the dataset covers exactly removes nothing and the caller reports that
    /// entry as the absence it is.
    pub fn delete_dataset_data_segment(
        &mut self,
        dataset_id: &str,
        time_series_id: &str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> bool {
        let Some(held) = self.dataset_data_segments.get(dataset_id) else {
            return false;
        };
        let Some(at) = data_segment_at(held, time_series_id, start, end) else {
            return false;
        };
        self.forget_dataset_data_segment_at(dataset_id, at);
        true
    }

    /// Forget the segment of `dataset_id` that `source_dataset_id`,
    /// `time_series_id` and the span `[start, end)` name EXACTLY, answering
    /// whether one was held.
    ///
    /// What `BatchDisassociateDataSegmentsFromDataset` does to one entry: the
    /// time series and the span are the segment's identity inside the dataset
    /// that holds it, and the source names the dataset the segment's data came
    /// from, so a segment the dataset holds under a different source is not the
    /// one addressed and nothing is removed. Only the association goes: the
    /// data the segment covered belongs to the source dataset and is untouched
    /// here, which is what separates this from
    /// [`Self::delete_dataset_data_segment`], whose caller deletes that data
    /// too.
    pub fn disassociate_dataset_data_segment(
        &mut self,
        dataset_id: &str,
        source_dataset_id: &str,
        time_series_id: &str,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> bool {
        let Some(held) = self.dataset_data_segments.get(dataset_id) else {
            return false;
        };
        let Some(at) = data_segment_at(held, time_series_id, start, end) else {
            return false;
        };
        if held[at].source_dataset_id != source_dataset_id {
            return false;
        }
        self.forget_dataset_data_segment_at(dataset_id, at);
        true
    }

    /// Drop the segment `dataset_id` holds at `at` in its sorted log — the one
    /// remover both paths above go through, so a dataset emptied by either is
    /// left in the same shape.
    pub(super) fn forget_dataset_data_segment_at(&mut self, dataset_id: &str, at: usize) {
        let Some(held) = self.dataset_data_segments.get_mut(dataset_id) else {
            return;
        };
        held.remove(at);
        // A dataset that holds no segments holds no entry either, so the map does
        // not grow an empty vector per dataset a removal has emptied.
        if held.is_empty() {
            self.dataset_data_segments.remove(dataset_id);
        }
    }

    /// The dataset that holds a data segment whose data came FROM `dataset_id`,
    /// paired with that segment, or `None` when nothing references its data.
    ///
    /// A segment names the dataset its data came from
    /// ([`DataSegmentRecord::source_dataset_id`]), so a segment ANOTHER dataset
    /// holds under this one references this dataset's data — the reference
    /// `DeleteDataset` refuses to break: "You can't delete a session dataset
    /// while a curated dataset references its data segments." A segment whose
    /// data came from the dataset that HOLDS it is that dataset's own data and
    /// references nothing, read exactly as
    /// [`Self::overlapping_data_segment_association`] reads it.
    ///
    /// Answered in dataset-then-span order, so one store answers one way
    /// whatever order its map iterates in.
    pub fn dataset_data_reference(&self, dataset_id: &str) -> Option<(&str, &DataSegmentRecord)> {
        self.dataset_data_segments
            .iter()
            .flat_map(|(holder, held)| {
                held.iter().map(move |segment| (holder.as_str(), segment))
            })
            .filter(|(holder, segment)| {
                segment.source_dataset_id == dataset_id && *holder != dataset_id
            })
            .min_by(|(left_id, left), (right_id, right)| {
                (*left_id, data_segment_key(left)).cmp(&(*right_id, data_segment_key(right)))
            })
    }

    /// Forget a dataset and the data segments it held, answering the record it
    /// removed.
    ///
    /// A segment whose data came from the dataset that HELD it is that dataset's
    /// OWN data, so the samples its span covers go with it — "Deleting a session
    /// dataset also deletes the underlying time series data in the session" —
    /// through [`Self::forget_series_window`], the one window remover a single
    /// segment's deletion already goes through. A segment whose source is
    /// ANOTHER dataset only REFERENCES that dataset's data, so just the
    /// reference goes: "Deleting a curated dataset doesn't delete the underlying
    /// data in the source session datasets", the same split
    /// [`Self::disassociate_dataset_data_segment`] makes for one segment.
    ///
    /// The dataset's tags go with its record, which owns them. A dataset whose
    /// data another dataset still references is NOT refused here:
    /// [`Self::dataset_data_reference`] is what the caller refuses it with.
    ///
    /// A bulk import job that was created to ingest INTO this dataset goes too:
    /// its destination is the dataset, this service models no call that deletes
    /// a job on its own, and a job the store kept would report a `datasetId`
    /// this store no longer holds — the same reason
    /// [`Self::forget_workspace`] takes the jobs of a workspace with it.
    pub fn forget_dataset(&mut self, dataset_id: &str) -> Option<DatasetRecord> {
        let record = self.datasets.remove(dataset_id)?;
        self.bulk_import_jobs
            .retain(|_, job| job.dataset_id.as_deref() != Some(dataset_id));
        let held = self
            .dataset_data_segments
            .remove(dataset_id)
            .unwrap_or_default();
        for segment in held {
            if segment.source_dataset_id != dataset_id {
                continue;
            }
            // A segment whose time series the store no longer holds has no data
            // left to delete, which is no reason to keep the dataset.
            if let Some(stream) = self.series_by_time_series_id(&segment.time_series_id) {
                self.forget_series_window(
                    &stream,
                    &segment.start_timestamp,
                    &segment.end_timestamp,
                );
            }
        }
        Some(record)
    }

    /// The stored data stream whose own `TimeSeriesId` is `time_series_id`, or
    /// `None` when no stream of this store carries it.
    ///
    /// The inverse of [`time_series_id`]: a stream an asset property owns is
    /// named by its `(assetId, propertyId)` pair joined by
    /// [`TIME_SERIES_ID_SEPARATOR`], and a stream no property owns carries a
    /// `TimeSeriesId` of its own, minted when the stream appeared. One resolver,
    /// so an operation that addresses a time series by that id reaches the same
    /// stream the alias and the id-pair forms reach.
    pub fn series_by_time_series_id(&self, time_series_id: &str) -> Option<SeriesKey> {
        if let Some((asset_id, property_id)) = time_series_id.split_once(TIME_SERIES_ID_SEPARATOR)
        {
            return self
                .assets
                .get(asset_id)
                .filter(|asset| asset.property(property_id).is_some())
                .map(|_| SeriesKey::Property(asset_id.to_string(), property_id.to_string()));
        }
        self.disassociated_time_series
            .values()
            .find(|series| series.time_series_id == time_series_id)
            .map(|series| SeriesKey::Disassociated(series.alias.clone()))
    }

    /// The `TimeSeriesId` of the stream `key` names, or `None` when this store
    /// holds no such stream.
    ///
    /// The inverse of [`Self::series_by_time_series_id`], through the same two
    /// identities: the `(assetId, propertyId)` pair of a stream an asset
    /// property owns ([`time_series_id`]), and the id minted for a stream no
    /// property owns. One derivation, so a producer that records a stream's id
    /// and a reader that resolves it back cannot disagree about which stream it
    /// names.
    pub fn series_time_series_id(&self, key: &SeriesKey) -> Option<String> {
        match key {
            SeriesKey::Property(asset_id, property_id) => self
                .assets
                .get(asset_id)
                .filter(|asset| asset.property(property_id).is_some())
                .map(|_| time_series_id(asset_id, property_id)),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get(alias)
                .map(|series| series.time_series_id.clone()),
        }
    }

    /// Forget every sample of the stream `key` names whose timestamp falls inside
    /// the HALF-OPEN window `[start, end)`, answering how many went.
    ///
    /// What "deleting a data segment deletes the underlying time series data for
    /// the segment's time range" (the `BatchDeleteDatasetDataSegments` model doc)
    /// does to the stream a segment covers: the samples inside that segment's own
    /// span go and every other sample of the stream stays, so the data of a
    /// neighbouring segment survives the deletion of this one. Answers 0 for a
    /// stream `key` names no longer, and leaves the series sorted ascending.
    pub fn forget_series_window(
        &mut self,
        key: &SeriesKey,
        start: &TimeInNanos,
        end: &TimeInNanos,
    ) -> usize {
        let series = match key {
            SeriesKey::Property(asset_id, property_id) => self
                .property_values
                .get_mut(asset_id)
                .and_then(|properties| properties.get_mut(property_id)),
            SeriesKey::Disassociated(alias) => self
                .disassociated_time_series
                .get_mut(alias)
                .map(|series| &mut series.values),
        };
        let Some(series) = series else {
            return 0;
        };
        let (start, end) = (timestamp_key(start), timestamp_key(end));
        let before = series.len();
        series.retain(|sample| {
            let at = timestamp_key(&sample.timestamp);
            at < start || at >= end
        });
        before - series.len()
    }

    /// The search `workspace` holds under `search_id`, or `None` when that
    /// workspace holds none: a search is reachable only through the workspace it
    /// was started in.
    pub fn search(&self, workspace_name: &str, search_id: &str) -> Option<&SearchRecord> {
        self.searches.get(&search_key(workspace_name, search_id))
    }

    /// How many searches `workspace` holds — what the per-workspace bound reads.
    pub fn searches_in_workspace(&self, workspace_name: &str) -> usize {
        self.searches
            .keys()
            .filter(|(workspace, _)| workspace == workspace_name)
            .count()
    }

    /// How many enrichment jobs `workspace_name` holds — what the per-workspace
    /// bound reads.
    pub fn enrichment_jobs_in_workspace(&self, workspace_name: &str) -> usize {
        self.enrichment_jobs
            .keys()
            .filter(|(workspace, _)| workspace == workspace_name)
            .count()
    }

    /// The enrichment job of `workspace_name` that is doing the same work as
    /// `configuration` and has not reached a terminal status at `now`, if any.
    ///
    /// What the duplicate-job rule reads: a create is refused while "a duplicate
    /// job (same workspace, dataset, property, and job type) is currently
    /// running", so two jobs are compared on the work they do
    /// ([`enrichment_job_target`]) rather than on the whole request.
    pub fn active_enrichment_job(
        &self,
        workspace_name: &str,
        configuration: &EnrichmentJobConfiguration,
        now: SystemTime,
    ) -> Option<&EnrichmentJobRecord> {
        let wanted = enrichment_job_target(configuration);
        self.enrichment_jobs.values().find(|record| {
            record.workspace_name == workspace_name
                && enrichment_job_target(&record.configuration) == wanted
                && record.is_active(now)
        })
    }

    /// The enrichment jobs of this store whose configuration names
    /// `dataset_id`, in no particular order.
    ///
    /// A job names "the IoT SiteWise dataset ID containing the video
    /// time-series data to analyze" ([`EnrichmentJobRecord::dataset_id`]), and
    /// `CreateEnrichmentJob` refuses one for a dataset this store does not
    /// hold, so that id is the link between a job and the dataset it analyses —
    /// the one every reader of a dataset's enrichment walks.
    pub fn enrichment_jobs_of_dataset<'a>(
        &'a self,
        dataset_id: &'a str,
    ) -> impl Iterator<Item = &'a EnrichmentJobRecord> + 'a {
        self.enrichment_jobs
            .values()
            .filter(move |record| record.dataset_id() == dataset_id)
    }

    /// Whether `job` analyses the time series `time_series_id` names.
    ///
    /// `EventDetection` identifies the video it analyses by "exactly one
    /// time-series identifier (timeSeriesId OR propertyAlias)", and both
    /// members are optional: an alias is resolved through the same stream
    /// registry every alias-addressed read of this store goes through, so one
    /// naming no stream this store holds names no series of the dataset either,
    /// and a configuration naming neither identifier analyses the dataset's
    /// video by the trim window alone — every series of it.
    pub(super) fn enrichment_job_analyses_series(
        &self,
        job: &EnrichmentJobRecord,
        time_series_id: &str,
    ) -> bool {
        if let Some(named) = job.time_series_id() {
            return named == time_series_id;
        }
        if let Some(alias) = job.property_alias() {
            return self
                .resolve_alias_series(alias)
                .and_then(|key| self.series_time_series_id(&key))
                .is_some_and(|resolved| resolved == time_series_id);
        }
        true
    }

    /// When the enrichment of `segment`, a data segment of the dataset whose
    /// enrichment `jobs` are, completed as of `now` — and `None` while nothing
    /// has enriched it.
    ///
    /// A segment is enriched by an enrichment job of its dataset that has
    /// reached `COMPLETED`, analyses the segment's time series and whose trim
    /// window contains the segment's span WHOLE: "Trim settings retrieve fully
    /// contained data segments within the specified time range", so a segment
    /// reaching outside the window is not one that job analysed. Of several such
    /// jobs the LATEST completion is the instant reported, which is the
    /// `lastEnrichedAt` of "the date the data was last enriched".
    ///
    /// The dataset's jobs are taken as a slice the caller gathered, so a dataset
    /// with many segments walks the store's jobs ONCE rather than once per
    /// segment it holds.
    pub(super) fn segment_enriched_at(
        &self,
        jobs: &[&EnrichmentJobRecord],
        segment: &DataSegmentRecord,
        now: SystemTime,
    ) -> Option<SystemTime> {
        jobs.iter()
            .filter(|job| self.enrichment_job_analyses_series(job, &segment.time_series_id))
            .filter(|job| job.analyses_span(&segment.start_timestamp, &segment.end_timestamp))
            .filter_map(|job| job.completed_at(now))
            .max()
    }

    /// The `enrichment` a data segment of the dataset whose enrichment jobs
    /// `jobs` are reports at `now`, and `None` when that slice is empty — the
    /// dataset no enrichment job names.
    ///
    /// `DataSegmentSummary$enrichment` is optional, so a segment of a dataset
    /// nothing was ever asked to enrich reports none at all. Everything else is
    /// DERIVED, like every other fact an enrichment job's own reads report: a
    /// segment a completed job analysed reports `ENRICHED` at the instant that
    /// job completed, and one no completed job has analysed yet reports
    /// `NOT_ENRICHED` and no instant — there is no date it was last enriched.
    ///
    /// The dataset's jobs are taken as a slice the caller gathered with
    /// [`Self::enrichment_jobs_of_dataset`], so a listing summarising many
    /// segments of one dataset walks the store's jobs ONCE rather than once per
    /// segment.
    pub fn data_segment_enrichment(
        &self,
        jobs: &[&EnrichmentJobRecord],
        segment: &DataSegmentRecord,
        now: SystemTime,
    ) -> Option<DataSegmentEnrichment> {
        if jobs.is_empty() {
            return None;
        }
        let enriched_at = self.segment_enriched_at(jobs, segment, now);
        Some(DataSegmentEnrichment {
            status: match enriched_at {
                Some(_) => ENRICHMENT_ENRICHED,
                None => ENRICHMENT_NOT_ENRICHED,
            }
            .to_string(),
            last_enriched_at: enriched_at.map(to_date_time),
        })
    }

    /// The `enrichmentStatus` the dataset `dataset_id` reports at `now`, and
    /// `None` for a dataset no enrichment job names — `enrichmentStatus` is
    /// optional, and a dataset nothing was ever asked to enrich has no
    /// enrichment to report.
    ///
    /// The entry aggregates exactly the per-segment statuses the dataset's own
    /// segments report ([`Self::data_segment_enrichment`]), so a dataset and its
    /// segments cannot disagree: `FULLY_ENRICHED` when every segment it holds is
    /// enriched, `PARTIALLY_ENRICHED` when only some are, `NOT_ENRICHED` when
    /// none is — which is also what a dataset holding no segment at all reports,
    /// since no enriched data of it is observable. `lastEnrichedAt` is the
    /// latest of those segments' enrichment instants, and absent while none is
    /// enriched.
    ///
    /// It is reported under `video` because the enrichment this API models —
    /// event detection — is the analysis of "video time-series data"; no other
    /// member of `DatasetEnrichment` has an enrichment job to derive it from.
    ///
    /// The dataset's own jobs are gathered here, once for the whole dataset,
    /// because a reader of a dataset asks about that dataset alone.
    pub fn dataset_enrichment(
        &self,
        dataset_id: &str,
        now: SystemTime,
    ) -> Option<DatasetEnrichment> {
        let jobs: Vec<&EnrichmentJobRecord> =
            self.enrichment_jobs_of_dataset(dataset_id).collect();
        if jobs.is_empty() {
            return None;
        }
        let held = self
            .dataset_data_segments
            .get(dataset_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let enriched: Vec<SystemTime> = held
            .iter()
            .filter_map(|segment| self.segment_enriched_at(&jobs, segment, now))
            .collect();
        let status = if enriched.is_empty() {
            DATASET_NOT_ENRICHED
        } else if enriched.len() == held.len() {
            DATASET_FULLY_ENRICHED
        } else {
            DATASET_PARTIALLY_ENRICHED
        };
        Some(DatasetEnrichment {
            video: Some(DatasetEnrichmentEntry {
                status: status.to_string(),
                last_enriched_at: enriched.iter().max().copied().map(to_date_time),
            }),
        })
    }

    /// Store `segment` on the capture source `(workspace_name, time_series_id)`
    /// names, creating the source on first write.
    ///
    /// This is the PRODUCER-side entry point — the one writer of a workspace's
    /// video, which `GetCaptureData` then reads. The producer path that reaches
    /// it is the bulk import of `mp4` files `CreateBulkImportJob` takes
    /// ([`Self::ingest_capture_segments`]); a test seeding a capture source
    /// reaches the same writer, so everything a read of that video must answer
    /// from is written here and nowhere else.
    ///
    /// `alias` BINDS that alias to the source, so the alias form of a lookup
    /// reaches the same record; an alias another source of the same workspace
    /// already carries moves to this one, record and index together, exactly as a
    /// rebound property alias does.
    ///
    /// The segment log stays sorted ascending by start time and single-valued at
    /// a start time — re-ingesting the span replaces the bytes stored for it —
    /// and is bounded at [`MAX_CAPTURE_SEGMENTS_PER_SOURCE`] by dropping the
    /// oldest segment, the same eviction a time series' samples get.
    ///
    /// The `Err` names WHICH rule refused the write ([`CaptureWriteRefusal`]), so
    /// the producer handlers this serves can map one refusal to one modelled
    /// error instead of re-deriving the checks — and nothing is written on any of
    /// them.
    ///
    /// `now` is the instant the write lands: the source's creation date for the
    /// write that creates it, and its last-update date for every write it
    /// accepts, which is what the time series it is reports
    /// ([`CaptureSourceRecord::time_series_summary`]).
    pub fn put_capture_segment(
        &mut self,
        workspace_name: &str,
        time_series_id: &str,
        alias: Option<&str>,
        data_type: &str,
        segment: CaptureSegment,
        now: SystemTime,
    ) -> Result<(), CaptureWriteRefusal> {
        if !self.workspaces.contains_key(workspace_name) {
            return Err(CaptureWriteRefusal::NoSuchWorkspace);
        }
        let id_length = time_series_id.chars().count();
        if !(MIN_TIME_SERIES_ID_LENGTH..=MAX_TIME_SERIES_ID_LENGTH).contains(&id_length) {
            return Err(CaptureWriteRefusal::TimeSeriesIdLength);
        }
        if let Some(alias) = alias {
            let alias_length = alias.chars().count();
            if !(MIN_PROPERTY_ALIAS_LENGTH..=MAX_PROPERTY_ALIAS_LENGTH).contains(&alias_length) {
                return Err(CaptureWriteRefusal::AliasLength);
            }
        }
        if !VIDEO_DATA_TYPE_VALUES.contains(&data_type) {
            return Err(CaptureWriteRefusal::UnmodelledDataType);
        }
        if !is_modelled_time(&segment.start_time) || !is_modelled_time(&segment.end_time) {
            return Err(CaptureWriteRefusal::UnmodelledInstant);
        }
        if timestamp_key(&segment.start_time) >= timestamp_key(&segment.end_time) {
            return Err(CaptureWriteRefusal::EmptySpan);
        }
        // The data type is the media type the source's video is in: it is
        // established by the write that creates the source, so a later write
        // naming another type is refused rather than accepted and ignored — a
        // producer is never told it stored media of a type a read will not
        // report.
        if let Some(source) = self.capture_source_by_id(workspace_name, time_series_id) {
            if source.data_type != data_type {
                return Err(CaptureWriteRefusal::DataTypeMismatch);
            }
        } else if self.capture_source_count() >= MAX_CAPTURE_SOURCES {
            return Err(CaptureWriteRefusal::SourceLimit);
        }

        let source = self
            .capture_sources
            .entry(workspace_name.to_string())
            .or_default()
            .entry(time_series_id.to_string())
            .or_insert_with(|| CaptureSourceRecord {
                workspace_name: workspace_name.to_string(),
                time_series_id: time_series_id.to_string(),
                alias: None,
                data_type: data_type.to_string(),
                creation_date: now,
                last_update_date: now,
                segments: Vec::new(),
            });
        // An accepted write is a write to the series, so its last-update instant
        // moves to this call's — the creation instant stays the one the write
        // that created it stamped.
        source.last_update_date = now;

        // A source's `alias` and the alias index move together: the alias the
        // source carried before is dropped from the index below, so nothing
        // resolves the source by it afterwards.
        let previous_alias = match alias {
            Some(alias) if source.alias.as_deref() != Some(alias) => {
                source.alias.replace(alias.to_string())
            }
            _ => None,
        };

        let at = source.segments.binary_search_by(|stored| {
            timestamp_key(&stored.start_time).cmp(&timestamp_key(&segment.start_time))
        });
        match at {
            Ok(at) => source.segments[at] = segment,
            Err(at) => source.segments.insert(at, segment),
        }
        if source.segments.len() > MAX_CAPTURE_SEGMENTS_PER_SOURCE {
            let excess = source.segments.len() - MAX_CAPTURE_SEGMENTS_PER_SOURCE;
            source.segments.drain(..excess);
        }

        if let Some(alias) = alias {
            let aliases = self
                .capture_source_aliases
                .entry(workspace_name.to_string())
                .or_default();
            if let Some(previous) = previous_alias {
                aliases.remove(&previous);
            }
            // Whoever held this alias in this workspace loses it, record and
            // index together, so no two stored sources can report one alias
            // while the index names only the newer.
            let displaced = aliases.insert(alias.to_string(), time_series_id.to_string());
            if let Some(displaced) = displaced.filter(|held| held != time_series_id) {
                if let Some(source) = self
                    .capture_sources
                    .get_mut(workspace_name)
                    .and_then(|sources| sources.get_mut(&displaced))
                {
                    source.alias = None;
                }
            }
        }
        Ok(())
    }

    /// How many capture sources this store holds, across every workspace — what
    /// [`MAX_CAPTURE_SOURCES`] bounds.
    pub fn capture_source_count(&self) -> usize {
        self.capture_sources
            .values()
            .map(::std::collections::HashMap::len)
            .sum()
    }

    /// The capture source `workspace_name` holds under `time_series_id`, or
    /// `None` when it holds none — which a read of it answers as
    /// `ResourceNotFoundException`.
    pub fn capture_source_by_id(
        &self,
        workspace_name: &str,
        time_series_id: &str,
    ) -> Option<&CaptureSourceRecord> {
        self.capture_sources.get(workspace_name)?.get(time_series_id)
    }

    /// The capture source `workspace_name` holds under the `propertyAlias`
    /// `alias`. The alias index is what makes the two identifying forms resolve
    /// to one source.
    pub fn capture_source_by_alias(
        &self,
        workspace_name: &str,
        alias: &str,
    ) -> Option<&CaptureSourceRecord> {
        let time_series_id = self.capture_source_aliases.get(workspace_name)?.get(alias)?;
        self.capture_source_by_id(workspace_name, time_series_id)
    }

    /// Every time series `workspace_name` holds, in no particular order — what a
    /// listing scoped to that workspace enumerates.
    ///
    /// A workspace's time series ARE the capture sources stored under it: the
    /// streams ingestion recorded a workspace for, which `GetCaptureData` reads
    /// back by the same two identifiers a summary of one reports.
    pub fn workspace_time_series(
        &self,
        workspace_name: &str,
    ) -> impl Iterator<Item = &CaptureSourceRecord> {
        self.capture_sources
            .get(workspace_name)
            .into_iter()
            .flat_map(::std::collections::HashMap::values)
    }

    /// The one time series `workspace_name` holds that `lookup` names, or which
    /// absence stopped the lookup ([`WorkspaceSeriesMiss`]).
    ///
    /// ONE resolution for every operation scoped to a workspace, so the alias
    /// form and the asset-property pair form cannot drift between the handlers
    /// that accept them: the alias resolves through the workspace's alias index,
    /// and a pair resolves to the [`time_series_id`] that pair owns and is then
    /// looked for among the workspace's own series.
    pub fn workspace_series(
        &self,
        workspace_name: &str,
        lookup: SeriesLookup<'_>,
    ) -> Result<&CaptureSourceRecord, WorkspaceSeriesMiss> {
        if !self.workspaces.contains_key(workspace_name) {
            return Err(WorkspaceSeriesMiss::NoSuchWorkspace);
        }
        let (held, requested) = match lookup {
            SeriesLookup::Alias(alias) => (
                self.capture_source_by_alias(workspace_name, alias),
                alias.to_string(),
            ),
            SeriesLookup::Pair(asset_label, property_label) => {
                // Both halves are modelled `CustomID`s, so each can arrive as the
                // raw UUID or as `externalId:<externalId>`; an absent one is the
                // absence it is, exactly as it is for a lookup naming no
                // workspace.
                let asset_id = self
                    .resolve_asset_id(asset_label)
                    .ok_or_else(|| WorkspaceSeriesMiss::NoSuchAsset(asset_label.to_string()))?;
                let property_id = self
                    .assets
                    .get(&asset_id)
                    .and_then(|asset| asset.property(property_label))
                    .map(|property| property.id.clone())
                    .ok_or_else(|| {
                        WorkspaceSeriesMiss::NoSuchProperty(
                            asset_label.to_string(),
                            property_label.to_string(),
                        )
                    })?;
                let time_series_id = time_series_id(&asset_id, &property_id);
                (
                    self.capture_source_by_id(workspace_name, &time_series_id),
                    time_series_id,
                )
            }
        };
        held.ok_or(WorkspaceSeriesMiss::NoSuchSeries(requested))
    }

    /// Forget the time series `workspace_name` holds under `time_series_id`,
    /// answering the record that was removed.
    ///
    /// What `DeleteTimeSeries` scoped to a workspace does: the stream goes,
    /// video and all, and neither its id nor the alias bound to it names
    /// anything in that workspace afterwards.
    pub fn forget_capture_source(
        &mut self,
        workspace_name: &str,
        time_series_id: &str,
    ) -> Option<CaptureSourceRecord> {
        let sources = self.capture_sources.get_mut(workspace_name)?;
        let record = sources.remove(time_series_id)?;
        if sources.is_empty() {
            self.capture_sources.remove(workspace_name);
        }
        if let Some(aliases) = self.capture_source_aliases.get_mut(workspace_name) {
            aliases.retain(|_, held| held != time_series_id);
            if aliases.is_empty() {
                self.capture_source_aliases.remove(workspace_name);
            }
        }
        Some(record)
    }

    /// The logging options `workspace` reports: the ones last set for that scope,
    /// or [`DEFAULT_LOGGING_LEVEL`] while nothing has set any. `None` asks for the
    /// account-and-region-wide scope.
    ///
    /// A pure READ: an unconfigured scope is not materialised, so it still reports
    /// the default to the next caller.
    pub fn logging_options_for(&self, workspace: Option<&str>) -> LoggingOptions {
        self.logging_options
            .get(workspace.unwrap_or(ACCOUNT_LOGGING_SCOPE))
            .map(|record| record.options.clone())
            .unwrap_or_else(|| LoggingOptions {
                level: DEFAULT_LOGGING_LEVEL.to_string(),
            })
    }

    /// Set `workspace`'s logging options, replacing whatever that scope carried.
    /// `None` sets the account-and-region-wide scope. One scope's options are
    /// independent of every other's.
    ///
    /// Bounded at [`MAX_LOGGING_SCOPES`]: past that many scopes the least recently
    /// set WORKSPACE scope is dropped, so a stream of puts naming fresh workspaces
    /// cannot grow this map without limit. A dropped scope reports
    /// [`DEFAULT_LOGGING_LEVEL`] again, which is what it reported before anything
    /// set it; the account-and-region-wide scope is exempt from the eviction, so
    /// the unnamed scope survives any number of workspace-scoped puts.
    pub fn set_logging_options(
        &mut self,
        workspace: Option<&str>,
        options: LoggingOptions,
        now: SystemTime,
    ) {
        self.logging_options.insert(
            workspace.unwrap_or(ACCOUNT_LOGGING_SCOPE).to_string(),
            LoggingOptionsRecord {
                options,
                set_at: now,
            },
        );
        while self.logging_options.len() > MAX_LOGGING_SCOPES {
            let Some(oldest) = self
                .logging_options
                .iter()
                .filter(|(scope, _)| scope.as_str() != ACCOUNT_LOGGING_SCOPE)
                .min_by_key(|(_, record)| record.set_at)
                .map(|(scope, _)| scope.clone())
            else {
                break;
            };
            self.logging_options.remove(&oldest);
        }
    }

    /// The tag set of the resource `arn` names, for reading.
    ///
    /// Resolved through [`parse_resource_arn`] and then by exact ARN equality, so
    /// an ARN that names a stored id in another partition, account or region
    /// resolves to nothing — the resource it names is not one this store holds.
    /// Every record this store holds that carries tags is reachable: the arms
    /// below are the whole list of taggable kinds, and each is one line because
    /// the per-record work lives in [`TaggedRecord`].
    pub fn tags_for_resource(&self, arn: &str) -> Option<&HashMap<String, String>> {
        let (resource_type, id) = parse_resource_arn(arn)?;
        match resource_type {
            ASSET_RESOURCE_TYPE => tags_of(self.assets.get(id), arn),
            ASSET_MODEL_RESOURCE_TYPE => tags_of(self.asset_models.get(id), arn),
            GATEWAY_RESOURCE_TYPE => tags_of(self.gateways.get(id), arn),
            PORTAL_RESOURCE_TYPE => tags_of(self.portals.get(id), arn),
            PROJECT_RESOURCE_TYPE => tags_of(self.projects.get(id), arn),
            PIPELINE_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of(self.pipelines.get(&pipeline_key(workspace, name)), arn)
            }
            TASK_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of(self.tasks.get(&task_key(workspace, name)), arn)
            }
            DATASET_RESOURCE_TYPE => tags_of(self.datasets.get(id), arn),
            APPLICATION_RESOURCE_TYPE => tags_of(self.applications.get(id), arn),
            WORKSPACE_RESOURCE_TYPE => tags_of(self.workspaces.get(id), arn),
            COMPUTATION_MODEL_RESOURCE_TYPE => tags_of(self.computation_models.get(id), arn),
            ACCESS_POLICY_RESOURCE_TYPE => tags_of(self.access_policies.get(id), arn),
            DASHBOARD_RESOURCE_TYPE => tags_of(self.dashboards.get(id), arn),
            _ => None,
        }
    }

    /// The same tag set, for writing. See [`Self::tags_for_resource`].
    pub fn tags_for_resource_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        let (resource_type, id) = parse_resource_arn(arn)?;
        match resource_type {
            ASSET_RESOURCE_TYPE => tags_of_mut(self.assets.get_mut(id), arn),
            ASSET_MODEL_RESOURCE_TYPE => tags_of_mut(self.asset_models.get_mut(id), arn),
            GATEWAY_RESOURCE_TYPE => tags_of_mut(self.gateways.get_mut(id), arn),
            PORTAL_RESOURCE_TYPE => tags_of_mut(self.portals.get_mut(id), arn),
            PROJECT_RESOURCE_TYPE => tags_of_mut(self.projects.get_mut(id), arn),
            PIPELINE_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of_mut(self.pipelines.get_mut(&pipeline_key(workspace, name)), arn)
            }
            TASK_RESOURCE_TYPE => {
                let (workspace, name) = id.split_once('/')?;
                tags_of_mut(self.tasks.get_mut(&task_key(workspace, name)), arn)
            }
            DATASET_RESOURCE_TYPE => tags_of_mut(self.datasets.get_mut(id), arn),
            APPLICATION_RESOURCE_TYPE => tags_of_mut(self.applications.get_mut(id), arn),
            WORKSPACE_RESOURCE_TYPE => tags_of_mut(self.workspaces.get_mut(id), arn),
            COMPUTATION_MODEL_RESOURCE_TYPE => {
                tags_of_mut(self.computation_models.get_mut(id), arn)
            }
            ACCESS_POLICY_RESOURCE_TYPE => tags_of_mut(self.access_policies.get_mut(id), arn),
            DASHBOARD_RESOURCE_TYPE => tags_of_mut(self.dashboards.get_mut(id), arn),
            _ => None,
        }
    }

    /// Forget an asset and everything keyed off it: its external-id entry, its
    /// property series, its property aliases, its hierarchy associations, its
    /// membership of every IoT SiteWise Monitor project, and the actions and
    /// executions that name it.
    pub fn forget_asset(&mut self, asset_id: &str) -> Option<AssetRecord> {
        let record = self.assets.remove(asset_id)?;
        if let Some(external) = record.external_id.as_deref() {
            self.asset_external_ids.remove(external);
        }
        self.property_values.remove(asset_id);
        self.property_alias_index
            .retain(|_, (asset, _)| asset != asset_id);
        self.asset_hierarchy_children
            .retain(|(asset, _), _| asset != asset_id);
        for children in self.asset_hierarchy_children.values_mut() {
            children.retain(|child| child != asset_id);
        }
        self.asset_hierarchy_children
            .retain(|_, children| !children.is_empty());
        // A project's membership names assets, so an asset this service no longer
        // holds is in no project: dropping the entry here is what keeps
        // `ListProjectAssets` from reporting an asset nothing can address.
        for project in self.projects.values_mut() {
            project.asset_ids.remove(asset_id);
        }
        // An action and its execution each NAME the asset they were taken on or
        // the one they resolved to, and the write that records them resolves
        // both references against this store first, so an asset this service no
        // longer holds can leave neither record behind for a read or a listing
        // to report, or for the [`MAX_ACTIONS`] budget to count. The reference
        // both sides are recognised by is the one [`references_asset`] states,
        // so what goes with the asset is exactly what was reported for it — the
        // cleanup [`Self::forget_computation_model`] does for the other kind of
        // resource those same records name.
        self.actions
            .retain(|_, action| !action.references_asset(asset_id));
        self.executions
            .retain(|_, execution| !execution.references_asset(asset_id));
        Some(record)
    }

    /// How many executions of `(workspace_name, pipeline_name)` have not
    /// finished, as [`PipelineExecutionRecord::is_active`] decides it.
    ///
    /// A count rather than a list: the executions are held in a hash map, so
    /// any order this reported would be the map's, not the service's.
    pub fn active_pipeline_execution_count(
        &self,
        workspace_name: &str,
        pipeline_name: &str,
        now: SystemTime,
    ) -> usize {
        self.pipeline_executions
            .values()
            .filter(|execution| {
                execution.workspace_name == workspace_name
                    && execution.pipeline_name == pipeline_name
                    && execution.is_active(now)
            })
            .count()
    }

    /// The pipeline of `workspace_name` whose definition names `task_name` on a
    /// compute node, as `(pipelineName, computeNodeName)`, or `None` when no
    /// pipeline of that workspace references the task.
    ///
    /// A compute node "references a task" through its `taskName`, and the
    /// definition that counts is the pipeline's CURRENT version: that is the one
    /// a start of the pipeline runs its nodes from
    /// (`crate::ops::start_pipeline_execution`), so a version a later write
    /// superseded references nothing any more. The dependency a task carries as
    /// something a pipeline points at, which is what the operation that removes
    /// a task has to refuse on — the same question
    /// [`Self::hierarchy_parent`] answers for an asset.
    ///
    /// The pipelines are held in a hash map, so the smallest
    /// `(pipelineName, computeNodeName)` pair is reported rather than whichever
    /// the map yields first: one store state then always names one reference,
    /// whatever order the map iterates in.
    pub fn pipeline_referencing_task(
        &self,
        workspace_name: &str,
        task_name: &str,
    ) -> Option<(&str, &str)> {
        self.pipelines
            .values()
            .filter(|pipeline| pipeline.workspace_name == workspace_name)
            .flat_map(|pipeline| {
                pipeline
                    .current
                    .computations
                    .iter()
                    .filter(|node| node.task_name == task_name)
                    .map(|node| (pipeline.name.as_str(), node.compute_node_name.as_str()))
            })
            .min()
    }

    /// Forget a pipeline and everything keyed off it: every version it holds and
    /// its tags (both on the record) and the executions recorded against it — so
    /// no later read resolves to a pipeline this service no longer holds.
    pub fn forget_pipeline(
        &mut self,
        workspace_name: &str,
        pipeline_name: &str,
    ) -> Option<PipelineRecord> {
        let record = self
            .pipelines
            .remove(&pipeline_key(workspace_name, pipeline_name))?;
        let forgotten: HashSet<String> = self
            .pipeline_executions
            .values()
            .filter(|execution| {
                execution.workspace_name == workspace_name
                    && execution.pipeline_name == pipeline_name
            })
            .map(|execution| execution.id.clone())
            .collect();
        self.pipeline_executions
            .retain(|id, _| !forgotten.contains(id));
        Some(record)
    }

    /// Forget a gateway and everything keyed off it: its capability
    /// configurations and its tags (both on the record), and the
    /// [`Self::gateway_devices`] entry that reserved its platform device.
    ///
    /// The index entry goes with the record, so the device the gateway ran on is
    /// free for the next `CreateGateway`: one device hosts at most one gateway,
    /// and a gateway this service no longer holds occupies none. Every entry
    /// naming this gateway is dropped, rather than only the one
    /// [`gateway_device_key`] rebuilds from the stored platform, so no index
    /// entry can outlive the gateway it pointed at.
    pub fn forget_gateway(&mut self, gateway_id: &str) -> Option<GatewayRecord> {
        let record = self.gateways.remove(gateway_id)?;
        self.gateway_devices.retain(|_, id| id != gateway_id);
        Some(record)
    }

    /// Forget every access policy whose `Resource` the predicate names.
    ///
    /// A policy is ABOUT one Monitor resource — the portal or the project its
    /// `Resource` names — and both `CreateAccessPolicy` and
    /// `UpdateAccessPolicy` resolve that resource against this store before they
    /// write ([`Self::missing_monitor_resource`]), so a resource this service no
    /// longer holds can leave no policy behind for a read or a listing to
    /// report: no write of this crate could have produced such a record, and no
    /// caller could repair one, because the rewrite asks the same resolution and
    /// so refuses the only resource the stored policy names. The policy's tags
    /// live on its record, which owns them, so they go with it — the same reason
    /// [`Self::forget_asset`] drops what is keyed off an asset.
    ///
    /// The one place this is stated: a portal's delete and a project's delete
    /// both reach it, so the two cannot drift.
    pub(super) fn forget_access_policies_about(&mut self, about: impl Fn(&Resource) -> bool) {
        self.access_policies
            .retain(|_, policy| !about(&policy.resource));
    }

    /// Forget an IoT SiteWise Monitor project, the dashboards created in it and
    /// the access policies granted on it.
    ///
    /// A dashboard is created IN a project and names that project on every read —
    /// the create refuses a project this store does not hold — so one whose
    /// project is gone could never have been created. The project's tags go with
    /// its record, which owns them. The access policies that NAME the project go
    /// through [`Self::forget_access_policies_about`], for the reason stated
    /// there.
    ///
    /// The one place this containment rule is stated: `DeleteProject` removes one
    /// project through it and [`Self::forget_portal`] removes every project of a
    /// portal through it, so the two cannot drift.
    pub fn forget_project(&mut self, project_id: &str) -> Option<ProjectRecord> {
        let record = self.projects.remove(project_id)?;
        self.dashboards
            .retain(|_, dashboard| dashboard.project_id != project_id);
        self.forget_access_policies_about(|resource| {
            resource
                .project
                .as_ref()
                .is_some_and(|project| project.id == project_id)
        });
        Some(record)
    }

    /// Forget an IoT SiteWise Monitor portal and everything this service holds
    /// only as a part of or a reference to it: the projects created in it, the
    /// dashboards created in those projects, and the access policies granted on
    /// the portal or on one of those projects.
    ///
    /// A project is created IN a portal — the create refuses a portal this store
    /// does not hold — so a portal the service no longer holds can leave none of
    /// its projects addressable behind. Dropping them here is what keeps a later
    /// read from resolving to a project or a dashboard whose portal is gone, the
    /// same reason [`Self::forget_asset`] drops what is keyed off an asset. Each
    /// project goes through [`Self::forget_project`], so a portal's delete and a
    /// project's own delete take exactly the same things with them — the policies
    /// granted on those projects included; the ones granted on the PORTAL go
    /// through [`Self::forget_access_policies_about`] here.
    pub fn forget_portal(&mut self, portal_id: &str) -> Option<PortalRecord> {
        let record = self.portals.remove(portal_id)?;
        let projects: Vec<String> = self
            .projects
            .values()
            .filter(|project| project.portal_id == portal_id)
            .map(|project| project.id.clone())
            .collect();
        for project in projects {
            self.forget_project(&project);
        }
        self.forget_access_policies_about(|resource| {
            resource
                .portal
                .as_ref()
                .is_some_and(|portal| portal.id == portal_id)
        });
        Some(record)
    }

    /// The IoT SiteWise Monitor resource an access policy is about, resolved
    /// against this store: `Some((kind, requested))` when the alternative the
    /// request names holds no record here, `None` when it resolves.
    ///
    /// A `Resource` names a portal OR a project — [`check_access_policy_resource`]
    /// is what holds a request to exactly one of them — and BOTH are resources
    /// this service holds, so each alternative is resolved against the map that
    /// holds it. The returned kind is the [`RESOURCE_PORTAL`] /
    /// [`RESOURCE_PROJECT`] label a not-found refusal names the absent resource
    /// by, and the identifier is the one the REQUEST supplied, so the caller is
    /// told which half of the choice resolved to nothing.
    ///
    /// The one place this resolution is stated: `CreateAccessPolicy` and
    /// `UpdateAccessPolicy` both ask it, so the two cannot drift.
    pub fn missing_monitor_resource<'a>(
        &self,
        resource: &'a Resource,
    ) -> Option<(&'static str, &'a str)> {
        if let Some(portal) = resource.portal.as_ref() {
            if !self.portals.contains_key(&portal.id) {
                return Some((RESOURCE_PORTAL, portal.id.as_str()));
            }
        }
        if let Some(project) = resource.project.as_ref() {
            if !self.projects.contains_key(&project.id) {
                return Some((RESOURCE_PROJECT, project.id.as_str()));
            }
        }
        None
    }

    /// Forget an asset model, its external-id entry and every interface
    /// relationship it stood on either side of.
    ///
    /// A relationship names two models, so a model that is gone leaves no
    /// relationship behind: the entries are dropped here rather than left to
    /// consume the [`MAX_INTERFACE_RELATIONSHIPS`] budget for a pair nothing can
    /// address any more.
    pub fn forget_asset_model(&mut self, asset_model_id: &str) -> Option<AssetModelRecord> {
        let record = self.asset_models.remove(asset_model_id)?;
        if let Some(external) = record.external_id.as_deref() {
            self.asset_model_external_ids.remove(external);
        }
        self.interface_relationships
            .retain(|(applied_to, interface), _| {
                applied_to != asset_model_id && interface != asset_model_id
            });
        Some(record)
    }

    /// Forget a computation model and everything this store holds only as a
    /// record OF it: the actions taken on it and the executions those actions
    /// started.
    ///
    /// An action and an execution each NAME the target resource they were taken
    /// on, and a computation model is one of the two kinds a target resource can
    /// be, so a model this service no longer holds can leave none of them behind
    /// for a listing to report or for the [`MAX_ACTIONS`] budget to count — the
    /// same reason [`Self::forget_asset`] drops what is keyed off an asset. They
    /// are addressed through the one
    /// [`target_resource_addresses`] rule the listings resolve a target by, so
    /// what goes with the model is exactly what was reported for it. The model's
    /// tags live on its record, which owns them, so they go with it.
    pub fn forget_computation_model(
        &mut self,
        computation_model_id: &str,
    ) -> Option<ComputationModelRecord> {
        let record = self.computation_models.remove(computation_model_id)?;
        self.actions.retain(|_, action| {
            !action.targets(
                TARGET_RESOURCE_TYPE_COMPUTATION_MODEL,
                computation_model_id,
            )
        });
        self.executions.retain(|_, execution| {
            !execution.targets(
                TARGET_RESOURCE_TYPE_COMPUTATION_MODEL,
                computation_model_id,
            )
        });
        Some(record)
    }

    /// The resources `workspace_name` CONTAINS, as `(resource kind, how many)`
    /// pairs in one fixed order, listing only the kinds that hold at least one.
    ///
    /// These are the kinds a caller creates INSIDE a workspace and addresses as
    /// resources of their own — each with its own create and its own delete — so
    /// they are what a delete of the container has to find empty. The records
    /// this store keys by a workspace but a caller cannot CREATE one at a time
    /// (a search, a query, an enrichment job, a dataset export job, a capture
    /// source) are NOT contents in this sense: they are what running the
    /// workspace produced, and they go with it in [`Self::forget_workspace`] —
    /// and so is a bulk import job, which a caller does start one at a time but
    /// can never DELETE, so counting it here would refuse the container's delete
    /// for good. A
    /// pipeline execution goes with its own PIPELINE
    /// ([`Self::forget_pipeline`]), which is a content and so has to be gone
    /// before the workspace can be deleted at all.
    pub fn workspace_contents(&self, workspace_name: &str) -> Vec<(&'static str, usize)> {
        let datasets = self
            .datasets
            .values()
            .filter(|dataset| dataset.workspace_name.as_deref() == Some(workspace_name))
            .count();
        let pipelines = self
            .pipelines
            .keys()
            .filter(|(workspace, _)| workspace == workspace_name)
            .count();
        let tasks = self
            .tasks
            .keys()
            .filter(|(workspace, _)| workspace == workspace_name)
            .count();
        let applications = self
            .applications
            .values()
            .filter(|application| application.workspace_name == workspace_name)
            .count();
        [
            (RESOURCE_DATASET, datasets),
            (RESOURCE_PIPELINE, pipelines),
            (RESOURCE_TASK, tasks),
            (RESOURCE_APPLICATION, applications),
        ]
        .into_iter()
        .filter(|(_, held)| *held > 0)
        .collect()
    }

    /// Forget a workspace and everything keyed off it: its tags (on the record),
    /// the searches, asynchronous queries, enrichment jobs, dataset export jobs
    /// and bulk import jobs started in it, the capture sources it holds together
    /// with the alias index they resolve through, and the logging scope set for
    /// it.
    ///
    /// Each of those is addressable ONLY inside the workspace, so a workspace
    /// this service no longer holds can leave none of them behind for a later
    /// read — or for the per-workspace bounds those maps are capped by. The
    /// logging options of the workspace scope go too, so a workspace later
    /// created under the same name reports the default level rather than
    /// inheriting what was set for the one that is gone; the
    /// account-and-region-wide scope ([`ACCOUNT_LOGGING_SCOPE`]) is keyed under a
    /// name no workspace can carry and so survives.
    ///
    /// The resources a caller addresses one at a time are NOT removed here:
    /// [`Self::workspace_contents`] is what refuses a delete while any of them
    /// stands.
    pub fn forget_workspace(&mut self, workspace_name: &str) -> Option<WorkspaceRecord> {
        let record = self.workspaces.remove(workspace_name)?;
        self.searches
            .retain(|(workspace, _), _| workspace != workspace_name);
        self.queries
            .retain(|(workspace, _), _| workspace != workspace_name);
        self.enrichment_jobs
            .retain(|(workspace, _), _| workspace != workspace_name);
        self.dataset_export_jobs
            .retain(|(workspace, _), _| workspace != workspace_name);
        // A bulk import job names the workspace its destination session dataset
        // lives in, and nothing deletes a job on its own, so the jobs of this
        // workspace go with it however the records were removed: a job the store
        // kept would answer a read with a `workspaceName` this store no longer
        // holds. A job ingesting into time series OUTSIDE any workspace names
        // none and so belongs to no workspace's delete.
        self.bulk_import_jobs
            .retain(|_, job| job.workspace_name.as_deref() != Some(workspace_name));
        self.capture_sources.remove(workspace_name);
        self.capture_source_aliases.remove(workspace_name);
        self.logging_options.remove(workspace_name);
        Some(record)
    }

    /// Re-materialise every asset created from asset model `asset_model_id`
    /// against that model's CURRENT property and hierarchy definitions.
    ///
    /// This is the inheritance `UpdateAssetModel` documents: "Updates an asset
    /// model and all of the assets that were created from the model. Each asset
    /// created from the model inherits the updated asset model's property and
    /// hierarchy definitions", and "If you remove a property from an asset model,
    /// IoT SiteWise deletes all previous data for that property".
    ///
    /// So, per asset of that model: a property the model still defines keeps the
    /// asset-level bindings the model does not define (its alias and its
    /// notification state) and takes the definition's name, data type and unit; a
    /// property the model no longer defines goes, and with it its stored time
    /// series and its alias binding; a hierarchy the model no longer defines goes,
    /// and with it the child associations recorded under it. Each affected asset
    /// reports `UPDATING` until the transition this call owes falls due, exactly as
    /// [`AssetRecord::begin_update`] records it for a direct `UpdateAsset`.
    ///
    /// "Property definitions" is everything the model declares a property by, so
    /// the definitions each COMPOSITE model of it contributes are propagated on
    /// the same terms as its own: a composite model added to the model reaches
    /// the assets already created from it, and one removed from it takes its
    /// per-asset properties with it.
    ///
    /// A composite model that names a `COMPONENT_MODEL` contributes THAT model's
    /// definitions, read from its stored record rather than copied when the
    /// composite model was created (see
    /// [`Self::composite_model_properties`]) — so a change to a component model
    /// changes what an asset of every model REUSING it carries, and those assets
    /// inherit it on the same terms as the assets of the model that changed.
    /// Walked transitively, because a model reused here may itself be reused
    /// elsewhere, and each model is re-materialised at most once so a reuse graph
    /// that closes on itself still terminates.
    ///
    /// `asset_model_id` is a stored id, not a label: resolve it with
    /// [`Self::resolve_asset_model_id`] first. Does nothing when no stored model
    /// has that id, or when no asset was created from it.
    pub fn propagate_asset_model_definitions(&mut self, asset_model_id: &str, now: SystemTime) {
        let mut pending = vec![asset_model_id.to_string()];
        let mut propagated: HashSet<String> = HashSet::new();
        while let Some(model_id) = pending.pop() {
            if !propagated.insert(model_id.clone()) {
                continue;
            }
            self.propagate_definitions_to_own_assets(&model_id, now);
            pending.extend(self.asset_models_reusing(&model_id));
        }
    }

    /// Every stored composite model that REUSES asset model `asset_model_id` as
    /// its component model, as `(the asset model holding it, its
    /// assetModelCompositeModelId)` pairs: the composition relationships of that
    /// model, read from the holder's side, which is the one place a composite
    /// model's reuse link is recorded.
    ///
    /// ONE definition of "which composite models reuse this asset model", so the
    /// listing that reports those relationships, the propagation that follows
    /// them and the delete that has to refuse while one stands all answer from
    /// the same enumeration. A composite model carrying no id holds no link and
    /// so is in no relationship.
    ///
    /// Ordered the way the relationships are reported: the holding asset models
    /// oldest first, ties broken by id so two created in the same instant still
    /// order deterministically, and within one holder the stored order of its
    /// composite models.
    pub fn composite_models_reusing(&self, asset_model_id: &str) -> Vec<(String, String)> {
        let mut holders: Vec<&AssetModelRecord> = self.asset_models.values().collect();
        holders.sort_by(|left, right| {
            left.creation_date
                .cmp(&right.creation_date)
                .then_with(|| left.id.cmp(&right.id))
        });
        holders
            .into_iter()
            .flat_map(|holder| {
                holder.composite_models.iter().filter_map(move |composite| {
                    let composite_id = composite.id.as_deref()?;
                    (holder.composed_asset_model_id(composite_id) == Some(asset_model_id))
                        .then(|| (holder.id.clone(), composite_id.to_string()))
                })
            })
            .collect()
    }

    /// The stored asset models holding a composite model whose component model is
    /// `asset_model_id`, each named once: the holders of its composition
    /// relationships ([`Self::composite_models_reusing`]).
    pub(super) fn asset_models_reusing(&self, asset_model_id: &str) -> Vec<String> {
        let mut named: HashSet<String> = HashSet::new();
        self.composite_models_reusing(asset_model_id)
            .into_iter()
            .filter_map(|(holder, _)| named.insert(holder.clone()).then_some(holder))
            .collect()
    }

    /// `asset_model_id` together with every asset model whose assets carry a
    /// definition of it: the transitive closure of [`Self::asset_models_reusing`],
    /// which is exactly the set of models
    /// [`Self::propagate_asset_model_definitions`] re-materialises the assets of.
    ///
    /// Asked by the dependency question a removal from this model's definitions
    /// has to answer ([`Self::data_segment_of_dropped_model_properties`]): the
    /// streams such a removal drops are those of the assets that propagation
    /// reaches, not only of the assets created directly from the model. Each
    /// model is answered at most once, so a reuse graph that closes on itself
    /// still terminates.
    pub(super) fn models_inheriting_definitions_of(&self, asset_model_id: &str) -> HashSet<String> {
        let mut pending = vec![asset_model_id.to_string()];
        let mut inheriting: HashSet<String> = HashSet::new();
        while let Some(model_id) = pending.pop() {
            if !inheriting.insert(model_id.clone()) {
                continue;
            }
            pending.extend(self.asset_models_reusing(&model_id));
        }
        inheriting
    }

    /// Re-materialise the assets created from asset model `asset_model_id` ONLY,
    /// which is one step of [`Self::propagate_asset_model_definitions`]: the
    /// models reusing this one are walked there.
    pub(super) fn propagate_definitions_to_own_assets(&mut self, asset_model_id: &str, now: SystemTime) {
        let Some(model) = self.asset_models.get(asset_model_id) else {
            return;
        };
        // Materialised ONCE, through the shared rule, and owned, so the per-asset
        // writes below are not borrowing the model: what an asset of this model
        // carries does not depend on which asset it is. The value each property
        // STARTS with is taken from the same place, indexed by property id so
        // the per-property lookups below cost nothing: an asset carries one
        // property per definition of its model and of every composite model of
        // it, so a linear scan per property would be quadratic in that count.
        let materialised = self.materialise_asset_properties(model, now);
        let initial_values = materialised.initial_values_by_id();
        let hierarchy_definitions = model.hierarchies.clone();
        let asset_ids: Vec<String> = self
            .assets
            .values()
            .filter(|asset| asset.asset_model_id == asset_model_id)
            .map(|asset| asset.id.clone())
            .collect();

        for asset_id in asset_ids {
            let Some(asset) = self.assets.get_mut(&asset_id) else {
                continue;
            };
            let carried = ::std::mem::take(&mut asset.properties);
            // Where each property this asset already carried sits, so the pass
            // below is one constant-time lookup per property rather than a scan.
            // The FIRST occurrence of an id wins, because that is the one
            // [`AssetRecord::property_position`] resolves and so the one every
            // write of an alias or a notification landed on: an id two composite
            // models reusing a component model contribute twice names one
            // definition, one series and one binding.
            let mut held_at: HashMap<&str, usize> = HashMap::new();
            for (at, property) in carried.iter().enumerate() {
                held_at.entry(property.id.as_str()).or_insert(at);
            }
            // What a property this asset did not carry STARTS with: it is
            // materialised HERE for the first time, so it takes the sample its
            // definition declares exactly as a property materialised by a create
            // does (see [`initial_property_sample`]). A property the asset
            // already carried keeps the series written to it.
            let mut properties = materialised.properties.clone();
            let mut seeded: Vec<(String, AssetPropertyValue)> = Vec::new();
            let mut seeded_ids: HashSet<String> = HashSet::new();
            for property in &mut properties {
                let held = held_at
                    .get(property.id.as_str())
                    .and_then(|at| carried.get(*at));
                if held.is_none() && seeded_ids.insert(property.id.clone()) {
                    if let Some(sample) = initial_values.get(property.id.as_str()) {
                        seeded.push((property.id.clone(), (*sample).clone()));
                    }
                }
                // An alias and a notification are not inherited from a
                // definition, so a property the asset already carried keeps the
                // ones set on it by `UpdateAssetProperty`.
                property.alias = held.and_then(|held| held.alias.clone());
                property.notification = held.and_then(|held| held.notification.clone());
            }
            let kept: HashSet<String> = properties
                .iter()
                .map(|property| property.id.clone())
                .collect();
            let hierarchies: Vec<AssetHierarchy> = hierarchy_definitions
                .iter()
                .map(|hierarchy| AssetHierarchy {
                    id: hierarchy.id.clone(),
                    external_id: hierarchy.external_id.clone(),
                    name: hierarchy.name.clone(),
                })
                .collect();
            let dropped_hierarchies: Vec<String> = asset
                .hierarchies
                .iter()
                .filter_map(|hierarchy| hierarchy.id.clone())
                .filter(|id| {
                    !hierarchies
                        .iter()
                        .any(|kept| kept.id.as_deref() == Some(id.as_str()))
                })
                .collect();
            let dropped_properties: Vec<AssetProperty> = carried
                .into_iter()
                .filter(|property| !kept.contains(&property.id))
                .collect();
            asset.properties = properties;
            asset.hierarchies = hierarchies;
            asset.begin_update(now);

            for property in dropped_properties {
                if let Some(series) = self.property_values.get_mut(&asset_id) {
                    series.remove(&property.id);
                    if series.is_empty() {
                        self.property_values.remove(&asset_id);
                    }
                }
                if let Some(alias) = property.alias {
                    let ours = self
                        .property_alias_index
                        .get(&alias)
                        .is_some_and(|(bound_asset, bound_property)| {
                            *bound_asset == asset_id && *bound_property == property.id
                        });
                    if ours {
                        self.property_alias_index.remove(&alias);
                    }
                }
            }
            for hierarchy_id in dropped_hierarchies {
                self.asset_hierarchy_children
                    .remove(&(asset_id.clone(), hierarchy_id.clone()));
            }
            for (property_id, value) in seeded {
                self.record_property_value(&asset_id, &property_id, value);
            }
        }
    }

    /// The child assets associated with `(asset_id, hierarchy_id)`.
    pub fn hierarchy_children(&self, asset_id: &str, hierarchy_id: &str) -> &[String] {
        self.asset_hierarchy_children
            .get(&(asset_id.to_string(), hierarchy_id.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether any child asset is associated under ANY hierarchy of `asset_id`.
    ///
    /// The dependency an asset carries as a PARENT: what makes it a resource other
    /// resources still point at, which is what the operations that remove an asset
    /// have to refuse on.
    pub fn has_hierarchy_children(&self, asset_id: &str) -> bool {
        self.asset_hierarchy_children
            .iter()
            .any(|((asset, _), children)| asset == asset_id && !children.is_empty())
    }

    /// The association `child_asset_id` is a child through, as
    /// `(parentAssetId, hierarchyId)`, or `None` when the asset is nobody's child.
    ///
    /// The inverse of [`Self::hierarchy_children`], and single-valued because
    /// `AssociateAssets` refuses an association for a child that already has a
    /// parent: the asset hierarchies form a TREE, so an asset appears as a child at
    /// most once in [`Self::asset_hierarchy_children`]. It lives here, beside the
    /// index it reads, because it is the one step every upward walk of that tree
    /// takes — `AssociateAssets`'s cycle check, `ListAssets`'s `TOP_LEVEL` filter,
    /// `ListAssetRelationships`'s `PATH_TO_ROOT` traversal and `DeleteAsset`'s
    /// refusal to remove a still-associated asset all take it.
    pub fn hierarchy_parent(&self, child_asset_id: &str) -> Option<(&str, &str)> {
        self.asset_hierarchy_children
            .iter()
            .find(|(_, children)| children.iter().any(|child| child == child_asset_id))
            .map(|((parent, hierarchy), _)| (parent.as_str(), hierarchy.as_str()))
    }

    /// Record `child_id` as a child of `asset_id` under `hierarchy_id`, after the
    /// children already associated there.
    ///
    /// The list stays a set: a child already associated under that hierarchy is not
    /// added twice. `AssociateAssets` refuses such a request before reaching here,
    /// so this keeps the stored invariant for any caller whose own rule allows it.
    pub fn associate_child(&mut self, asset_id: &str, hierarchy_id: &str, child_id: &str) {
        let children = self
            .asset_hierarchy_children
            .entry((asset_id.to_string(), hierarchy_id.to_string()))
            .or_default();
        if !children.iter().any(|child| child == child_id) {
            children.push(child_id.to_string());
        }
    }

    /// Resolve a `(parent, hierarchyId, childAssetId)` triple of `CustomID` labels
    /// to the stored ids it names.
    ///
    /// Every label takes either form the model admits (a raw id or
    /// `externalId:<externalId>`). The hierarchy is resolved against the PARENT
    /// ASSET'S MODEL — the same place `AssociateAssets` resolves it, since the
    /// model is what defines which groupings exist — so the operation that records
    /// an association and the one that removes it address exactly one set of
    /// hierarchies, even once a model's definition has changed since the asset was
    /// created. The asset's own copied hierarchy list is the fallback for a parent
    /// whose model is no longer stored.
    pub fn resolve_hierarchy_triple(
        &self,
        asset_label: &str,
        hierarchy_label: &str,
        child_label: &str,
    ) -> Result<HierarchyTriple, TripleRefusal> {
        let asset_id = self
            .resolve_asset_id(asset_label)
            .ok_or_else(|| TripleRefusal::AssetNotFound(asset_label.to_string()))?;
        let asset = self
            .assets
            .get(&asset_id)
            .ok_or_else(|| TripleRefusal::AssetNotFound(asset_label.to_string()))?;
        let hierarchy_id = self
            .asset_models
            .get(&asset.asset_model_id)
            .and_then(|model| {
                model.hierarchies.iter().find(|hierarchy| {
                    matches_id_label(
                        hierarchy.id.as_deref(),
                        hierarchy.external_id.as_deref(),
                        hierarchy_label,
                    )
                })
            })
            .and_then(|hierarchy| hierarchy.id.clone())
            .or_else(|| {
                asset
                    .hierarchy(hierarchy_label)
                    .and_then(|hierarchy| hierarchy.id.clone())
            })
            .ok_or_else(|| TripleRefusal::HierarchyNotFound(hierarchy_label.to_string()))?;
        let child_asset_id = self
            .resolve_asset_id(child_label)
            .ok_or_else(|| TripleRefusal::AssetNotFound(child_label.to_string()))?;
        Ok(HierarchyTriple {
            asset_id,
            hierarchy_id,
            child_asset_id,
        })
    }

    /// Remove `child_id` from `(asset_id, hierarchy_id)`, answering whether an
    /// association was there to remove. `false` means the triple was never
    /// associated, which is the caller's cue to refuse rather than to report a
    /// removal that did not happen.
    ///
    /// The write side of the association set lives in `AssociateAssets`, which
    /// owns the hierarchy rules it has to enforce before recording one; this is
    /// the removal `DisassociateAssets` performs.
    pub fn disassociate_child(
        &mut self,
        asset_id: &str,
        hierarchy_id: &str,
        child_id: &str,
    ) -> bool {
        let key = (asset_id.to_string(), hierarchy_id.to_string());
        let Some(children) = self.asset_hierarchy_children.get_mut(&key) else {
            return false;
        };
        let Some(at) = children.iter().position(|child| child == child_id) else {
            return false;
        };
        children.remove(at);
        // An emptied hierarchy keeps no entry, so the map holds only live
        // associations and cannot grow with keys nothing reads.
        if children.is_empty() {
            self.asset_hierarchy_children.remove(&key);
        }
        true
    }
}
