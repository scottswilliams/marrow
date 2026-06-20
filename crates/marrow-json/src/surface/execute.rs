use marrow_check::CheckedProgram;
use marrow_run::{
    ProjectSurfaceReadSession, ProjectSurfaceSession, SurfaceCreate, SurfaceDelete,
    SurfaceDeleteInput, SurfaceError, SurfaceReadError, SurfaceReadInput, SurfaceReadOperation,
    SurfaceUpdate,
};
use marrow_store::tree::TreeStore;

use super::{
    SurfacePageJson, SurfacePageRequestJson, SurfacePointCreateRequestJson,
    SurfacePointDeleteRequestJson, SurfacePointRequestJson, SurfacePointUpdateRequestJson,
    SurfaceRecordJson, SurfaceSingletonCreateRequestJson, SurfaceSingletonUpdateRequestJson,
    SurfaceUniqueLookupRequestJson,
};

pub fn execute_surface_point_read_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
    request: &SurfacePointRequestJson,
) -> Result<SurfaceRecordJson, SurfaceReadError> {
    let operation = SurfaceReadOperation::admit_by_operation_tag(program, store, operation_tag)?;
    execute_point_read(operation, request)
}

pub fn execute_project_surface_point_read_by_tag(
    session: &ProjectSurfaceReadSession,
    operation_tag: &str,
    request: &SurfacePointRequestJson,
) -> Result<SurfaceRecordJson, SurfaceReadError> {
    let operation = session.admit_read_by_operation_tag(operation_tag)?;
    execute_point_read(operation, request)
}

pub(super) fn execute_point_read(
    operation: SurfaceReadOperation<'_>,
    request: &SurfacePointRequestJson,
) -> Result<SurfaceRecordJson, SurfaceReadError> {
    let read = operation.point_read()?;
    let decoded = request.decode(read)?;
    let record = read.execute(SurfaceReadInput::Point {
        identity: decoded.identity(),
    })?;
    Ok(SurfaceRecordJson::from(&record))
}

pub fn execute_surface_singleton_read_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
) -> Result<SurfaceRecordJson, SurfaceReadError> {
    let operation = SurfaceReadOperation::admit_by_operation_tag(program, store, operation_tag)?;
    execute_singleton_read(operation)
}

pub fn execute_project_surface_singleton_read_by_tag(
    session: &ProjectSurfaceReadSession,
    operation_tag: &str,
) -> Result<SurfaceRecordJson, SurfaceReadError> {
    let operation = session.admit_read_by_operation_tag(operation_tag)?;
    execute_singleton_read(operation)
}

pub(super) fn execute_singleton_read(
    operation: SurfaceReadOperation<'_>,
) -> Result<SurfaceRecordJson, SurfaceReadError> {
    let read = operation.singleton_read()?;
    let record = read.execute(SurfaceReadInput::Singleton)?;
    Ok(SurfaceRecordJson::from(&record))
}

pub fn execute_surface_page_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
    request: &SurfacePageRequestJson,
) -> Result<SurfacePageJson, SurfaceReadError> {
    let operation = SurfaceReadOperation::admit_by_operation_tag(program, store, operation_tag)?;
    execute_page(operation, operation_tag, request)
}

pub fn execute_project_surface_page_by_tag(
    session: &ProjectSurfaceReadSession,
    operation_tag: &str,
    request: &SurfacePageRequestJson,
) -> Result<SurfacePageJson, SurfaceReadError> {
    let operation = session.admit_read_by_operation_tag(operation_tag)?;
    execute_page(operation, operation_tag, request)
}

pub(super) fn execute_page(
    operation: SurfaceReadOperation<'_>,
    operation_tag: &str,
    request: &SurfacePageRequestJson,
) -> Result<SurfacePageJson, SurfaceReadError> {
    let read = operation.page_read()?;
    request.validate_cursor_operation_tag(operation_tag)?;
    let decoded = request.decode(read)?;
    let page = read.page(decoded.as_page_request())?;
    SurfacePageJson::from_page(read, &page)
}

pub fn execute_surface_unique_lookup_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
    request: &SurfaceUniqueLookupRequestJson,
) -> Result<Option<SurfaceRecordJson>, SurfaceReadError> {
    let operation = SurfaceReadOperation::admit_by_operation_tag(program, store, operation_tag)?;
    execute_unique_lookup(operation, request)
}

pub fn execute_project_surface_unique_lookup_by_tag(
    session: &ProjectSurfaceReadSession,
    operation_tag: &str,
    request: &SurfaceUniqueLookupRequestJson,
) -> Result<Option<SurfaceRecordJson>, SurfaceReadError> {
    let operation = session.admit_read_by_operation_tag(operation_tag)?;
    execute_unique_lookup(operation, request)
}

pub(super) fn execute_unique_lookup(
    operation: SurfaceReadOperation<'_>,
    request: &SurfaceUniqueLookupRequestJson,
) -> Result<Option<SurfaceRecordJson>, SurfaceReadError> {
    let read = operation.unique_lookup()?;
    let decoded = request.decode(read)?;
    Ok(read
        .lookup_unique(decoded.keys())?
        .as_ref()
        .map(SurfaceRecordJson::from))
}

pub fn execute_surface_point_update_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
    request: &SurfacePointUpdateRequestJson,
) -> Result<(), SurfaceError> {
    let update = SurfaceUpdate::admit_by_operation_tag(program, store, operation_tag)?;
    execute_point_update(update, request)
}

