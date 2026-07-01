use std::fmt::Write;

use marrow_run::{
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_ACTION, SURFACE_COMPUTED, SURFACE_CONFLICT,
    SURFACE_CURSOR, SURFACE_INVALID_DATA, SURFACE_LIMIT, SURFACE_REQUEST, SURFACE_STALE_CURSOR,
    SURFACE_STORE, SURFACE_WRITE,
};

use super::client_model::{RecordDecode, ScalarKind};
use super::{
    SurfaceAbiJson, SurfaceClientModel, SurfaceClientRecord, SurfaceClientStore, SurfaceFieldType,
    SurfaceMethod, SurfaceMethodInput, SurfaceMethodParam, SurfaceMethodResult,
    SurfaceOperationCatalog, SurfaceOperationCatalogError, SurfaceRecordField,
    SurfaceRouteBindingError, SurfaceRouteBindings, SurfaceRouteManifestJson,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceClientCursorProfile {
    Typed,
    Token,
}

impl SurfaceClientCursorProfile {
    fn client_profile(self) -> &'static str {
        match self {
            Self::Typed => SURFACE_CLIENT_PROFILE,
            Self::Token => SURFACE_CLIENT_TOKEN_CURSOR_PROFILE,
        }
    }
}

/// The closed set of public `surface.*` error codes a generated client may surface, sourced from the
/// runtime constants so the union never drifts from the wire codes the server emits.
const SURFACE_ERROR_CODES: &[&str] = &[
    SURFACE_REQUEST,
    SURFACE_ABSENT,
    SURFACE_CURSOR,
    SURFACE_STALE_CURSOR,
    SURFACE_ABI_MISMATCH,
    SURFACE_INVALID_DATA,
    SURFACE_LIMIT,
    SURFACE_CONFLICT,
    SURFACE_WRITE,
    SURFACE_ACTION,
    SURFACE_COMPUTED,
    SURFACE_STORE,
];

pub fn render_typescript_client(
    abi: &SurfaceAbiJson,
    routes: &SurfaceRouteManifestJson,
) -> Result<String, SurfaceClientRenderError> {
    render_typescript_client_with_cursor_profile(abi, routes, SurfaceClientCursorProfile::Typed)
}

