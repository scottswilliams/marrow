use serde::Serialize;

use super::{
    SURFACE_OPERATION_PROFILE_VERSION, SurfaceAbiJson, SurfaceOperationRequestBodyJson,
    SurfaceReadOperationKindJson, SurfaceUpdateOperationKindJson,
};

pub const SURFACE_ROUTE_PROFILE_VERSION: &str = "surface.route.v1";
const SURFACE_READ_ROUTE_PREFIX: &str = "/surface/v1/read/";
const SURFACE_UPDATE_ROUTE_PREFIX: &str = "/surface/v1/update/";
const SURFACE_ACTION_ROUTE_PREFIX: &str = "/surface/v1/action/";

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

impl SurfaceRouteRequestJson {
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            Self::SingletonRead | Self::PointRead | Self::Page | Self::UniqueLookup
        )
    }

    fn path_prefix(&self) -> &'static str {
        match self {
            Self::SingletonRead | Self::PointRead | Self::Page | Self::UniqueLookup => {
                SURFACE_READ_ROUTE_PREFIX
            }
            Self::SingletonUpdate | Self::PointUpdate => SURFACE_UPDATE_ROUTE_PREFIX,
            Self::Action => SURFACE_ACTION_ROUTE_PREFIX,
        }
    }

    pub fn matches_operation_body(&self, body: &SurfaceOperationRequestBodyJson) -> bool {
        matches!(
            (self, body),
            (
                Self::SingletonRead,
                SurfaceOperationRequestBodyJson::SingletonRead
            ) | (
                Self::PointRead,
                SurfaceOperationRequestBodyJson::PointRead { .. }
            ) | (Self::Page, SurfaceOperationRequestBodyJson::Page { .. })
                | (
                    Self::UniqueLookup,
                    SurfaceOperationRequestBodyJson::UniqueLookup { .. }
                )
                | (
                    Self::SingletonUpdate,
                    SurfaceOperationRequestBodyJson::SingletonUpdate { .. }
                )
                | (
                    Self::PointUpdate,
                    SurfaceOperationRequestBodyJson::PointUpdate { .. }
                )
                | (Self::Action, SurfaceOperationRequestBodyJson::Action { .. })
        )
    }
}

impl SurfaceRouteManifestJson {
    pub fn from_abi(abi: &SurfaceAbiJson) -> Self {
        let mut routes = Vec::new();
        for surface in &abi.surfaces {
            let route_surface = SurfaceRouteSurfaceJson {
                module: surface.module.clone(),
                name: surface.name.clone(),
            };
            routes.extend(surface.read.iter().map(|read| {
                let request = read_request(&read.kind);
                SurfaceRouteJson {
                    method: SurfaceRouteMethodJson::Post,
                    path: operation_path(request.path_prefix(), &read.operation_tag),
                    surface: route_surface.clone(),
                    alias: read.alias.clone(),
                    operation_tag: read.operation_tag.clone(),
                    request,
                }
            }));
            if let Some(update) = &surface.update {
                let request = update_request(&update.kind);
                routes.push(SurfaceRouteJson {
                    method: SurfaceRouteMethodJson::Post,
                    path: operation_path(request.path_prefix(), &update.operation_tag),
                    surface: route_surface.clone(),
                    alias: "update".into(),
                    operation_tag: update.operation_tag.clone(),
                    request,
                });
            }
            routes.extend(surface.actions.iter().map(|action| SurfaceRouteJson {
                method: SurfaceRouteMethodJson::Post,
                path: operation_path(
                    SurfaceRouteRequestJson::Action.path_prefix(),
                    &action.operation_tag,
                ),
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

fn operation_path(prefix: &str, operation_tag: &str) -> String {
    format!("{prefix}{operation_tag}")
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
