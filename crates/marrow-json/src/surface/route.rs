use serde::Serialize;

use super::{
    SURFACE_OPERATION_PROFILE_VERSION, SurfaceAbiJson, SurfaceReadOperationKindJson,
    SurfaceUpdateOperationKindJson,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
        let mut routes = Vec::new();
        for surface in &abi.surfaces {
            let route_surface = SurfaceRouteSurfaceJson {
                module: surface.module.clone(),
                name: surface.name.clone(),
            };
            routes.extend(surface.read.iter().map(|read| SurfaceRouteJson {
                method: SurfaceRouteMethodJson::Post,
                path: operation_path("read", &read.operation_tag),
                surface: route_surface.clone(),
                alias: read.alias.clone(),
                operation_tag: read.operation_tag.clone(),
                request: read_request(&read.kind),
            }));
            if let Some(update) = &surface.update {
                routes.push(SurfaceRouteJson {
                    method: SurfaceRouteMethodJson::Post,
                    path: operation_path("update", &update.operation_tag),
                    surface: route_surface.clone(),
                    alias: "update".into(),
                    operation_tag: update.operation_tag.clone(),
                    request: update_request(&update.kind),
                });
            }
            routes.extend(surface.actions.iter().map(|action| SurfaceRouteJson {
                method: SurfaceRouteMethodJson::Post,
                path: operation_path("action", &action.operation_tag),
                surface: route_surface.clone(),
                alias: action.alias.clone(),
                operation_tag: action.operation_tag.clone(),
                request: SurfaceRouteRequestJson::Action,
            }));
        }
        Self {
            profile_version: SURFACE_ROUTE_PROFILE_VERSION.into(),
            operation_profile_version: SURFACE_OPERATION_PROFILE_VERSION.into(),
            routes,
        }
    }
}

fn operation_path(kind: &str, operation_tag: &str) -> String {
    format!("/surface/v1/{kind}/{operation_tag}")
}

fn read_request(kind: &SurfaceReadOperationKindJson) -> SurfaceRouteRequestJson {
    match kind {
        SurfaceReadOperationKindJson::SingletonRead => SurfaceRouteRequestJson::SingletonRead,
        SurfaceReadOperationKindJson::PointRead => SurfaceRouteRequestJson::PointRead,
        SurfaceReadOperationKindJson::PagedRootCollection
        | SurfaceReadOperationKindJson::PagedIndexCollection { .. } => {
            SurfaceRouteRequestJson::Page
        }
        SurfaceReadOperationKindJson::UniqueIndexLookup { .. } => {
            SurfaceRouteRequestJson::UniqueLookup
        }
    }
}

fn update_request(kind: &SurfaceUpdateOperationKindJson) -> SurfaceRouteRequestJson {
    match kind {
        SurfaceUpdateOperationKindJson::SingletonUpdate => SurfaceRouteRequestJson::SingletonUpdate,
        SurfaceUpdateOperationKindJson::PointUpdate => SurfaceRouteRequestJson::PointUpdate,
    }
}
