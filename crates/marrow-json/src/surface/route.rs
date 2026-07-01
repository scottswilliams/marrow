use serde::Serialize;

use super::{
    SurfaceAbiJson, SurfaceOperationCatalog, SurfaceOperationProfile,
    operation_catalog::SurfaceOperationBinding,
};

pub const SURFACE_ROUTE_PROFILE_VERSION: &str = "surface.route.v1";
pub const SURFACE_ROUTE_PROFILE_VERSION_V2: &str = "surface.route.v2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceRouteManifestJson {
    pub profile_version: String,
    pub operation_profile_version: String,
    pub routes: Vec<SurfaceRouteJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceRouteJson {
    pub method: SurfaceRouteMethodJson,
    pub path: String,
    pub surface: SurfaceRouteSurfaceJson,
    pub alias: String,
    pub operation_tag: String,
    pub request: SurfaceRouteRequestJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SurfaceRouteMethodJson {
    #[serde(rename = "POST")]
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceRouteSurfaceJson {
    pub module: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceRouteRequestJson {
    SingletonRead,
    PointRead,
    Page,
    RangePage,
    UniqueLookup,
    SingletonUpdate,
    PointUpdate,
    SingletonCreate,
    PointCreate,
    SingletonDelete,
    PointDelete,
    Action,
    ComputedRead,
}

impl SurfaceRouteManifestJson {
    pub fn from_abi(abi: &SurfaceAbiJson) -> Self {
        Self::from_abi_for_profile(abi, SurfaceOperationProfile::V1)
    }

    pub fn from_abi_v2(abi: &SurfaceAbiJson) -> Self {
        Self::from_abi_for_profile(abi, SurfaceOperationProfile::V2)
    }

    pub fn from_abi_for_profile(abi: &SurfaceAbiJson, profile: SurfaceOperationProfile) -> Self {
        let catalog = SurfaceOperationCatalog::from_abi_for_profile(abi, profile)
            .expect("surface route manifest requires unique ABI operation tags");
        let mut routes = Vec::new();
        for surface in &abi.surfaces {
            let route_surface = SurfaceRouteSurfaceJson {
                module: surface.module.clone(),
                name: surface.name.clone(),
            };
            for read in &surface.read {
                push_route_for_tag(&mut routes, &catalog, &route_surface, &read.operation_tag);
            }
            if let Some(create) = &surface.create {
                push_route_for_tag(&mut routes, &catalog, &route_surface, &create.operation_tag);
            }
            if let Some(update) = &surface.update {
                push_route_for_tag(&mut routes, &catalog, &route_surface, &update.operation_tag);
            }
            if let Some(delete) = &surface.delete {
                push_route_for_tag(&mut routes, &catalog, &route_surface, &delete.operation_tag);
            }
            for action in &surface.actions {
                push_route_for_tag(&mut routes, &catalog, &route_surface, &action.operation_tag);
            }
            for computed_read in &surface.computed_reads {
                push_route_for_tag(
                    &mut routes,
                    &catalog,
                    &route_surface,
                    &computed_read.operation_tag,
                );
            }
        }
        Self {
            profile_version: route_profile_version(profile).into(),
            operation_profile_version: profile.version().into(),
            routes,
        }
    }
}

fn push_route_for_tag(
    routes: &mut Vec<SurfaceRouteJson>,
    catalog: &SurfaceOperationCatalog,
    surface: &SurfaceRouteSurfaceJson,
    operation_tag: &str,
) {
    if let Some(binding) = catalog.binding(operation_tag) {
        routes.push(route_from_binding(binding, surface.clone()));
    }
}

pub(crate) fn route_profile_version(profile: SurfaceOperationProfile) -> &'static str {
    match profile {
        SurfaceOperationProfile::V1 => SURFACE_ROUTE_PROFILE_VERSION,
        SurfaceOperationProfile::V2 => SURFACE_ROUTE_PROFILE_VERSION_V2,
    }
}

fn route_from_binding(
    binding: &SurfaceOperationBinding,
    surface: SurfaceRouteSurfaceJson,
) -> SurfaceRouteJson {
    SurfaceRouteJson {
        method: SurfaceRouteMethodJson::Post,
        path: binding.path.clone(),
        surface,
        alias: binding.alias.clone(),
        operation_tag: binding.operation_tag.clone(),
        request: binding.kind.route_request(),
    }
}