pub fn render_typescript_client_with_cursor_profile(
    abi: &SurfaceAbiJson,
    routes: &SurfaceRouteManifestJson,
    cursor_profile: SurfaceClientCursorProfile,
) -> Result<String, SurfaceClientRenderError> {
    let catalog = SurfaceOperationCatalog::from_abi(abi).map_err(SurfaceClientRenderError::from)?;
    let bindings = SurfaceRouteBindings::from_manifest_for_client(routes, &catalog)
        .map_err(SurfaceClientRenderError::from)?;
    let model = SurfaceClientModel::build(abi, &bindings);
    let surface_digest = surface_abi_digest(abi, routes);
    let header = surface_client_header_with_cursor_profile(
        &surface_digest,
        &surface_client_digest_from_surface(&surface_digest, cursor_profile),
        cursor_profile,
    );
    Ok(render_model(&header, &model, cursor_profile))
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

fn render_model(
    header: &str,
    model: &SurfaceClientModel,
    cursor_profile: SurfaceClientCursorProfile,
) -> String {
    let mut output = String::new();
    output.push_str(header);
    output.push_str(CLIENT_PREAMBLE);
    render_error_codes(&mut output);
    render_route_tables(&mut output, model);
    render_catalog_constants(&mut output, model);
    render_stores(&mut output, model);
    render_surfaces(&mut output, model, cursor_profile);
    render_resources(&mut output, model);
    render_create_client(&mut output, model);
    output
}

fn render_resources(output: &mut String, model: &SurfaceClientModel) {
    for resource in &model.resources {
        render_record_type(output, resource);
        render_record_decoder(output, resource);
    }
}

/// Emit the per-operation request-kind and route-prefix lookups the transport uses to build a wire
/// request and POST path from an operation tag. Operation identity stays the tag; these are render
/// data, not a second classifier.
fn render_route_tables(output: &mut String, model: &SurfaceClientModel) {
    output.push_str("const REQUEST_KIND_BY_TAG: Record<string, string> = {\n");
    for route in &model.routes {
        writeln!(
            output,
            "  {}: {},",
            ts_string(&route.operation_tag),
            ts_string(route.request_kind)
        )
        .expect("write request kind");
    }
    output.push_str("};\n\n");
    output.push_str("const ROUTE_PREFIX_BY_TAG: Record<string, string> = {\n");
    for route in &model.routes {
        writeln!(
            output,
            "  {}: {},",
            ts_string(&route.operation_tag),
            ts_string(route.route_prefix)
        )
        .expect("write route prefix");
    }
    output.push_str("};\n\n");
}

fn render_error_codes(output: &mut String) {
    output.push_str("export type SurfaceErrorCode =\n");
    for (index, code) in SURFACE_ERROR_CODES.iter().enumerate() {
        let separator = if index + 1 == SURFACE_ERROR_CODES.len() {
            ";"
        } else {
            ""
        };
        writeln!(output, "  | {}{separator}", ts_string(code)).expect("write error code");
    }
    output.push('\n');
}

/// Emit the private catalog-id constants: enum member-id tables and store catalog ids. These keep
/// every catalog id out of user-facing signatures while letting decode/encode key on the stable id.
fn render_catalog_constants(output: &mut String, model: &SurfaceClientModel) {
    for store in &model.stores {
        writeln!(
            output,
            "const {} = {};",
            store.catalog_const,
            ts_string(&store.store_catalog_id)
        )
        .expect("write store catalog id");
    }
    if !model.stores.is_empty() {
        output.push('\n');
    }
    for enumeration in &model.enums {
        writeln!(
            output,
            "const {} = {};",
            enumeration.enum_catalog_const,
            ts_string(&enumeration.enum_catalog_id)
        )
        .expect("write enum catalog id");
        writeln!(output, "const {} = {{", enumeration.member_table_const)
            .expect("write enum table");
        for member in &enumeration.members {
            writeln!(
                output,
                "  {}: {},",
                ts_string(&member.member_catalog_id),
                ts_string(&member.label)
            )
            .expect("write enum member");
        }
        output.push_str("} as const;\n");
        writeln!(
            output,
            "const {} = invertMembers({});\n",
            enumeration.encode_table_const, enumeration.member_table_const
        )
        .expect("write enum encode table");
    }
}

fn render_stores(output: &mut String, model: &SurfaceClientModel) {
    for store in &model.stores {
        render_store(output, store);
    }
}

fn render_store(output: &mut String, store: &SurfaceClientStore) {
    let brand = &store.brand_type;
    writeln!(
        output,
        "export type {brand} = {{ readonly __brand: \"{brand}\"; readonly __store: string; readonly keys: SurfaceKeyJson[] }};"
    )
    .expect("write brand type");
    let params = store
        .key_scalars
        .iter()
        .enumerate()
        .map(|(index, scalar)| format!("key{index}: {}", scalar_input_type(*scalar)))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "export function {}({params}): {brand} {{",
        store.constructor
    )
    .expect("write constructor signature");
    output.push_str("  return {\n");
    writeln!(output, "    __brand: \"{brand}\",").expect("write brand tag");
    writeln!(output, "    __store: {},", store.catalog_const).expect("write brand store");
    output.push_str("    keys: [");
    let key_exprs = store
        .key_scalars
        .iter()
        .enumerate()
        .map(|(index, scalar)| request_scalar_expr(*scalar, &format!("key{index}")))
        .collect::<Vec<_>>()
        .join(", ");
    output.push_str(&key_exprs);
    output.push_str("],\n");
    output.push_str("  };\n}\n");
    writeln!(
        output,
        "function {}(keys: SurfaceKeyJson[]): {brand} {{ return {{ __brand: \"{brand}\", __store: {}, keys }}; }}\n",
        store.decode_fn, store.catalog_const
    )
    .expect("write brand decoder");
}

fn render_surfaces(
    output: &mut String,
    model: &SurfaceClientModel,
    cursor_profile: SurfaceClientCursorProfile,
) {
    for surface in &model.surfaces {
        if let Some(cursor_brand) = &surface.page_cursor_brand {
            render_cursor_brand(output, cursor_brand, cursor_profile);
        }
        for enumeration in surface.enums.iter() {
            render_enum_type(output, model, enumeration);
        }
        for record in &surface.records {
            render_record_type(output, record);
            if record.decode == RecordDecode::SurfaceRead {
                render_record_decoder(output, record);
            }
        }
    }
}

/// Emit a surface's opaque page-cursor brand. The cursor is preserved verbatim across a page round
/// trip, so the brand is a distinct nominal alias over the wire cursor shape, not a decoded value.
fn render_cursor_brand(
    output: &mut String,
    cursor_brand: &str,
    cursor_profile: SurfaceClientCursorProfile,
) {
    let base = match cursor_profile {
        SurfaceClientCursorProfile::Typed => "SurfaceCursorJson",
        SurfaceClientCursorProfile::Token => "string",
    };
    writeln!(
        output,
        "export type {cursor_brand} = {base} & {{ readonly __brand: \"{cursor_brand}\" }};\n"
    )
    .expect("write cursor brand");
}

