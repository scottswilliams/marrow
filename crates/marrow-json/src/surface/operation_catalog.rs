use std::collections::BTreeMap;

use super::{
    SurfaceAbiJson, SurfaceCreateOperationKindJson, SurfaceDeleteOperationKindJson,
    SurfaceOperationRequestBodyJson, SurfaceReadOperationKindJson, SurfaceRouteRequestJson,
    SurfaceUpdateOperationKindJson,
};

const SURFACE_READ_ROUTE_PREFIX: &str = "/surface/v1/read/";
const SURFACE_UPDATE_ROUTE_PREFIX: &str = "/surface/v1/update/";
const SURFACE_CREATE_ROUTE_PREFIX: &str = "/surface/v1/create/";
const SURFACE_DELETE_ROUTE_PREFIX: &str = "/surface/v1/delete/";
const SURFACE_ACTION_ROUTE_PREFIX: &str = "/surface/v1/action/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceOperationKind {
    SingletonRead,
    PointRead,
    Page,
    UniqueLookup,
    SingletonUpdate,
    PointUpdate,
    SingletonCreate,
    PointCreate,
    SingletonDelete,
    PointDelete,
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceOperationBinding {
    pub operation_tag: String,
    pub kind: SurfaceOperationKind,
    pub path: String,
    pub surface_module: String,
    pub surface_name: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceOperationCatalog {
    by_tag: BTreeMap<String, SurfaceOperationBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceOperationCatalogError {
    kind: SurfaceOperationCatalogErrorKind,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceOperationCatalogErrorKind {
    DuplicateOperationTag,
}

impl SurfaceOperationKind {
    pub fn is_read(self) -> bool {
        matches!(
            self,
            Self::SingletonRead | Self::PointRead | Self::Page | Self::UniqueLookup
        )
    }

    pub fn requires_write_session(self) -> bool {
        matches!(
            self,
            Self::SingletonUpdate
                | Self::PointUpdate
                | Self::SingletonCreate
                | Self::PointCreate
                | Self::SingletonDelete
                | Self::PointDelete
                | Self::Action
        )
    }

    pub fn matches_operation_body(self, body: &SurfaceOperationRequestBodyJson) -> bool {
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
                | (
                    Self::SingletonCreate,
                    SurfaceOperationRequestBodyJson::SingletonCreate { .. }
                )
                | (
                    Self::PointCreate,
                    SurfaceOperationRequestBodyJson::PointCreate { .. }
                )
                | (
                    Self::SingletonDelete,
                    SurfaceOperationRequestBodyJson::SingletonDelete
                )
                | (
                    Self::PointDelete,
                    SurfaceOperationRequestBodyJson::PointDelete { .. }
                )
                | (Self::Action, SurfaceOperationRequestBodyJson::Action { .. })
        )
    }

    pub fn route_prefix(self) -> &'static str {
        match self {
            Self::SingletonRead | Self::PointRead | Self::Page | Self::UniqueLookup => {
                SURFACE_READ_ROUTE_PREFIX
            }
            Self::SingletonUpdate | Self::PointUpdate => SURFACE_UPDATE_ROUTE_PREFIX,
            Self::SingletonCreate | Self::PointCreate => SURFACE_CREATE_ROUTE_PREFIX,
            Self::SingletonDelete | Self::PointDelete => SURFACE_DELETE_ROUTE_PREFIX,
            Self::Action => SURFACE_ACTION_ROUTE_PREFIX,
        }
    }

    pub fn route_request(self) -> SurfaceRouteRequestJson {
        match self {
            Self::SingletonRead => SurfaceRouteRequestJson::SingletonRead,
            Self::PointRead => SurfaceRouteRequestJson::PointRead,
            Self::Page => SurfaceRouteRequestJson::Page,
            Self::UniqueLookup => SurfaceRouteRequestJson::UniqueLookup,
            Self::SingletonUpdate => SurfaceRouteRequestJson::SingletonUpdate,
            Self::PointUpdate => SurfaceRouteRequestJson::PointUpdate,
            Self::SingletonCreate => SurfaceRouteRequestJson::SingletonCreate,
            Self::PointCreate => SurfaceRouteRequestJson::PointCreate,
            Self::SingletonDelete => SurfaceRouteRequestJson::SingletonDelete,
            Self::PointDelete => SurfaceRouteRequestJson::PointDelete,
            Self::Action => SurfaceRouteRequestJson::Action,
        }
    }

    pub fn operation_request_kind(self) -> &'static str {
        match self {
            Self::SingletonRead => "singleton_read",
            Self::PointRead => "point_read",
            Self::Page => "page",
            Self::UniqueLookup => "unique_lookup",
            Self::SingletonUpdate => "singleton_update",
            Self::PointUpdate => "point_update",
            Self::SingletonCreate => "singleton_create",
            Self::PointCreate => "point_create",
            Self::SingletonDelete => "singleton_delete",
            Self::PointDelete => "point_delete",
            Self::Action => "action",
        }
    }

    pub fn operation_result_kind(self) -> &'static str {
        match self {
            Self::SingletonRead | Self::PointRead => "record",
            Self::Page => "page",
            Self::UniqueLookup => "optional_record",
            Self::SingletonUpdate | Self::PointUpdate => "updated",
            Self::SingletonCreate | Self::PointCreate => "created",
            Self::SingletonDelete | Self::PointDelete => "deleted",
            Self::Action => "action",
        }
    }

    pub(crate) fn from_program_tag(
        program: &marrow_check::CheckedProgram,
        operation_tag: &str,
    ) -> Result<Option<Self>, SurfaceOperationCatalogError> {
        let mut matched = None;
        for surface in program.facts.surfaces() {
            if !matches!(
                surface.catalog_status,
                marrow_check::SurfaceCatalogStatus::Stable
            ) {
                continue;
            }
            for operation in &surface.read_operations {
                if let Some(descriptor) =
                    marrow_check::SurfaceReadOperationDescriptor::from_operation(
                        program, surface, operation,
                    )
                    .filter(|descriptor| descriptor.operation_tag == operation_tag)
                {
                    record_kind_match(&mut matched, Self::from(descriptor.kind), operation_tag)?;
                }
            }
            if let Some(descriptor) =
                marrow_check::SurfaceUpdateOperationDescriptor::from_surface(program, surface)
                    .filter(|descriptor| descriptor.operation_tag == operation_tag)
            {
                record_kind_match(&mut matched, Self::from(descriptor.kind), operation_tag)?;
            }
            if let Some(descriptor) =
                marrow_check::SurfaceCreateOperationDescriptor::from_surface(program, surface)
                    .filter(|descriptor| descriptor.operation_tag == operation_tag)
            {
                record_kind_match(&mut matched, Self::from(descriptor.kind), operation_tag)?;
            }
            if let Some(descriptor) =
                marrow_check::SurfaceDeleteOperationDescriptor::from_surface(program, surface)
                    .filter(|descriptor| descriptor.operation_tag == operation_tag)
            {
                record_kind_match(&mut matched, Self::from(descriptor.kind), operation_tag)?;
            }
            for action in &surface.actions {
                if let Some(_descriptor) =
                    marrow_check::SurfaceActionOperationDescriptor::from_action(
                        program, surface, action,
                    )
                    .filter(|descriptor| descriptor.operation_tag == operation_tag)
                {
                    record_kind_match(&mut matched, Self::Action, operation_tag)?;
                }
            }
        }
        Ok(matched)
    }
}

