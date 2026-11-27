// SPDX-License-Identifier: CC0-1.0

//! The FRONT DOOR this service's operations are served behind: which operations the
//! body-validating layer ahead of the handlers fronts, and the one it does not.
//!
//! The rule that layer applies, the violations it reads and the answer it states are the
//! shared ones ([`::service_common::front_door`]); this module holds only the per-service
//! half — the table of operations served behind it, built from the one shared engine. See
//! [`crate::service`] for what each half of the rule is and what the recordings show.

use ::service_common::front_door::FrontDoorTable;
use ::smithy_server_runtime::OperationRoutingTable;
use ::std::sync::Arc;

/// Every modelled operation the body-validating layer fronts, each wrapped in the shared
/// front door and keyed by its own modelled name (which [`FrontDoorTable`] reads off the
/// operation), so the rule is applied by ONE code path rather than per operation.
///
/// `CreateCase` is absent on purpose: the recordings show its body refused in the modelled
/// per-member wording rather than with the door's fixed sentence, so it is left to the
/// generated routing table (see [`crate::service`]). Every other operation is here,
/// including the ones that bind nothing to the body — such an operation simply never has a
/// body violation to find, which the door reads off its input schema.
pub fn table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let mut front_door = FrontDoorTable::new();
    front_door
        .serve(crate::ops::batch_create_related_item::BatchCreateRelatedItem::new(Arc::clone(
            engine,
        )))
        .serve(crate::ops::batch_get_case_rule::BatchGetCaseRule::new(Arc::clone(engine)))
        .serve(crate::ops::batch_get_field::BatchGetField::new(Arc::clone(engine)))
        .serve(crate::ops::batch_put_field_options::BatchPutFieldOptions::new(Arc::clone(
            engine,
        )))
        .serve(crate::ops::batch_update_case::BatchUpdateCase::new(Arc::clone(engine)))
        .serve(crate::ops::create_case_rule::CreateCaseRule::new(Arc::clone(engine)))
        .serve(crate::ops::create_domain::CreateDomain::new(Arc::clone(engine)))
        .serve(crate::ops::create_field::CreateField::new(Arc::clone(engine)))
        .serve(crate::ops::create_layout::CreateLayout::new(Arc::clone(engine)))
        .serve(crate::ops::create_related_item::CreateRelatedItem::new(Arc::clone(engine)))
        .serve(crate::ops::create_template::CreateTemplate::new(Arc::clone(engine)))
        .serve(crate::ops::delete_case::DeleteCase::new(Arc::clone(engine)))
        .serve(crate::ops::delete_case_rule::DeleteCaseRule::new(Arc::clone(engine)))
        .serve(crate::ops::delete_domain::DeleteDomain::new(Arc::clone(engine)))
        .serve(crate::ops::delete_field::DeleteField::new(Arc::clone(engine)))
        .serve(crate::ops::delete_layout::DeleteLayout::new(Arc::clone(engine)))
        .serve(crate::ops::delete_related_item::DeleteRelatedItem::new(Arc::clone(engine)))
        .serve(crate::ops::delete_template::DeleteTemplate::new(Arc::clone(engine)))
        .serve(crate::ops::get_case::GetCase::new(Arc::clone(engine)))
        .serve(crate::ops::get_case_audit_events::GetCaseAuditEvents::new(Arc::clone(engine)))
        .serve(crate::ops::get_case_event_configuration::GetCaseEventConfiguration::new(
            Arc::clone(engine),
        ))
        .serve(crate::ops::get_domain::GetDomain::new(Arc::clone(engine)))
        .serve(crate::ops::get_layout::GetLayout::new(Arc::clone(engine)))
        .serve(crate::ops::get_merge_cases_status::GetMergeCasesStatus::new(Arc::clone(engine)))
        .serve(crate::ops::get_template::GetTemplate::new(Arc::clone(engine)))
        .serve(crate::ops::list_case_rules::ListCaseRules::new(Arc::clone(engine)))
        .serve(crate::ops::list_cases_for_contact::ListCasesForContact::new(Arc::clone(engine)))
        .serve(crate::ops::list_domains::ListDomains::new(Arc::clone(engine)))
        .serve(crate::ops::list_field_options::ListFieldOptions::new(Arc::clone(engine)))
        .serve(crate::ops::list_fields::ListFields::new(Arc::clone(engine)))
        .serve(crate::ops::list_layouts::ListLayouts::new(Arc::clone(engine)))
        .serve(crate::ops::list_tags_for_resource::ListTagsForResource::new(Arc::clone(engine)))
        .serve(crate::ops::list_templates::ListTemplates::new(Arc::clone(engine)))
        .serve(crate::ops::put_case_event_configuration::PutCaseEventConfiguration::new(
            Arc::clone(engine),
        ))
        .serve(crate::ops::search_all_related_items::SearchAllRelatedItems::new(Arc::clone(
            engine,
        )))
        .serve(crate::ops::search_cases::SearchCases::new(Arc::clone(engine)))
        .serve(crate::ops::search_related_items::SearchRelatedItems::new(Arc::clone(engine)))
        .serve(crate::ops::start_merge_cases::StartMergeCases::new(Arc::clone(engine)))
        .serve(crate::ops::tag_resource::TagResource::new(Arc::clone(engine)))
        .serve(crate::ops::untag_resource::UntagResource::new(Arc::clone(engine)))
        .serve(crate::ops::update_case::UpdateCase::new(Arc::clone(engine)))
        .serve(crate::ops::update_case_rule::UpdateCaseRule::new(Arc::clone(engine)))
        .serve(crate::ops::update_field::UpdateField::new(Arc::clone(engine)))
        .serve(crate::ops::update_layout::UpdateLayout::new(Arc::clone(engine)))
        .serve(crate::ops::update_related_item::UpdateRelatedItem::new(Arc::clone(engine)))
        .serve(crate::ops::update_template::UpdateTemplate::new(Arc::clone(engine)));
    front_door.finish()
}