fn render_enum_type(output: &mut String, model: &SurfaceClientModel, enum_const: &str) {
    let enumeration = model
        .enums
        .iter()
        .find(|enumeration| enumeration.member_table_const == enum_const)
        .expect("enum model present");
    writeln!(output, "export type {} =", enumeration.type_name).expect("write enum union head");
    for (index, member) in enumeration.members.iter().enumerate() {
        let separator = if index + 1 == enumeration.members.len() {
            ";"
        } else {
            ""
        };
        writeln!(output, "  | {}{separator}", ts_string(&member.label)).expect("write enum member");
    }
    output.push('\n');
}

fn render_record_type(output: &mut String, record: &SurfaceClientRecord) {
    writeln!(output, "export type {} = {{", record.type_name).expect("write record type head");
    for field in &record.fields {
        let nullable = if field.required { "" } else { " | null" };
        let key_suffix = if field.optional { "?" } else { "" };
        writeln!(
            output,
            "  {}{key_suffix}: {}{nullable};",
            ts_property(&field.label),
            value_ts_type(&field.ty)
        )
        .expect("write record field");
    }
    output.push_str("};\n\n");
}

/// Emit a `decode<Type>` that maps a wire record onto the typed record. A keyed surface record reads
/// its `id` from the record identity and its values from `record.fields`; a keyless singleton record
/// reads only its values; a resource record reads every field from the resource value's `fields`.
/// All index a field by its stable catalog id and fail loud on a missing required field, so a
/// malformed wire shape throws rather than mis-decodes.
fn render_record_decoder(output: &mut String, record: &SurfaceClientRecord) {
    let source_type = match record.decode {
        RecordDecode::SurfaceRead => "SurfaceRecordWireJson",
        RecordDecode::ResourceValue => "SurfaceResourceWireJson",
        RecordDecode::InputOnly => panic!("input-only body records have no decoder"),
    };
    writeln!(
        output,
        "function decode{}(record: {source_type}): {} {{",
        record.type_name, record.type_name
    )
    .expect("write decoder head");
    output.push_str("  const fields = fieldsByCatalogId(record.fields);\n");
    output.push_str("  return {\n");
    for field in &record.fields {
        let value = record_field_decode_expr(field, record.decode);
        writeln!(output, "    {}: {value},", ts_property(&field.label))
            .expect("write field decode");
    }
    output.push_str("  };\n}\n\n");
}

fn record_field_decode_expr(field: &SurfaceRecordField, decode: RecordDecode) -> String {
    let Some(member_catalog_id) = &field.member_catalog_id else {
        // The synthetic identity field is the only member-less field, present only on a keyed
        // surface record, and decodes from the record identity rather than a projected value.
        debug_assert!(
            decode == RecordDecode::SurfaceRead,
            "only a keyed surface record carries an identity field"
        );
        let SurfaceFieldType::Identity { decode_fn, .. } = &field.ty else {
            panic!("identity field must have an identity type");
        };
        return format!("{decode_fn}(record.identity.keys)");
    };
    let present = format!("presentField(fields, {})", ts_string(member_catalog_id));
    let value_decode = decode_value_expr(&field.ty, "raw");
    if field.required {
        format!("((raw) => {value_decode})(requiredValue({present}))")
    } else {
        format!("optionalValue({present}, (raw) => {value_decode})")
    }
}

fn render_create_client(output: &mut String, model: &SurfaceClientModel) {
    output.push_str("export function createClient(options: MarrowSurfaceClientOptions = {}) {\n");
    output.push_str("  const transport = makeTransport(options);\n");
    output.push_str("  return {\n");
    for surface in &model.surfaces {
        writeln!(output, "    {}: {{", ts_property(&surface.name)).expect("write surface key");
        for method in &surface.methods {
            render_method(output, method);
        }
        output.push_str("    },\n");
    }
    output.push_str("  };\n}\n");
}

fn render_method(output: &mut String, method: &SurfaceMethod) {
    if matches!(method.result, SurfaceMethodResult::PageIterator { .. }) {
        render_page_iterator_method(output, method);
        return;
    }
    let signature = method_signature(method);
    let request_expr = method_request_expr(method);
    let decode_expr = method_decode_expr(method);
    writeln!(
        output,
        "      {}: async ({signature}): Promise<{}> => {{",
        ts_property(&method.name),
        method_result_type(method)
    )
    .expect("write method head");
    writeln!(
        output,
        "        const envelope = await transport.invoke({}, {}, {request_expr});",
        ts_string(&method.operation_tag),
        ts_string(method.result_kind)
    )
    .expect("write method invoke");
    writeln!(output, "        return {decode_expr};").expect("write method decode");
    output.push_str("      },\n");
}

