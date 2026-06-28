use marrow_run::{SurfaceServeBoundary, SurfaceServeMode, SurfaceServeProcessControl};
use serde::Serialize;

use crate::saved_data::DataViewBoundaryJson;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceServeBoundaryJson {
    #[serde(rename = "serveMode")]
    pub serve_mode: &'static str,
    #[serde(rename = "dataViewBoundary")]
    pub data_view_boundary: DataViewBoundaryJson,
    #[serde(rename = "processControl")]
    pub process_control: SurfaceServeProcessControlJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceServeProcessControlJson {
    pub kind: &'static str,
}

pub fn surface_serve_boundary_to_json(boundary: &SurfaceServeBoundary) -> SurfaceServeBoundaryJson {
    SurfaceServeBoundaryJson {
        serve_mode: surface_serve_mode_name(boundary.mode),
        data_view_boundary: DataViewBoundaryJson::from(&boundary.data_view_boundary),
        process_control: surface_serve_process_control_to_json(boundary.process_control),
    }
}

fn surface_serve_mode_name(mode: SurfaceServeMode) -> &'static str {
    match mode {
        SurfaceServeMode::ReadOnly => "read_only",
        SurfaceServeMode::Write => "write",
    }
}

fn surface_serve_process_control_to_json(
    control: SurfaceServeProcessControl,
) -> SurfaceServeProcessControlJson {
    match control {
        SurfaceServeProcessControl::NotExposed => SurfaceServeProcessControlJson {
            kind: "not_exposed",
        },
    }
}
