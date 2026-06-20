use std::collections::{BTreeMap, BTreeSet};

use super::{
    SURFACE_OPERATION_PROFILE_VERSION, SURFACE_ROUTE_PROFILE_VERSION, SurfaceOperationCatalog,
    SurfaceOperationKind, SurfaceRouteManifestJson, SurfaceRouteMethodJson,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceRouteBinding {
    pub path: String,
    pub operation_tag: String,
    pub kind: SurfaceOperationKind,
    pub surface_module: String,
    pub surface_name: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceRouteBindings {
    routes: Vec<SurfaceRouteBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceRouteBindingError {
    kind: SurfaceRouteBindingErrorKind,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRouteBindingErrorKind {
    InactiveRouteProfile,
    InactiveOperationProfile,
    NonPostMethod,
    DuplicatePath,
    DuplicateOperationTag,
    UnknownOperationTag,
    RequestKindMismatch,
    PathMismatch,
    SurfaceModuleMismatch,
    SurfaceNameMismatch,
    AliasMismatch,
}

impl SurfaceRouteBindings {
    pub fn from_manifest(
        manifest: &SurfaceRouteManifestJson,
        catalog: &SurfaceOperationCatalog,
    ) -> Result<Self, SurfaceRouteBindingError> {
        Ok(Self {
            routes: validate_routes(manifest, catalog)?,
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &SurfaceRouteBinding> {
        self.routes.iter()
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl SurfaceRouteBindingError {
    pub fn kind(&self) -> SurfaceRouteBindingErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for SurfaceRouteBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SurfaceRouteBindingError {}

fn validate_routes(
    manifest: &SurfaceRouteManifestJson,
    catalog: &SurfaceOperationCatalog,
) -> Result<Vec<SurfaceRouteBinding>, SurfaceRouteBindingError> {
    if manifest.profile_version != SURFACE_ROUTE_PROFILE_VERSION {
        return Err(error(
            SurfaceRouteBindingErrorKind::InactiveRouteProfile,
            "surface route profile version is not active",
        ));
    }
    if manifest.operation_profile_version != SURFACE_OPERATION_PROFILE_VERSION {
        return Err(error(
            SurfaceRouteBindingErrorKind::InactiveOperationProfile,
            "surface route operation profile version is not active",
        ));
    }

    let mut paths = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut by_path = BTreeMap::new();
    for route in &manifest.routes {
        if route.method != SurfaceRouteMethodJson::Post {
            return Err(error(
                SurfaceRouteBindingErrorKind::NonPostMethod,
                "surface route method must be POST",
            ));
        }
        if !paths.insert(route.path.as_str()) {
            return Err(error(
                SurfaceRouteBindingErrorKind::DuplicatePath,
                format!("duplicate route path `{}`", route.path),
            ));
        }
        if !tags.insert(route.operation_tag.as_str()) {
            return Err(error(
                SurfaceRouteBindingErrorKind::DuplicateOperationTag,
                format!("duplicate route operation tag `{}`", route.operation_tag),
            ));
        }
        let Some(expected) = catalog.binding(&route.operation_tag) else {
            return Err(error(
                SurfaceRouteBindingErrorKind::UnknownOperationTag,
                format!(
                    "route operation tag `{}` is not present in the ABI",
                    route.operation_tag
                ),
            ));
        };
        let route_kind = SurfaceOperationKind::from(&route.request);
        if route_kind != expected.kind {
            return Err(error(
                SurfaceRouteBindingErrorKind::RequestKindMismatch,
                format!(
                    "route request kind does not match ABI operation tag `{}`",
                    route.operation_tag
                ),
            ));
        }
        if route.path != expected.path {
            return Err(error(
                SurfaceRouteBindingErrorKind::PathMismatch,
                format!(
                    "route path does not match ABI operation tag `{}`",
                    route.operation_tag
                ),
            ));
        }
        if route.surface.module != expected.surface_module {
            return Err(error(
                SurfaceRouteBindingErrorKind::SurfaceModuleMismatch,
                format!(
                    "route surface module does not match ABI operation tag `{}`",
                    route.operation_tag
                ),
            ));
        }
        if route.surface.name != expected.surface_name {
            return Err(error(
                SurfaceRouteBindingErrorKind::SurfaceNameMismatch,
                format!(
                    "route surface name does not match ABI operation tag `{}`",
                    route.operation_tag
                ),
            ));
        }
        if route.alias != expected.alias {
            return Err(error(
                SurfaceRouteBindingErrorKind::AliasMismatch,
                format!(
                    "route alias does not match ABI operation tag `{}`",
                    route.operation_tag
                ),
            ));
        }
        by_path.insert(
            route.path.clone(),
            SurfaceRouteBinding {
                path: route.path.clone(),
                operation_tag: route.operation_tag.clone(),
                kind: expected.kind,
                surface_module: expected.surface_module.clone(),
                surface_name: expected.surface_name.clone(),
                alias: expected.alias.clone(),
            },
        );
    }
    Ok(by_path.into_values().collect())
}

fn error(
    kind: SurfaceRouteBindingErrorKind,
    message: impl Into<String>,
) -> SurfaceRouteBindingError {
    SurfaceRouteBindingError {
        kind,
        message: message.into(),
    }
}