fn render_page_iterator_method(output: &mut String, method: &SurfaceMethod) {
    let signature = method_signature(method);
    let request_expr = method_request_expr(method);
    let decode_expr = method_decode_expr(method);
    writeln!(
        output,
        "      {}: async function* ({signature}): {} {{",
        ts_property(&method.name),
        method_result_type(method)
    )
    .expect("write page iterator method head");
    output.push_str("        let cursor = options.initialCursor ?? null;\n");
    output.push_str("        while (true) {\n");
    writeln!(
        output,
        "          const envelope = await transport.invoke({}, {}, {request_expr});",
        ts_string(&method.operation_tag),
        ts_string(method.result_kind)
    )
    .expect("write page iterator invoke");
    writeln!(output, "          const page = {decode_expr};").expect("write page iterator decode");
    output.push_str("          yield page;\n");
    output.push_str("          if (page.next === null) {\n");
    output.push_str("            return;\n");
    output.push_str("          }\n");
    output.push_str("          cursor = page.next;\n");
    output.push_str("        }\n");
    output.push_str("      },\n");
}

fn method_signature(method: &SurfaceMethod) -> String {
    match &method.input {
        SurfaceMethodInput::None | SurfaceMethodInput::SingletonDelete => String::new(),
        SurfaceMethodInput::Identity { brand } => format!("id: {brand}"),
        SurfaceMethodInput::Delete { brand } => format!("id: {brand}"),
        SurfaceMethodInput::Create {
            brand, body_type, ..
        } => {
            format!("id: {brand}, body: {body_type}")
        }
        SurfaceMethodInput::SingletonCreate { body_type, .. } => format!("body: {body_type}"),
        SurfaceMethodInput::Update {
            brand, body_type, ..
        } => {
            format!("id: {brand}, body: {body_type}")
        }
        SurfaceMethodInput::SingletonUpdate { body_type, .. } => format!("body: {body_type}"),
        SurfaceMethodInput::Page { exact_keys } => page_signature(exact_keys, &method.cursor_brand),
        SurfaceMethodInput::PageIterator { exact_keys } => {
            page_iterator_signature(exact_keys, &method.cursor_brand)
        }
        SurfaceMethodInput::UniqueLookup { keys } => keys
            .iter()
            .enumerate()
            .map(|(index, ty)| format!("key{index}: {}", argument_ts_type(ty)))
            .collect::<Vec<_>>()
            .join(", "),
        SurfaceMethodInput::Callable { params } => callable_signature(params),
    }
}

fn page_signature(exact_keys: &[SurfaceFieldType], cursor_brand: &str) -> String {
    let exact = exact_keys
        .iter()
        .enumerate()
        .map(|(index, ty)| format!("exactKey{index}: {}", argument_ts_type(ty)))
        .collect::<Vec<_>>();
    let mut params = exact;
    params.push("limit: number".to_string());
    params.push(format!("cursor?: {cursor_brand} | null"));
    params.join(", ")
}

fn page_iterator_signature(exact_keys: &[SurfaceMethodParam], cursor_brand: &str) -> String {
    let mut params = exact_keys
        .iter()
        .map(|param| format!("{}: {}", param.name, argument_ts_type(&param.ty)))
        .collect::<Vec<_>>();
    params.push(format!(
        "options: {{ limit: number; initialCursor?: {cursor_brand} | null }}"
    ));
    params.join(", ")
}