impl SurfaceOperationCatalog {
    pub fn from_abi(abi: &SurfaceAbiJson) -> Result<Self, SurfaceOperationCatalogError> {
        let mut by_tag = BTreeMap::new();
        for surface in &abi.surfaces {
            for read in &surface.read {
                let kind = SurfaceOperationKind::from(&read.kind);
                insert_binding(
                    &mut by_tag,
                    SurfaceOperationBinding {
                        operation_tag: read.operation_tag.clone(),
                        kind,
                        path: operation_path(kind, &read.operation_tag),
                        surface_module: surface.module.clone(),
                        surface_name: surface.name.clone(),
                        alias: read.alias.clone(),
                    },
                )?;
            }
            if let Some(create) = &surface.create {
                let kind = SurfaceOperationKind::from(&create.kind);
                insert_binding(
                    &mut by_tag,
                    SurfaceOperationBinding {
                        operation_tag: create.operation_tag.clone(),
                        kind,
                        path: operation_path(kind, &create.operation_tag),
                        surface_module: surface.module.clone(),
                        surface_name: surface.name.clone(),
                        alias: "create".into(),
                    },
                )?;
            }
            if let Some(update) = &surface.update {
                let kind = SurfaceOperationKind::from(&update.kind);
                insert_binding(
                    &mut by_tag,
                    SurfaceOperationBinding {
                        operation_tag: update.operation_tag.clone(),
                        kind,
                        path: operation_path(kind, &update.operation_tag),
                        surface_module: surface.module.clone(),
                        surface_name: surface.name.clone(),
                        alias: "update".into(),
                    },
                )?;
            }
            if let Some(delete) = &surface.delete {
                let kind = SurfaceOperationKind::from(&delete.kind);
                insert_binding(
                    &mut by_tag,
                    SurfaceOperationBinding {
                        operation_tag: delete.operation_tag.clone(),
                        kind,
                        path: operation_path(kind, &delete.operation_tag),
                        surface_module: surface.module.clone(),
                        surface_name: surface.name.clone(),
                        alias: "delete".into(),
                    },
                )?;
            }
            for action in &surface.actions {
                let kind = SurfaceOperationKind::Action;
                insert_binding(
                    &mut by_tag,
                    SurfaceOperationBinding {
                        operation_tag: action.operation_tag.clone(),
                        kind,
                        path: operation_path(kind, &action.operation_tag),
                        surface_module: surface.module.clone(),
                        surface_name: surface.name.clone(),
                        alias: action.alias.clone(),
                    },
                )?;
            }
        }
        Ok(Self { by_tag })
    }

