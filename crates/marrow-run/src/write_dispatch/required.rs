use marrow_check::{CheckedSavedMember, CheckedSavedPlace};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::RuntimeError;
use crate::store::{DataAddress, LayerAddress, data_exists};
use crate::write::{ResourceValue, materialized_plain_fields};

pub(super) fn created_required_field_path(
    place: &CheckedSavedPlace,
    identity: &[marrow_store::key::SavedKey],
    layers: &[LayerAddress],
    members: &[CheckedSavedMember],
    field: &str,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Option<DataAddress>, RuntimeError> {
    if env.transaction_depth() == 0 || !checked_field_required(members, field).unwrap_or(false) {
        return Ok(None);
    }
    let address = DataAddress::member_path(place, identity, layers, &[field.to_string()], span)?;
    let absent = !data_exists(env.store, &address, span)?;
    Ok(absent.then_some(address))
}

pub(super) fn checked_field_required(members: &[CheckedSavedMember], field: &str) -> Option<bool> {
    members
        .iter()
        .find(|member| member.name == field)
        .and_then(|member| member.plain_field().map(|(_, required)| required))
}

pub(crate) fn created_required_paths_for_value(
    place: &CheckedSavedPlace,
    identity: &[marrow_store::key::SavedKey],
    layers: &[LayerAddress],
    members: &[CheckedSavedMember],
    value: &ResourceValue,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<DataAddress>, RuntimeError> {
    if env.transaction_depth() == 0 {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for field in materialized_plain_fields(members) {
        if !field.required || !resource_value_supplies(value, &field.path) {
            continue;
        }
        let address = DataAddress::member_path(place, identity, layers, &field.path, span)?;
        if !data_exists(env.store, &address, span)? {
            paths.push(address);
        }
    }
    Ok(paths)
}

fn resource_value_supplies(value: &ResourceValue, field: &[String]) -> bool {
    let name = field.join(".");
    value.fields.iter().any(|(field, _)| field == &name)
        || value
            .identities
            .iter()
            .any(|identity| identity.field == name)
}

pub(super) fn required_delete_has_preexisting_data(
    paths: &[DataAddress],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<bool, RuntimeError> {
    for path in paths {
        if env.required_path_created_in_transaction(path) {
            continue;
        }
        if data_exists(env.store, path, span)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn required_paths_under_group(
    place: &CheckedSavedPlace,
    identity: &[marrow_store::key::SavedKey],
    layers: &[LayerAddress],
    group_name: &str,
    group: &CheckedSavedMember,
    span: SourceSpan,
) -> Result<Vec<DataAddress>, RuntimeError> {
    materialized_plain_fields(&group.group_members)
        .into_iter()
        .filter(|field| field.required)
        .map(|field| {
            let mut path = vec![group_name.to_string()];
            path.extend(field.path);
            DataAddress::member_path(place, identity, layers, &path, span)
        })
        .collect()
}

pub(super) fn checked_unkeyed_group<'a>(
    members: &'a [CheckedSavedMember],
    field: &str,
) -> Option<&'a CheckedSavedMember> {
    members
        .iter()
        .find(|member| member.name == field && member.is_unkeyed_group())
}

pub(super) fn checked_member_exists(members: &[CheckedSavedMember], field: &str) -> bool {
    members.iter().any(|member| member.name == field)
}

pub(super) fn checked_group_has_required_materialized_field(group: &CheckedSavedMember) -> bool {
    materialized_plain_fields(&group.group_members)
        .into_iter()
        .any(|field| field.required)
}