fn callable_signature(params: &[(String, SurfaceFieldType)]) -> String {
    params
        .iter()
        .map(|(name, ty)| format!("{}: {}", ts_property(name), argument_ts_type(ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn method_request_expr(method: &SurfaceMethod) -> String {
    match &method.input {
        SurfaceMethodInput::None => "undefined".to_string(),
        // A keyless singleton delete carries the closed empty request body, never an identity.
        SurfaceMethodInput::SingletonDelete => "{}".to_string(),
        SurfaceMethodInput::Identity { .. } | SurfaceMethodInput::Delete { .. } => {
            "{ identity: identityFromBrand(id) }".to_string()
        }
        SurfaceMethodInput::Create { fields, .. } => format!(
            "{{ identity: identityFromBrand(id), fields: {} }}",
            create_fields_expr(fields)
        ),
        SurfaceMethodInput::SingletonCreate { fields, .. } => {
            format!("{{ fields: {} }}", create_fields_expr(fields))
        }
        SurfaceMethodInput::Update { fields, .. } => format!(
            "{{ identity: identityFromBrand(id), fields: {} }}",
            update_fields_expr(fields)
        ),
        SurfaceMethodInput::SingletonUpdate { fields, .. } => {
            format!("{{ fields: {} }}", update_fields_expr(fields))
        }
        SurfaceMethodInput::Page { exact_keys } => page_request_expr(exact_keys),
        SurfaceMethodInput::PageIterator { exact_keys } => page_iterator_request_expr(exact_keys),
        SurfaceMethodInput::UniqueLookup { keys } => {
            let keys = keys
                .iter()
                .enumerate()
                .map(|(index, ty)| request_value_expr(ty, &format!("key{index}")))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ keys: [{keys}] }}")
        }
        SurfaceMethodInput::Callable { params } => callable_request_expr(params),
    }
}

fn create_fields_expr(fields: &[CreateFieldPlan]) -> String {
    let entries = fields
        .iter()
        .map(|field| {
            format!(
                "{{ catalog_id: {}, value: {} }}",
                ts_string(&field.member_catalog_id),
                request_value_expr(&field.ty, &format!("body[{}]", ts_string(&field.label)))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{entries}]")
}

/// The sparse-update patch: a field is encoded into the request only when the caller provided it,
/// so an omitted field is preserved server-side rather than forced through a read-modify-write. Each
/// present field spreads in its single encoded entry, keeping the patch typed with no `any`.
fn update_fields_expr(fields: &[CreateFieldPlan]) -> String {
    let entries = fields
        .iter()
        .map(|field| {
            let source = format!("body[{}]", ts_string(&field.label));
            format!(
                "...({source} !== undefined ? [{{ catalog_id: {}, value: {} }}] : [])",
                ts_string(&field.member_catalog_id),
                request_value_expr(&field.ty, &source)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{entries}]")
}

fn page_request_expr(exact_keys: &[SurfaceFieldType]) -> String {
    let mut parts = Vec::new();
    if !exact_keys.is_empty() {
        let keys = exact_keys
            .iter()
            .enumerate()
            .map(|(index, ty)| request_value_expr(ty, &format!("exactKey{index}")))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("exact_keys: [{keys}]"));
    }
    parts.push("limit".to_string());
    parts.push("cursor: cursor ?? undefined".to_string());
    format!("{{ {} }}", parts.join(", "))
}

fn page_iterator_request_expr(exact_keys: &[SurfaceMethodParam]) -> String {
    let mut parts = Vec::new();
    if !exact_keys.is_empty() {
        let keys = exact_keys
            .iter()
            .map(|param| request_value_expr(&param.ty, &param.name))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("exact_keys: [{keys}]"));
    }
    parts.push("limit: options.limit".to_string());
    parts.push("cursor: cursor ?? undefined".to_string());
    format!("{{ {} }}", parts.join(", "))
}

fn callable_request_expr(params: &[(String, SurfaceFieldType)]) -> String {
    let entries = params
        .iter()
        .map(|(name, ty)| {
            format!(
                "{{ name: {}, value: {} }}",
                ts_string(name),
                entry_argument_expr(ty, &ts_property(name))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{ arguments: [{entries}] }}")
}

fn method_decode_expr(method: &SurfaceMethod) -> String {
    match &method.result {
        SurfaceMethodResult::Record { record } => {
            format!("decode{}(recordOf(envelope))", record)
        }
        SurfaceMethodResult::OptionalRecord { record } => {
            format!("optionalRecordOf(envelope, decode{record})")
        }
        SurfaceMethodResult::Page { record } => {
            format!(
                "pageOf<{record}, {}>(envelope, decode{record})",
                method.cursor_brand
            )
        }
        SurfaceMethodResult::PageIterator { record } => {
            format!(
                "pageOf<{record}, {}>(envelope, decode{record})",
                method.cursor_brand
            )
        }
        SurfaceMethodResult::Created { record } => {
            format!("decode{}(recordOf(envelope))", record)
        }
        SurfaceMethodResult::Updated | SurfaceMethodResult::Deleted => "undefined".to_string(),
        SurfaceMethodResult::Action { value } => match value {
            Some(ty) => {
                let value_decode = decode_value_expr(ty, "value");
                format!("actionResultValue(envelope, (value) => {value_decode})")
            }
            None => "actionResultVoid(envelope)".to_string(),
        },
        SurfaceMethodResult::ComputedRead { value } => match value {
            Some(ty) => {
                let value_decode = decode_value_expr(ty, "value");
                format!("computedReadValue(envelope, (value) => {value_decode})")
            }
            None => "computedReadVoid(envelope)".to_string(),
        },
    }
}

fn method_result_type(method: &SurfaceMethod) -> String {
    match &method.result {
        SurfaceMethodResult::Record { record } | SurfaceMethodResult::Created { record } => {
            record.clone()
        }
        SurfaceMethodResult::OptionalRecord { record } => format!("{record} | null"),
        SurfaceMethodResult::Page { record } => format!("Page<{record}, {}>", method.cursor_brand),
        SurfaceMethodResult::PageIterator { record } => {
            format!("AsyncIterable<Page<{record}, {}>>", method.cursor_brand)
        }
        SurfaceMethodResult::Updated | SurfaceMethodResult::Deleted => "void".to_string(),
        SurfaceMethodResult::Action { value } => {
            let value_type = value
                .as_ref()
                .map(value_ts_type)
                .unwrap_or_else(|| "null".to_string());
            format!("{{ value: {value_type}; output: string }}")
        }
        SurfaceMethodResult::ComputedRead { value } => value
            .as_ref()
            .map(value_ts_type)
            .unwrap_or_else(|| "null".to_string()),
    }
}

fn argument_ts_type(ty: &SurfaceFieldType) -> String {
    match ty {
        SurfaceFieldType::Scalar(scalar) => scalar_input_type(*scalar).to_string(),
        SurfaceFieldType::Enum { type_name, .. } => type_name.clone(),
        SurfaceFieldType::Identity { brand, .. } => brand.clone(),
        SurfaceFieldType::Sequence(inner) => format!("{}[]", argument_ts_type(inner)),
        SurfaceFieldType::Resource { type_name, .. } => type_name.clone(),
    }
}

fn value_ts_type(ty: &SurfaceFieldType) -> String {
    match ty {
        SurfaceFieldType::Scalar(scalar) => scalar_output_type(*scalar).to_string(),
        SurfaceFieldType::Enum { type_name, .. } => type_name.clone(),
        SurfaceFieldType::Identity { brand, .. } => brand.clone(),
        SurfaceFieldType::Sequence(inner) => format!("{}[]", value_ts_type(inner)),
        SurfaceFieldType::Resource { type_name, .. } => type_name.clone(),
    }
}

fn decode_value_expr(ty: &SurfaceFieldType, source: &str) -> String {
    match ty {
        SurfaceFieldType::Scalar(scalar) => decode_scalar_expr(*scalar, source),
        SurfaceFieldType::Enum {
            type_name,
            encode_table,
            ..
        } => {
            format!("decodeEnumValue<{type_name}>({source}, {encode_table})")
        }
        SurfaceFieldType::Identity { decode_fn, .. } => {
            format!("decodeIdentityValue({source}, {decode_fn})")
        }
        SurfaceFieldType::Sequence(inner) => {
            let element = decode_value_expr(inner, "item");
            format!("decodeSequence({source}, (item) => {element})")
        }
        SurfaceFieldType::Resource { decode_fn, .. } => {
            format!("{decode_fn}(resourceValueOf({source}))")
        }
    }
}

fn decode_scalar_expr(scalar: ScalarKind, source: &str) -> String {
    match scalar {
        ScalarKind::Int => format!("decodeIntValue({source})"),
        ScalarKind::Bool => format!("decodeBoolValue({source})"),
        ScalarKind::String => format!("decodeStringValue({source})"),
        ScalarKind::Decimal => format!("decodeWireScalar({source}, \"decimal\")"),
        ScalarKind::Bytes => format!("decodeWireScalar({source}, \"bytes\")"),
        ScalarKind::Date => format!("decodeDateValue({source})"),
        ScalarKind::Instant => format!("decodeNanosValue({source}, \"instant\")"),
        ScalarKind::Duration => format!("decodeNanosValue({source}, \"duration\")"),
    }
}

/// Encode a typed value into the request shape used by write fields, index exact-keys, and
/// unique-lookup keys. The server decodes all three against `SurfaceWriteValueJson` /
/// `SurfaceArgumentJson`, which carry the enum catalog id and per-scalar field names.
fn request_value_expr(ty: &SurfaceFieldType, source: &str) -> String {
    match ty {
        SurfaceFieldType::Scalar(scalar) => request_scalar_expr(*scalar, source),
        SurfaceFieldType::Enum {
            enum_catalog_const,
            encode_table,
            ..
        } => {
            format!("encodeEnum({source}, {enum_catalog_const}, {encode_table})")
        }
        SurfaceFieldType::Identity { .. } => format!("encodeIdentity({source})"),
        SurfaceFieldType::Sequence(_) | SurfaceFieldType::Resource { .. } => {
            // Create/update bodies never carry sequence or nested-resource leaves on a store field;
            // index and lookup keys are always scalar, enum, or identity.
            format!("encodeWriteValue({source})")
        }
    }
}

/// Encode a typed value into the entry argument shape an action or computed read decodes. This is a
/// distinct wire contract from the request shape: enums tag the bare member under `enum_member`, and
/// temporal/bytes scalars carry their datum under a uniform `value` field.
fn entry_argument_expr(ty: &SurfaceFieldType, source: &str) -> String {
    match ty {
        SurfaceFieldType::Scalar(scalar) => entry_scalar_expr(*scalar, source),
        SurfaceFieldType::Enum { encode_table, .. } => {
            format!("encodeEnumMember({source}, {encode_table})")
        }
        SurfaceFieldType::Identity { .. } => format!("encodeIdentityArgument({source})"),
        SurfaceFieldType::Sequence(inner) => {
            let element = entry_argument_expr(inner, "item");
            format!("{{ kind: \"sequence\", value: {source}.map((item) => {element}) }}")
        }
        SurfaceFieldType::Resource { .. } => {
            panic!("resource arguments are not part of the entry argument surface")
        }
    }
}

/// Encode a scalar leaf into the request shape (`SurfaceWriteValueJson` / `SurfaceArgumentJson`),
/// which names each temporal/bytes field explicitly. `SurfaceKeyJson` agrees on these field names,
/// so identity keys reuse this encoder. A `date` key takes the faithful day count, `instant`/
/// `duration` their nanosecond count, and `bytes` its base64 text; `decimal` reaches this encoder
/// only through a write field, never a key.
fn request_scalar_expr(scalar: ScalarKind, source: &str) -> String {
    match scalar {
        ScalarKind::Int => format!("intKey({source})"),
        ScalarKind::Bool => format!("boolKey({source})"),
        ScalarKind::String => format!("stringKey({source})"),
        ScalarKind::Decimal => format!("{{ kind: \"decimal\", value: {source} }}"),
        ScalarKind::Date => format!("dateKey({source})"),
        ScalarKind::Duration => format!("durationKey({source})"),
        ScalarKind::Instant => format!("instantKey({source})"),
        ScalarKind::Bytes => format!("bytesKey({source})"),
    }
}

/// Encode a scalar leaf into the entry argument shape an action or computed read decodes, which
/// carries every scalar datum under a uniform `value` field. The decoder reads the value's canonical
/// datum, so a `date` argument sends canonical `YYYY-MM-DD` text built from its day count, a `bytes`
/// argument sends hex built from its base64 text, and the remaining kinds send their value directly.
fn entry_scalar_expr(scalar: ScalarKind, source: &str) -> String {
    match scalar {
        ScalarKind::Int => format!("{{ kind: \"int\", value: encodeMarrowInt({source}) }}"),
        ScalarKind::Bool => format!("{{ kind: \"bool\", value: {source} }}"),
        ScalarKind::Instant => {
            format!("{{ kind: \"instant\", value: encodeMarrowInt({source}) }}")
        }
        ScalarKind::Duration => {
            format!("{{ kind: \"duration\", value: encodeMarrowInt({source}) }}")
        }
        ScalarKind::Date => format!("{{ kind: \"date\", value: dateText(Number({source})) }}"),
        ScalarKind::Bytes => format!("{{ kind: \"bytes\", value: base64ToHex({source}) }}"),
        ScalarKind::String | ScalarKind::Decimal => {
            format!("{{ kind: {}, value: {source} }}", ts_string(scalar.name()))
        }
    }
}

/// The TypeScript input type for a scalar key or write field. Temporal keys take their faithful wire
/// datum as the brand and request encoders store it without lossy conversion: a `date` is its day
/// count, an `instant`/`duration` its nanosecond count (both `MarrowIntInput`, since the count can
/// exceed 2^53), and `bytes` its base64 text.
fn scalar_input_type(scalar: ScalarKind) -> &'static str {
    match scalar {
        ScalarKind::Int | ScalarKind::Date | ScalarKind::Instant | ScalarKind::Duration => {
            "MarrowIntInput"
        }
        ScalarKind::Bool => "boolean",
        ScalarKind::String | ScalarKind::Decimal | ScalarKind::Bytes => "string",
    }
}

/// The TypeScript type a decoded response value lands in. A `date` is its day count (an i32 number),
/// an `instant`/`duration` its nanosecond count as a bigint (the count can exceed 2^53), `decimal`
/// its canonical text, and `bytes` its base64 text.
fn scalar_output_type(scalar: ScalarKind) -> &'static str {
    match scalar {
        ScalarKind::Int | ScalarKind::Instant | ScalarKind::Duration => "bigint",
        ScalarKind::Bool => "boolean",
        ScalarKind::Date => "number",
        ScalarKind::String | ScalarKind::Decimal | ScalarKind::Bytes => "string",
    }
}

#[derive(Debug, Clone)]
pub(super) struct CreateFieldPlan {
    pub label: String,
    pub member_catalog_id: String,
    pub ty: SurfaceFieldType,
}

fn ts_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

/// Render an object-property key: a bare identifier when the label is already a safe JS identifier,
/// otherwise a quoted string so arbitrary render labels stay valid TypeScript.
fn ts_property(label: &str) -> String {
    if is_plain_identifier(label) {
        label.to_string()
    } else {
        ts_string(label)
    }
}

fn is_plain_identifier(label: &str) -> bool {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' || first == '$' => {}
        _ => return false,
    }
    chars.all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '$')
}

/// The deterministic ABI/route identity for a surface: a SHA-256 over the canonically serialized
/// surface ABI and route manifest. Two checkouts of the same surface shape produce the same digest;
/// a non-surface `.mw` edit leaves it unchanged.
pub fn surface_abi_digest(abi: &SurfaceAbiJson, routes: &SurfaceRouteManifestJson) -> String {
    let mut bytes = serde_json::to_vec(abi).expect("surface ABI serializes");
    bytes.push(b'\n');
    bytes.extend_from_slice(&serde_json::to_vec(routes).expect("route manifest serializes"));
    marrow_project::sha256_digest(&bytes)
}

/// The deterministic freshness key for the generated TypeScript client profile over a surface.
pub fn surface_client_digest(abi: &SurfaceAbiJson, routes: &SurfaceRouteManifestJson) -> String {
    surface_client_digest_with_cursor_profile(abi, routes, SurfaceClientCursorProfile::Typed)
}

pub fn surface_client_digest_with_cursor_profile(
    abi: &SurfaceAbiJson,
    routes: &SurfaceRouteManifestJson,
    cursor_profile: SurfaceClientCursorProfile,
) -> String {
    surface_client_digest_from_surface(&surface_abi_digest(abi, routes), cursor_profile)
}

fn surface_client_digest_from_surface(
    surface_digest: &str,
    cursor_profile: SurfaceClientCursorProfile,
) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(cursor_profile.client_profile().as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(surface_digest.as_bytes());
    marrow_project::sha256_digest(&bytes)
}

/// The do-not-edit + profile/digest header prepended to every generated client.
pub fn surface_client_header(surface_digest: &str, client_digest: &str) -> String {
    surface_client_header_with_cursor_profile(
        surface_digest,
        client_digest,
        SurfaceClientCursorProfile::Typed,
    )
}

fn surface_client_header_with_cursor_profile(
    surface_digest: &str,
    client_digest: &str,
    cursor_profile: SurfaceClientCursorProfile,
) -> String {
    let client_profile = cursor_profile.client_profile();
    format!(
        "{SURFACE_CLIENT_DO_NOT_EDIT}\n{SURFACE_CLIENT_PROFILE_PREFIX}{client_profile}\n{SURFACE_ABI_DIGEST_PREFIX}{surface_digest}\n{SURFACE_CLIENT_DIGEST_PREFIX}{client_digest}\n\n"
    )
}

/// Extract the generated-client `sha256:<...>` freshness value from the header, if present.
pub fn surface_client_header_digest(contents: &str) -> Option<String> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(SURFACE_CLIENT_DIGEST_PREFIX))
        .map(|value| value.trim().to_string())
}

pub const SURFACE_CLIENT_DO_NOT_EDIT: &str = "// Generated by marrow — do not edit.";
pub const SURFACE_CLIENT_PROFILE: &str = "typescript.v2";
pub const SURFACE_CLIENT_TOKEN_CURSOR_PROFILE: &str = "typescript.v2+surface.cursor_token.v1";
pub const SURFACE_CLIENT_PROFILE_PREFIX: &str = "// marrow-client-profile: ";
pub const SURFACE_ABI_DIGEST_PREFIX: &str = "// marrow-surface-digest: ";
pub const SURFACE_CLIENT_DIGEST_PREFIX: &str = "// marrow-client-digest: ";

const CLIENT_PREAMBLE: &str = include_str!("client_ts_preamble.ts");