    pub fn binding(&self, operation_tag: &str) -> Option<&SurfaceOperationBinding> {
        self.by_tag.get(operation_tag)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SurfaceOperationBinding> {
        self.by_tag.values()
    }

    pub fn kind(&self, operation_tag: &str) -> Option<SurfaceOperationKind> {
        self.binding(operation_tag).map(|binding| binding.kind)
    }

    pub fn len(&self) -> usize {
        self.by_tag.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tag.is_empty()
    }
}

impl From<marrow_check::SurfaceReadOperationDescriptorKind> for SurfaceOperationKind {
    fn from(kind: marrow_check::SurfaceReadOperationDescriptorKind) -> Self {
        match kind {
            marrow_check::SurfaceReadOperationDescriptorKind::SingletonRead => Self::SingletonRead,
            marrow_check::SurfaceReadOperationDescriptorKind::PointRead => Self::PointRead,
            marrow_check::SurfaceReadOperationDescriptorKind::PagedRootCollection
            | marrow_check::SurfaceReadOperationDescriptorKind::PagedIndexCollection { .. } => {
                Self::Page
            }
            marrow_check::SurfaceReadOperationDescriptorKind::UniqueIndexLookup { .. } => {
                Self::UniqueLookup
            }
        }
    }
}

impl From<marrow_check::SurfaceUpdateOperationDescriptorKind> for SurfaceOperationKind {
    fn from(kind: marrow_check::SurfaceUpdateOperationDescriptorKind) -> Self {
        match kind {
            marrow_check::SurfaceUpdateOperationDescriptorKind::SingletonUpdate => {
                Self::SingletonUpdate
            }
            marrow_check::SurfaceUpdateOperationDescriptorKind::PointUpdate => Self::PointUpdate,
        }
    }
}

impl From<marrow_check::SurfaceCreateOperationDescriptorKind> for SurfaceOperationKind {
    fn from(kind: marrow_check::SurfaceCreateOperationDescriptorKind) -> Self {
        match kind {
            marrow_check::SurfaceCreateOperationDescriptorKind::SingletonCreate => {
                Self::SingletonCreate
            }
            marrow_check::SurfaceCreateOperationDescriptorKind::PointCreate => Self::PointCreate,
        }
    }
}

impl From<marrow_check::SurfaceDeleteOperationDescriptorKind> for SurfaceOperationKind {
    fn from(kind: marrow_check::SurfaceDeleteOperationDescriptorKind) -> Self {
        match kind {
            marrow_check::SurfaceDeleteOperationDescriptorKind::SingletonDelete => {
                Self::SingletonDelete
            }
            marrow_check::SurfaceDeleteOperationDescriptorKind::PointDelete => Self::PointDelete,
        }
    }
}

impl From<&SurfaceReadOperationKindJson> for SurfaceOperationKind {
    fn from(kind: &SurfaceReadOperationKindJson) -> Self {
        match kind {
            SurfaceReadOperationKindJson::SingletonRead => Self::SingletonRead,
            SurfaceReadOperationKindJson::PointRead => Self::PointRead,
            SurfaceReadOperationKindJson::PagedRootCollection
            | SurfaceReadOperationKindJson::PagedIndexCollection { .. } => Self::Page,
            SurfaceReadOperationKindJson::UniqueIndexLookup { .. } => Self::UniqueLookup,
        }
    }
}

impl From<&SurfaceUpdateOperationKindJson> for SurfaceOperationKind {
    fn from(kind: &SurfaceUpdateOperationKindJson) -> Self {
        match kind {
            SurfaceUpdateOperationKindJson::SingletonUpdate => Self::SingletonUpdate,
            SurfaceUpdateOperationKindJson::PointUpdate => Self::PointUpdate,
        }
    }
}

impl From<&SurfaceCreateOperationKindJson> for SurfaceOperationKind {
    fn from(kind: &SurfaceCreateOperationKindJson) -> Self {
        match kind {
            SurfaceCreateOperationKindJson::SingletonCreate => Self::SingletonCreate,
            SurfaceCreateOperationKindJson::PointCreate => Self::PointCreate,
        }
    }
}

impl From<&SurfaceDeleteOperationKindJson> for SurfaceOperationKind {
    fn from(kind: &SurfaceDeleteOperationKindJson) -> Self {
        match kind {
            SurfaceDeleteOperationKindJson::SingletonDelete => Self::SingletonDelete,
            SurfaceDeleteOperationKindJson::PointDelete => Self::PointDelete,
        }
    }
}

impl From<&SurfaceRouteRequestJson> for SurfaceOperationKind {
    fn from(request: &SurfaceRouteRequestJson) -> Self {
        match request {
            SurfaceRouteRequestJson::SingletonRead => Self::SingletonRead,
            SurfaceRouteRequestJson::PointRead => Self::PointRead,
            SurfaceRouteRequestJson::Page => Self::Page,
            SurfaceRouteRequestJson::UniqueLookup => Self::UniqueLookup,
            SurfaceRouteRequestJson::SingletonUpdate => Self::SingletonUpdate,
            SurfaceRouteRequestJson::PointUpdate => Self::PointUpdate,
            SurfaceRouteRequestJson::SingletonCreate => Self::SingletonCreate,
            SurfaceRouteRequestJson::PointCreate => Self::PointCreate,
            SurfaceRouteRequestJson::SingletonDelete => Self::SingletonDelete,
            SurfaceRouteRequestJson::PointDelete => Self::PointDelete,
            SurfaceRouteRequestJson::Action => Self::Action,
        }
    }
}

impl SurfaceOperationCatalogError {
    pub fn kind(&self) -> SurfaceOperationCatalogErrorKind {
        self.kind
    }
}

impl std::fmt::Display for SurfaceOperationCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SurfaceOperationCatalogError {}

pub(crate) fn operation_path(kind: SurfaceOperationKind, operation_tag: &str) -> String {
    format!("{}{}", kind.route_prefix(), operation_tag)
}

fn insert_binding(
    by_tag: &mut BTreeMap<String, SurfaceOperationBinding>,
    binding: SurfaceOperationBinding,
) -> Result<(), SurfaceOperationCatalogError> {
    if by_tag.contains_key(&binding.operation_tag) {
        return Err(catalog_error(
            SurfaceOperationCatalogErrorKind::DuplicateOperationTag,
            format!(
                "duplicate operation tag `{}` in surface ABI",
                binding.operation_tag
            ),
        ));
    }
    by_tag.insert(binding.operation_tag.clone(), binding);
    Ok(())
}

fn record_kind_match(
    matched: &mut Option<SurfaceOperationKind>,
    kind: SurfaceOperationKind,
    operation_tag: &str,
) -> Result<(), SurfaceOperationCatalogError> {
    if matched.replace(kind).is_some() {
        return Err(catalog_error(
            SurfaceOperationCatalogErrorKind::DuplicateOperationTag,
            format!("duplicate operation tag `{operation_tag}` in checked surface operations"),
        ));
    }
    Ok(())
}

fn catalog_error(
    kind: SurfaceOperationCatalogErrorKind,
    message: impl Into<String>,
) -> SurfaceOperationCatalogError {
    SurfaceOperationCatalogError {
        kind,
        message: message.into(),
    }
}
