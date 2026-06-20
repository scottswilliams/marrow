use serde::Serialize;

use super::{
    SURFACE_OPERATION_PROFILE_VERSION, SurfaceAbiJson, SurfaceOperationCatalog,
    operation_catalog::SurfaceOperationBinding,
};

pub const SURFACE_ROUTE_PROFILE_VERSION: &str = "surface.route.v1";

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
    UniqueLookup,
    SingletonUpdate,
    PointUpdate,
    Action,
}

impl SurfaceRouteManifestJson {
    pub fn from_abi(abi: &SurfaceAbiJson) -> Self {
        let catalog = SurfaceOperationCatalog::from_abi(abi)
            .expect("surface route manifest requires unique ABI operation tags");
        let mut routes = Vec::new();
        for surface in &abi.surfaces {
            let route_surface = SurfaceRouteSurfaceJson {
                module: surface.module.clone(),
                name: surface.name.clone(),
            };
            routes.extend(surface.read.iter().map(|read| {
                let binding = catalog
                    .binding(&read.operation_tag)
                    .expect("read descriptor has catalog binding");
                route_from_binding(binding, route_surface.clone())
            }));
            if let Some(update) = &surface.update {
                let binding = catalog
                    .binding(&update.operation_tag)
                    .expect("update descriptor has catalog binding");
                routes.push(route_from_binding(binding, route_surface.clone()));
            }
            routes.extend(surface.actions.iter().map(|action| {
                let binding = catalog
                    .binding(&action.operation_tag)
                    .expect("action descriptor has catalog binding");
                route_from_binding(binding, route_surface.clone())
            }));
        }
        Self {
            profile_version: SURFACE_ROUTE_PROFILE_VERSION.into(),
            operation_profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
            routes,
        }
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
