use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde::Serialize;

use super::{
    SurfaceAbiJson, SurfaceOperationCatalog, SurfaceOperationCatalogError, SurfaceOperationKind,
    SurfaceRouteBinding, SurfaceRouteBindingError, SurfaceRouteBindings, SurfaceRouteManifestJson,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceClientRenderError {
    kind: SurfaceClientRenderErrorKind,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceClientRenderErrorKind {
    OperationCatalog,
    RouteBinding,
}

pub fn render_typescript_client(
    abi: &SurfaceAbiJson,
    routes: &SurfaceRouteManifestJson,
) -> Result<String, SurfaceClientRenderError> {
    let catalog = SurfaceOperationCatalog::from_abi(abi).map_err(SurfaceClientRenderError::from)?;
    let bindings = SurfaceRouteBindings::from_manifest_for_client(routes, &catalog)
        .map_err(SurfaceClientRenderError::from)?;
    Ok(render_bindings(&bindings))
}

impl SurfaceClientRenderError {
    pub fn kind(&self) -> SurfaceClientRenderErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for SurfaceClientRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SurfaceClientRenderError {}

impl From<SurfaceOperationCatalogError> for SurfaceClientRenderError {
    fn from(error: SurfaceOperationCatalogError) -> Self {
        Self {
            kind: SurfaceClientRenderErrorKind::OperationCatalog,
            message: error.to_string(),
        }
    }
}

impl From<SurfaceRouteBindingError> for SurfaceClientRenderError {
    fn from(error: SurfaceRouteBindingError) -> Self {
        Self {
            kind: SurfaceClientRenderErrorKind::RouteBinding,
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct SurfaceClientGroup {
    module: String,
    surface: String,
    operations: Vec<SurfaceRouteBinding>,
}

#[derive(Debug, Clone)]
struct NamedGroup {
    name: String,
    group: SurfaceClientGroup,
}

#[derive(Debug, Clone)]
struct NamedOperation {
    name: String,
    binding: SurfaceRouteBinding,
}

#[derive(Debug, Serialize)]
struct TypeScriptOperationBinding {
    operation_tag: String,
    path: String,
    request_kind: &'static str,
    result_kind: &'static str,
}

fn render_bindings(bindings: &SurfaceRouteBindings) -> String {
    let modules = named_modules(bindings);
    let mut output = String::new();
    output.push_str(CLIENT_PREAMBLE);
    render_operation_binding_constants(&mut output, &modules);
    output.push_str(
        "export function createMarrowSurfaceClient(options: MarrowSurfaceClientOptions = {}) {\n",
    );
    output.push_str("  return {\n");
    for module in modules {
        writeln!(output, "    {}: {{", ts_string(&module.name)).expect("write module");
        for surface in named_surfaces(module.group.clone()) {
            writeln!(output, "      {}: {{", ts_string(&surface.name)).expect("write surface");
            for operation in named_operations(surface.group.operations) {
                let binding_ref = format!(
                    "SURFACE_OPERATION_BINDINGS[{}][{}][{}]",
                    ts_string(&module.name),
                    ts_string(&surface.name),
                    ts_string(&operation.name)
                );
                render_operation(&mut output, &operation, &binding_ref);
            }
            output.push_str("      },\n");
        }
        output.push_str("    },\n");
    }
    output.push_str("  };\n");
    output.push_str("}\n");
    output
}

fn named_modules(bindings: &SurfaceRouteBindings) -> Vec<NamedGroup> {
    let mut by_module = BTreeMap::<String, BTreeMap<String, Vec<SurfaceRouteBinding>>>::new();
    for binding in bindings.iter() {
        by_module
            .entry(binding.surface_module.clone())
            .or_default()
            .entry(binding.surface_name.clone())
            .or_default()
            .push(binding.clone());
    }

    let groups = by_module
        .into_iter()
        .map(|(module, surfaces)| {
            let operations = surfaces
                .values()
                .flat_map(|bindings| bindings.iter().cloned())
                .collect::<Vec<_>>();
            SurfaceClientGroup {
                module: module.clone(),
                surface: module,
                operations,
            }
        })
        .collect::<Vec<_>>();
    named_groups(groups, |group| &group.module)
}

fn named_surfaces(module: SurfaceClientGroup) -> Vec<NamedGroup> {
    let mut by_surface = BTreeMap::<String, Vec<SurfaceRouteBinding>>::new();
    for binding in module.operations {
        by_surface
            .entry(binding.surface_name.clone())
            .or_default()
            .push(binding);
    }
    let groups = by_surface
        .into_iter()
        .map(|(surface, operations)| SurfaceClientGroup {
            module: module.module.clone(),
            surface,
            operations,
        })
        .collect::<Vec<_>>();
    named_groups(groups, |group| &group.surface)
}

fn named_groups(
    mut groups: Vec<SurfaceClientGroup>,
    label: impl Fn(&SurfaceClientGroup) -> &str,
) -> Vec<NamedGroup> {
    groups.sort_by(|left, right| {
        sanitize_group_identifier(label(left))
            .cmp(&sanitize_group_identifier(label(right)))
            .then_with(|| group_suffix(left).cmp(&group_suffix(right)))
    });
    let mut used = BTreeSet::new();
    groups
        .into_iter()
        .map(|group| {
            let name = unique_name(
                &mut used,
                &sanitize_group_identifier(label(&group)),
                &group_suffix(&group),
            );
            NamedGroup { name, group }
        })
        .collect()
}

fn named_operations(mut operations: Vec<SurfaceRouteBinding>) -> Vec<NamedOperation> {
    operations.sort_by(|left, right| {
        sanitize_property_identifier(&left.alias)
            .cmp(&sanitize_property_identifier(&right.alias))
            .then_with(|| left.operation_tag.cmp(&right.operation_tag))
    });
    let mut used = BTreeSet::new();
    operations
        .into_iter()
        .map(|binding| {
            let name = unique_name(
                &mut used,
                &sanitize_property_identifier(&binding.alias),
                &operation_tag_suffix(&binding.operation_tag),
            );
            NamedOperation { name, binding }
        })
        .collect()
}

fn unique_name(used: &mut BTreeSet<String>, base: &str, suffix: &str) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let suffixed = format!("{base}__{suffix}");
    if used.insert(suffixed.clone()) {
        return suffixed;
    }
    let mut counter = 2usize;
    loop {
        let candidate = format!("{base}__{suffix}_{counter}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

fn render_operation_binding_constants(output: &mut String, modules: &[NamedGroup]) {
    let constants = operation_binding_constants(modules);
    output.push_str("const SURFACE_OPERATION_BINDINGS = ");
    output.push_str(
        &serde_json::to_string_pretty(&constants)
            .expect("surface operation binding constants should serialize"),
    );
    output.push_str(" as const;\n\n");
}

fn operation_binding_constants(
    modules: &[NamedGroup],
) -> BTreeMap<String, BTreeMap<String, BTreeMap<String, TypeScriptOperationBinding>>> {
    let mut constants = BTreeMap::new();
    for module in modules {
        let mut surfaces = BTreeMap::new();
        for surface in named_surfaces(module.group.clone()) {
            let mut operations = BTreeMap::new();
            for operation in named_operations(surface.group.operations.clone()) {
                let binding = operation.binding;
                operations.insert(
                    operation.name,
                    TypeScriptOperationBinding {
                        operation_tag: binding.operation_tag,
                        path: binding.path,
                        request_kind: binding.kind.operation_request_kind(),
                        result_kind: binding.kind.operation_result_kind(),
                    },
                );
            }
            surfaces.insert(surface.name, operations);
        }
        constants.insert(module.name.clone(), surfaces);
    }
    constants
}

fn render_operation(output: &mut String, operation: &NamedOperation, binding_ref: &str) {
    let method_name = ts_string(&operation.name);
    if let Some(request_type) = request_type(operation.binding.kind) {
        writeln!(
            output,
            "        {method_name}: (request: {request_type}) => invoke({binding_ref}, request, options),"
        )
        .expect("write operation");
    } else {
        writeln!(
            output,
            "        {method_name}: () => invoke({binding_ref}, undefined, options),"
        )
        .expect("write operation");
    }
}

fn request_type(kind: SurfaceOperationKind) -> Option<&'static str> {
    match kind {
        SurfaceOperationKind::SingletonRead | SurfaceOperationKind::SingletonDelete => None,
        SurfaceOperationKind::PointRead => Some("SurfacePointRequestJson"),
        SurfaceOperationKind::Page => Some("SurfacePageRequestJson"),
        SurfaceOperationKind::UniqueLookup => Some("SurfaceUniqueLookupRequestJson"),
        SurfaceOperationKind::SingletonUpdate => Some("SurfaceSingletonUpdateRequestJson"),
        SurfaceOperationKind::PointUpdate => Some("SurfacePointUpdateRequestJson"),
        SurfaceOperationKind::SingletonCreate => Some("SurfaceSingletonCreateRequestJson"),
        SurfaceOperationKind::PointCreate => Some("SurfacePointCreateRequestJson"),
        SurfaceOperationKind::PointDelete => Some("SurfacePointDeleteRequestJson"),
        SurfaceOperationKind::Action => Some("SurfaceActionRequestJson"),
        SurfaceOperationKind::ComputedRead => Some("SurfaceComputedReadRequestJson"),
    }
}

fn sanitize_group_identifier(label: &str) -> String {
    sanitize_identifier(label, true)
}

fn sanitize_property_identifier(label: &str) -> String {
    sanitize_identifier(label, false)
}

fn sanitize_identifier(label: &str, avoid_reserved_words: bool) -> String {
    let mut sanitized = String::new();
    for character in label.chars() {
        if sanitized.is_empty() {
            if is_identifier_start(character) {
                sanitized.push(character);
            } else if is_identifier_continue(character) {
                sanitized.push('_');
                sanitized.push(character);
            } else {
                sanitized.push('_');
            }
        } else if is_identifier_continue(character) {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() {
        sanitized.push('_');
    }
    if avoid_reserved_words && is_reserved_word(&sanitized) {
        sanitized.push('_');
    }
    sanitized
}

fn is_identifier_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_' || character == '$'
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit()
}

fn is_reserved_word(word: &str) -> bool {
    matches!(
        word,
        "as" | "async"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "from"
            | "function"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "of"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

fn group_suffix(group: &SurfaceClientGroup) -> String {
    group
        .operations
        .iter()
        .map(|operation| operation_tag_suffix(&operation.operation_tag))
        .min()
        .unwrap_or_else(|| "surface".into())
}

fn operation_tag_suffix(operation_tag: &str) -> String {
    let suffix = operation_tag
        .strip_prefix("sha256:")
        .unwrap_or(operation_tag)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>();
    if suffix.is_empty() {
        "op".into()
    } else {
        suffix
    }
}

fn ts_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

/// The deterministic freshness key for a project's TypeScript client: a SHA-256 over the
/// canonically serialized surface ABI and route manifest. Two checkouts of the same surface
/// shape produce the same digest; a non-surface `.mw` edit leaves it unchanged.
pub fn surface_abi_digest(abi: &SurfaceAbiJson, routes: &SurfaceRouteManifestJson) -> String {
    // The DTOs serialize with fixed struct field order and sorted surfaces, so the bytes are
    // stable across runs and checkouts; the newline separates the two payloads unambiguously.
    let mut bytes = serde_json::to_vec(abi).expect("surface ABI serializes");
    bytes.push(b'\n');
    bytes.extend_from_slice(&serde_json::to_vec(routes).expect("route manifest serializes"));
    marrow_project::sha256_digest(&bytes)
}

/// The do-not-edit + digest header prepended to every generated client.
pub fn surface_client_header(digest: &str) -> String {
    format!("{SURFACE_CLIENT_DO_NOT_EDIT}\n{SURFACE_CLIENT_DIGEST_PREFIX}{digest}\n\n")
}

/// Extract the `sha256:<...>` value from a generated client's header, if present.
pub fn surface_client_header_digest(contents: &str) -> Option<String> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(SURFACE_CLIENT_DIGEST_PREFIX))
        .map(|value| value.trim().to_string())
}

pub const SURFACE_CLIENT_DO_NOT_EDIT: &str = "// Generated by marrow — do not edit.";
pub const SURFACE_CLIENT_DIGEST_PREFIX: &str = "// marrow-surface-digest: ";

const CLIENT_PREAMBLE: &str = r#"const SURFACE_OPERATION_PROFILE_VERSION = "surface.operation.v1";

type MarrowIntInput = string | number | bigint;
type SurfaceScalarKeyJson =
  | { kind: "int"; value: MarrowIntInput }
  | { kind: "bool"; value: boolean }
  | { kind: "string"; value: string }
  | { kind: "date"; days_since_epoch: number }
  | { kind: "duration"; nanos: MarrowIntInput }
  | { kind: "instant"; nanos_since_epoch: MarrowIntInput }
  | { kind: "bytes"; value_b64: string };
type SurfaceKeyJson = SurfaceScalarKeyJson;
type SurfaceArgumentJson =
  | SurfaceScalarKeyJson
  | { kind: "enum"; enum_catalog_id: string; member_catalog_id: string }
  | { kind: "identity"; store_catalog_id: string; keys: SurfaceKeyJson[] };
type SurfaceIdentityJson = { store_catalog_id: string; keys: SurfaceKeyJson[] };
type SurfaceCursorJson = { operation_tag: string; [key: string]: unknown };
type SurfaceWriteValueJson =
  | SurfaceArgumentJson
  | { kind: "decimal"; value: string };
type SurfaceUpdateFieldJson = { catalog_id: string; value: SurfaceWriteValueJson };
type SurfaceCreateFieldJson = { catalog_id: string; value: SurfaceWriteValueJson };
type SurfacePointRequestJson = { identity: SurfaceIdentityJson };
type SurfacePageRequestJson = {
  exact_keys?: SurfaceArgumentJson[];
  limit: number;
  cursor?: SurfaceCursorJson;
};
type SurfaceUniqueLookupRequestJson = { keys: SurfaceArgumentJson[] };
type SurfacePointUpdateRequestJson = { identity: SurfaceIdentityJson; fields: SurfaceUpdateFieldJson[] };
type SurfaceSingletonUpdateRequestJson = { fields: SurfaceUpdateFieldJson[] };
type SurfacePointCreateRequestJson = { identity: SurfaceIdentityJson; fields: SurfaceCreateFieldJson[] };
type SurfaceSingletonCreateRequestJson = { fields: SurfaceCreateFieldJson[] };
type SurfacePointDeleteRequestJson = { identity: SurfaceIdentityJson };
type SurfaceActionRequestJson = { arguments: unknown[] };
type SurfaceComputedReadRequestJson = { arguments: unknown[] };
type SurfaceOperationRequestKind =
  | "singleton_read"
  | "point_read"
  | "page"
  | "unique_lookup"
  | "singleton_update"
  | "point_update"
  | "singleton_create"
  | "point_create"
  | "singleton_delete"
  | "point_delete"
  | "action"
  | "computed_read";
type SurfaceOperationResultKind =
  | "record"
  | "page"
  | "optional_record"
  | "updated"
  | "created"
  | "deleted"
  | "action"
  | "computed_read";
type SurfaceOperationRequestJson = {
  profile_version: typeof SURFACE_OPERATION_PROFILE_VERSION;
  operation_tag: string;
  request: { kind: SurfaceOperationRequestKind; request?: unknown };
};
type SurfaceOperationResponseJson = {
  profile_version: typeof SURFACE_OPERATION_PROFILE_VERSION;
  operation_tag: string;
  result: { kind: SurfaceOperationResultKind; [key: string]: unknown };
};
type SurfaceOperationBinding = {
  readonly operation_tag: string;
  readonly path: string;
  readonly request_kind: SurfaceOperationRequestKind;
  readonly result_kind: SurfaceOperationResultKind;
};
type SurfaceHttpResponse = { ok: boolean; json(): Promise<unknown> };
type SurfaceFetch = (
  input: string,
  init: { method: "POST"; headers: Record<string, string>; body: string },
) => Promise<SurfaceHttpResponse>;
export type MarrowSurfaceClientOptions = {
  baseUrl?: string;
  fetch?: SurfaceFetch;
  headers?: Record<string, string>;
};

function encodeMarrowInt(value: MarrowIntInput): string {
  if (typeof value === "bigint") {
    return value.toString();
  }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) {
      throw new Error("Marrow int number inputs must be safe integers");
    }
    return String(value);
  }
  if (typeof value === "string") {
    return value;
  }
  throw new Error("Marrow int inputs must be string, number, or bigint");
}

function encodeSurfaceJson(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(encodeSurfaceJson);
  }
  if (value === null || typeof value !== "object") {
    return value;
  }
  const record = value as Record<string, unknown>;
  const encoded: Record<string, unknown> = {};
  for (const [key, item] of Object.entries(record)) {
    encoded[key] = encodeSurfaceJson(item);
  }
  if (record.kind === "int" && "value" in record) {
    encoded.value = encodeMarrowInt(record.value as MarrowIntInput);
  }
  if (record.kind === "duration" && "nanos" in record) {
    encoded.nanos = encodeMarrowInt(record.nanos as MarrowIntInput);
  }
  if (record.kind === "instant" && "nanos_since_epoch" in record) {
    encoded.nanos_since_epoch = encodeMarrowInt(record.nanos_since_epoch as MarrowIntInput);
  }
  return encoded;
}

function operationRequest(
  operationTag: string,
  requestKind: SurfaceOperationRequestKind,
  request: unknown,
): SurfaceOperationRequestJson {
  const body: SurfaceOperationRequestJson["request"] = { kind: requestKind };
  if (request !== undefined) {
    body.request = encodeSurfaceJson(request);
  }
  return {
    profile_version: SURFACE_OPERATION_PROFILE_VERSION,
    operation_tag: operationTag,
    request: body,
  };
}

function validateSurfaceResponse(
  response: unknown,
  operationTag: string,
  expectedResultKind: SurfaceOperationResultKind,
): SurfaceOperationResponseJson {
  if (response === null || typeof response !== "object") {
    throw new Error("Marrow surface response must be an object");
  }
  const envelope = response as SurfaceOperationResponseJson;
  if (envelope.profile_version !== SURFACE_OPERATION_PROFILE_VERSION) {
    throw new Error("Marrow surface response profile version mismatch");
  }
  if (envelope.operation_tag !== operationTag) {
    throw new Error("Marrow surface response operation tag mismatch");
  }
  if (!envelope.result || envelope.result.kind !== expectedResultKind) {
    throw new Error("Marrow surface response result kind mismatch");
  }
  return envelope;
}

function surfaceUrl(baseUrl: string | undefined, path: string): string {
  if (!baseUrl) {
    return path;
  }
  return `${baseUrl.replace(/\/$/, "")}${path}`;
}

async function invoke(
  binding: SurfaceOperationBinding,
  request: unknown,
  options: MarrowSurfaceClientOptions,
): Promise<SurfaceOperationResponseJson> {
  const fetcher =
    options.fetch ?? (globalThis as unknown as { fetch?: SurfaceFetch }).fetch;
  if (!fetcher) {
    throw new Error("Marrow surface client requires a fetch implementation");
  }
  const response = await fetcher(surfaceUrl(options.baseUrl, binding.path), {
    method: "POST",
    headers: { "Content-Type": "application/json", ...(options.headers ?? {}) },
    body: JSON.stringify(operationRequest(binding.operation_tag, binding.request_kind, request)),
  });
  const json = await response.json();
  if (!response.ok) {
    throw json;
  }
  return validateSurfaceResponse(json, binding.operation_tag, binding.result_kind);
}

"#;
