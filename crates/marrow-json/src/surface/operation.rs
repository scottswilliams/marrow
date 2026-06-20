use marrow_run::{
    Host, ProjectInvokeError, ProjectSurfaceReadSession, ProjectSurfaceSession, RUN_ENTRY_ARGUMENT,
    SURFACE_ABI_MISMATCH, SURFACE_ACTION, SURFACE_CONFLICT, SURFACE_INVALID_DATA, SURFACE_LIMIT,
    SURFACE_REQUEST, SURFACE_STORE, SURFACE_WRITE, SurfaceError, SurfaceReadOperation,
    SurfaceUpdate, entry_arguments_from_json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use super::execute::{
    execute_page, execute_point_read, execute_point_update, execute_singleton_read,
    execute_singleton_update, execute_unique_lookup,
};
use super::{
    SurfaceActionValueJson, SurfaceOperationKind, SurfacePageJson, SurfacePageRequestJson,
    SurfacePointRequestJson, SurfacePointUpdateRequestJson, SurfaceRecordJson,
    SurfaceSingletonUpdateRequestJson, SurfaceUniqueLookupRequestJson,
    surface_action_value_to_json,
};

pub const SURFACE_OPERATION_PROFILE_VERSION: &str = "surface.operation.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceOperationRequestJson {
    pub profile_version: String,
    pub operation_tag: String,
    pub request: SurfaceOperationRequestBodyJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceOperationRequestBodyJson {
    SingletonRead,
    PointRead {
        request: SurfacePointRequestJson,
    },
    Page {
        request: SurfacePageRequestJson,
    },
    UniqueLookup {
        request: SurfaceUniqueLookupRequestJson,
    },
    SingletonUpdate {
        request: SurfaceSingletonUpdateRequestJson,
    },
    PointUpdate {
        request: SurfacePointUpdateRequestJson,
    },
    Action {
        request: SurfaceActionRequestJson,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceOperationResponseJson {
    pub profile_version: String,
    pub operation_tag: String,
    pub result: SurfaceOperationResultJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceOperationResultJson {
    Record { record: SurfaceRecordJson },
    Page { page: SurfacePageJson },
    OptionalRecord { record: Option<SurfaceRecordJson> },
    Updated,
    Action { result: SurfaceActionResultJson },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceActionRequestJson {
    pub arguments: Vec<Json>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceActionResultJson {
    pub output: String,
    pub value: Option<SurfaceActionValueJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceOperationErrorJson {
    pub code: String,
    pub message: String,
}

pub fn execute_project_surface_operation_read_only(
    session: &ProjectSurfaceReadSession,
    request: &SurfaceOperationRequestJson,
) -> Result<SurfaceOperationResponseJson, SurfaceOperationErrorJson> {
    validate_profile(request)?;
    let kind = request_kind(session.program(), request)?;
    if kind.requires_write_session() {
        return Err(abi_mismatch(
            "surface operation request requires a writable project surface session",
        ));
    }
    let operation = session
        .admit_read_by_operation_tag(&request.operation_tag)
        .map_err(SurfaceOperationErrorJson::from)?;
    let result = execute_read_operation(operation, &request.operation_tag, &request.request)
        .map_err(SurfaceOperationErrorJson::from)?;
    Ok(operation_response(request, result))
}

pub fn execute_project_surface_operation(
    session: &ProjectSurfaceSession,
    request: &SurfaceOperationRequestJson,
) -> Result<SurfaceOperationResponseJson, SurfaceOperationErrorJson> {
    let host = Host::new();
    execute_project_surface_operation_with_host(session, request, &host)
}

pub fn execute_project_surface_operation_with_host(
    session: &ProjectSurfaceSession,
    request: &SurfaceOperationRequestJson,
    host: &Host,
) -> Result<SurfaceOperationResponseJson, SurfaceOperationErrorJson> {
    validate_profile(request)?;
    let kind = request_kind(session.program(), request)?;
    let result = match kind {
        SurfaceOperationKind::SingletonUpdate | SurfaceOperationKind::PointUpdate => {
            execute_update_for_session(session, request)?
        }
        SurfaceOperationKind::Action => match &request.request {
            SurfaceOperationRequestBodyJson::Action { request: action } => {
                execute_action_for_session(session, &request.operation_tag, action, host)?
            }
            _ => {
                return Err(request_error(
                    "surface operation request body does not match the operation tag",
                ));
            }
        },
        SurfaceOperationKind::SingletonRead
        | SurfaceOperationKind::PointRead
        | SurfaceOperationKind::Page
        | SurfaceOperationKind::UniqueLookup => execute_read_for_session(session, request)?,
    };
    Ok(operation_response(request, result))
}

impl From<SurfaceError> for SurfaceOperationErrorJson {
    fn from(error: SurfaceError) -> Self {
        let code = error.code().to_string();
        if let Some(message) = public_fault_message(&code) {
            return Self {
                code,
                message: message.to_string(),
            };
        }
        let rendered = error.to_string();
        let prefix = format!("{code}: ");
        let message = rendered
            .strip_prefix(&prefix)
            .unwrap_or(&rendered)
            .to_string();
        Self { code, message }
    }
}

fn public_fault_message(code: &str) -> Option<&'static str> {
    match code {
        SURFACE_ABI_MISMATCH => Some("surface operation is not active"),
        SURFACE_CONFLICT => Some("surface operation conflicts with existing saved data"),
        SURFACE_INVALID_DATA => Some("surface operation reached invalid saved data"),
        SURFACE_LIMIT => Some("surface operation exceeded a public limit"),
        SURFACE_ACTION => Some("surface action failed"),
        SURFACE_STORE => Some("surface store fault while executing operation"),
        SURFACE_WRITE => Some("surface write could not be applied"),
        _ => None,
    }
}

fn validate_profile(
    request: &SurfaceOperationRequestJson,
) -> Result<(), SurfaceOperationErrorJson> {
    if request.profile_version == SURFACE_OPERATION_PROFILE_VERSION {
        Ok(())
    } else {
        Err(abi_mismatch(
            "surface operation profile version is not active",
        ))
    }
}

fn request_kind(
    program: &marrow_check::CheckedProgram,
    request: &SurfaceOperationRequestJson,
) -> Result<SurfaceOperationKind, SurfaceOperationErrorJson> {
    let Some(kind) = SurfaceOperationKind::from_program_tag(program, &request.operation_tag)
        .map_err(|error| abi_mismatch(error.to_string()))?
    else {
        return Err(abi_mismatch("surface operation is not active"));
    };
    if !kind.matches_operation_body(&request.request) {
        return Err(request_error(
            "surface operation request body does not match the operation tag",
        ));
    }
    Ok(kind)
}

fn execute_read_for_session(
    session: &ProjectSurfaceSession,
    request: &SurfaceOperationRequestJson,
) -> Result<SurfaceOperationResultJson, SurfaceOperationErrorJson> {
    let operation = session
        .admit_read_by_operation_tag(&request.operation_tag)
        .map_err(SurfaceOperationErrorJson::from)?;
    execute_read_operation(operation, &request.operation_tag, &request.request)
        .map_err(SurfaceOperationErrorJson::from)
}

fn execute_update_for_session(
    session: &ProjectSurfaceSession,
    request: &SurfaceOperationRequestJson,
) -> Result<SurfaceOperationResultJson, SurfaceOperationErrorJson> {
    let update = session
        .admit_update_by_operation_tag(&request.operation_tag)
        .map_err(SurfaceOperationErrorJson::from)?;
    execute_update_operation(update, &request.request).map_err(SurfaceOperationErrorJson::from)
}

fn execute_read_operation(
    operation: SurfaceReadOperation<'_>,
    operation_tag: &str,
    request: &SurfaceOperationRequestBodyJson,
) -> Result<SurfaceOperationResultJson, SurfaceError> {
    match request {
        SurfaceOperationRequestBodyJson::SingletonRead => Ok(SurfaceOperationResultJson::Record {
            record: execute_singleton_read(operation)?,
        }),
        SurfaceOperationRequestBodyJson::PointRead { request } => {
            Ok(SurfaceOperationResultJson::Record {
                record: execute_point_read(operation, request)?,
            })
        }
        SurfaceOperationRequestBodyJson::Page { request } => Ok(SurfaceOperationResultJson::Page {
            page: execute_page(operation, operation_tag, request)?,
        }),
        SurfaceOperationRequestBodyJson::UniqueLookup { request } => {
            Ok(SurfaceOperationResultJson::OptionalRecord {
                record: execute_unique_lookup(operation, request)?,
            })
        }
        SurfaceOperationRequestBodyJson::SingletonUpdate { .. }
        | SurfaceOperationRequestBodyJson::PointUpdate { .. }
        | SurfaceOperationRequestBodyJson::Action { .. } => Err(SurfaceError::request(
            "surface operation request body does not match a read operation",
        )),
    }
}

fn execute_update_operation(
    update: SurfaceUpdate<'_>,
    request: &SurfaceOperationRequestBodyJson,
) -> Result<SurfaceOperationResultJson, SurfaceError> {
    match request {
        SurfaceOperationRequestBodyJson::SingletonUpdate { request } => {
            execute_singleton_update(update, request)?;
            Ok(SurfaceOperationResultJson::Updated)
        }
        SurfaceOperationRequestBodyJson::PointUpdate { request } => {
            execute_point_update(update, request)?;
            Ok(SurfaceOperationResultJson::Updated)
        }
        SurfaceOperationRequestBodyJson::SingletonRead
        | SurfaceOperationRequestBodyJson::PointRead { .. }
        | SurfaceOperationRequestBodyJson::Page { .. }
        | SurfaceOperationRequestBodyJson::UniqueLookup { .. }
        | SurfaceOperationRequestBodyJson::Action { .. } => Err(SurfaceError::request(
            "surface operation request body does not match an update operation",
        )),
    }
}

fn execute_action_for_session(
    session: &ProjectSurfaceSession,
    operation_tag: &str,
    request: &SurfaceActionRequestJson,
    host: &Host,
) -> Result<SurfaceOperationResultJson, SurfaceOperationErrorJson> {
    let action = session
        .admit_action_by_operation_tag(operation_tag)
        .map_err(SurfaceOperationErrorJson::from)?;
    let arguments = entry_arguments_from_json(&request.arguments)
        .map_err(|error| request_error(error.message()))?;
    let mut output = String::new();
    let run_output = session
        .invoke_action(&action, arguments, host, &mut output)
        .map_err(surface_action_error)?;
    let value = run_output
        .value
        .as_ref()
        .map(|value| surface_action_value_to_json(session.program(), value))
        .transpose()
        .map_err(|_| operation_error(SURFACE_ACTION, "surface action failed"))?;
    Ok(SurfaceOperationResultJson::Action {
        result: SurfaceActionResultJson { output, value },
    })
}

fn operation_response(
    request: &SurfaceOperationRequestJson,
    result: SurfaceOperationResultJson,
) -> SurfaceOperationResponseJson {
    SurfaceOperationResponseJson {
        profile_version: SURFACE_OPERATION_PROFILE_VERSION.to_string(),
        operation_tag: request.operation_tag.clone(),
        result,
    }
}

fn abi_mismatch(message: impl Into<String>) -> SurfaceOperationErrorJson {
    operation_error(SURFACE_ABI_MISMATCH, message)
}

fn request_error(message: impl Into<String>) -> SurfaceOperationErrorJson {
    operation_error(SURFACE_REQUEST, message)
}

fn surface_action_error(error: ProjectInvokeError) -> SurfaceOperationErrorJson {
    if error.code() == RUN_ENTRY_ARGUMENT {
        request_error(error.message())
    } else {
        operation_error(SURFACE_ACTION, "surface action failed")
    }
}

fn operation_error(code: &str, message: impl Into<String>) -> SurfaceOperationErrorJson {
    SurfaceOperationErrorJson {
        code: code.to_string(),
        message: message.into(),
    }
}