pub fn execute_project_surface_point_update_by_tag(
    session: &ProjectSurfaceSession,
    operation_tag: &str,
    request: &SurfacePointUpdateRequestJson,
) -> Result<(), SurfaceError> {
    let update = session.admit_update_by_operation_tag(operation_tag)?;
    execute_point_update(update, request)
}

pub(super) fn execute_point_update(
    update: SurfaceUpdate<'_>,
    request: &SurfacePointUpdateRequestJson,
) -> Result<(), SurfaceError> {
    let decoded = request.decode(&update)?;
    update.execute(decoded.as_update_input())
}

pub fn execute_surface_singleton_update_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
    request: &SurfaceSingletonUpdateRequestJson,
) -> Result<(), SurfaceError> {
    let update = SurfaceUpdate::admit_by_operation_tag(program, store, operation_tag)?;
    execute_singleton_update(update, request)
}

pub fn execute_project_surface_singleton_update_by_tag(
    session: &ProjectSurfaceSession,
    operation_tag: &str,
    request: &SurfaceSingletonUpdateRequestJson,
) -> Result<(), SurfaceError> {
    let update = session.admit_update_by_operation_tag(operation_tag)?;
    execute_singleton_update(update, request)
}

pub(super) fn execute_singleton_update(
    update: SurfaceUpdate<'_>,
    request: &SurfaceSingletonUpdateRequestJson,
) -> Result<(), SurfaceError> {
    let decoded = request.decode(&update)?;
    update.execute(decoded.as_update_input())
}

pub fn execute_surface_point_create_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
    request: &SurfacePointCreateRequestJson,
) -> Result<SurfaceRecordJson, SurfaceError> {
    let create = SurfaceCreate::admit_by_operation_tag(program, store, operation_tag)?;
    execute_point_create(create, request)
}

pub fn execute_project_surface_point_create_by_tag(
    session: &ProjectSurfaceSession,
    operation_tag: &str,
    request: &SurfacePointCreateRequestJson,
) -> Result<SurfaceRecordJson, SurfaceError> {
    let create = session.admit_create_by_operation_tag(operation_tag)?;
    execute_point_create(create, request)
}

pub(super) fn execute_point_create(
    create: SurfaceCreate<'_>,
    request: &SurfacePointCreateRequestJson,
) -> Result<SurfaceRecordJson, SurfaceError> {
    let decoded = request.decode(&create)?;
    let record = create.execute(decoded.as_create_input())?;
    Ok(SurfaceRecordJson::from(&record))
}

pub fn execute_surface_singleton_create_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
    request: &SurfaceSingletonCreateRequestJson,
) -> Result<SurfaceRecordJson, SurfaceError> {
    let create = SurfaceCreate::admit_by_operation_tag(program, store, operation_tag)?;
    execute_singleton_create(create, request)
}

pub fn execute_project_surface_singleton_create_by_tag(
    session: &ProjectSurfaceSession,
    operation_tag: &str,
    request: &SurfaceSingletonCreateRequestJson,
) -> Result<SurfaceRecordJson, SurfaceError> {
    let create = session.admit_create_by_operation_tag(operation_tag)?;
    execute_singleton_create(create, request)
}

pub(super) fn execute_singleton_create(
    create: SurfaceCreate<'_>,
    request: &SurfaceSingletonCreateRequestJson,
) -> Result<SurfaceRecordJson, SurfaceError> {
    let decoded = request.decode(&create)?;
    let record = create.execute(decoded.as_create_input())?;
    Ok(SurfaceRecordJson::from(&record))
}

pub fn execute_surface_point_delete_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
    request: &SurfacePointDeleteRequestJson,
) -> Result<(), SurfaceError> {
    let delete = SurfaceDelete::admit_by_operation_tag(program, store, operation_tag)?;
    execute_point_delete(delete, request)
}

pub fn execute_project_surface_point_delete_by_tag(
    session: &ProjectSurfaceSession,
    operation_tag: &str,
    request: &SurfacePointDeleteRequestJson,
) -> Result<(), SurfaceError> {
    let delete = session.admit_delete_by_operation_tag(operation_tag)?;
    execute_point_delete(delete, request)
}

pub(super) fn execute_point_delete(
    delete: SurfaceDelete<'_>,
    request: &SurfacePointDeleteRequestJson,
) -> Result<(), SurfaceError> {
    let decoded = request.decode(&delete)?;
    delete.execute(decoded.as_delete_input())
}

pub fn execute_surface_singleton_delete_by_tag(
    program: &CheckedProgram,
    store: &TreeStore,
    operation_tag: &str,
) -> Result<(), SurfaceError> {
    let delete = SurfaceDelete::admit_by_operation_tag(program, store, operation_tag)?;
    execute_singleton_delete(delete)
}

pub fn execute_project_surface_singleton_delete_by_tag(
    session: &ProjectSurfaceSession,
    operation_tag: &str,
) -> Result<(), SurfaceError> {
    let delete = session.admit_delete_by_operation_tag(operation_tag)?;
    execute_singleton_delete(delete)
}

pub(super) fn execute_singleton_delete(delete: SurfaceDelete<'_>) -> Result<(), SurfaceError> {
    delete.execute(SurfaceDeleteInput::Singleton)
}
