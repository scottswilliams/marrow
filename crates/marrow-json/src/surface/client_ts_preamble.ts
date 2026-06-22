const SURFACE_OPERATION_PROFILE_VERSION = "surface.operation.v1";

export type MarrowIntInput = string | number | bigint;

type SurfaceKeyJson =
  | { kind: "int"; value: string }
  | { kind: "bool"; value: boolean }
  | { kind: "string"; value: string }
  | { kind: "date"; days_since_epoch: number }
  | { kind: "duration"; nanos: string }
  | { kind: "instant"; nanos_since_epoch: string }
  | { kind: "bytes"; value_b64: string };
type SurfaceWireValueJson = { kind: string; [key: string]: unknown };
type SurfaceFieldWireJson = { catalog_id: string; value: SurfaceWireValueJson | null };
type SurfaceIdentityWireJson = { store_catalog_id: string; keys: SurfaceKeyJson[] };
type SurfaceRecordWireJson = {
  identity: SurfaceIdentityWireJson;
  fields: SurfaceFieldWireJson[];
};
type SurfaceResourceFieldWireJson = {
  member_catalog_id: string;
  value: SurfaceWireValueJson | null;
};
type SurfaceResourceWireJson = { fields: SurfaceResourceFieldWireJson[] };
type SurfaceCursorJson = { operation_tag: string; [key: string]: unknown };

export type Page<Row, Cursor> = { rows: Row[]; next: Cursor | null };

type SurfaceOperationResponseJson = {
  profile_version: string;
  operation_tag: string;
  result: { kind: string; [key: string]: unknown };
};
type SurfaceErrorBodyJson = { code?: unknown; message?: unknown };

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

/// A typed surface fault. `code` is one of the closed `surface.*` codes; branch on it rather than on
/// the HTTP status or the human `message`. `rawBody` keeps the unparsed envelope for diagnostics.
export class MarrowSurfaceError extends Error {
  readonly code: SurfaceErrorCode;
  readonly rawBody: unknown;
  constructor(code: SurfaceErrorCode, message: string, rawBody: unknown) {
    super(message);
    this.name = "MarrowSurfaceError";
    this.code = code;
    this.rawBody = rawBody;
  }
}

export function isMarrowSurfaceError(error: unknown): error is MarrowSurfaceError {
  return error instanceof MarrowSurfaceError;
}

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

function invertMembers(table: Record<string, string>): Record<string, string> {
  const inverted: Record<string, string> = {};
  for (const [catalogId, label] of Object.entries(table)) {
    inverted[label] = catalogId;
  }
  return inverted;
}

function surfaceUrl(baseUrl: string | undefined, path: string): string {
  if (!baseUrl) {
    return path;
  }
  return `${baseUrl.replace(/\/$/, "")}${path}`;
}

function surfaceErrorFromBody(body: unknown): MarrowSurfaceError {
  const fault = (body ?? {}) as SurfaceErrorBodyJson;
  const code = typeof fault.code === "string" ? (fault.code as SurfaceErrorCode) : "surface.request";
  const message = typeof fault.message === "string" ? fault.message : "surface request failed";
  return new MarrowSurfaceError(code, message, body);
}

function expectEnvelope(
  value: unknown,
  operationTag: string,
  expectedResultKind: string,
): SurfaceOperationResponseJson {
  if (value === null || typeof value !== "object") {
    throw new Error("Marrow surface response must be an object");
  }
  const envelope = value as SurfaceOperationResponseJson;
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

type SurfaceTransport = {
  invoke(
    operationTag: string,
    resultKind: string,
    request: unknown,
  ): Promise<SurfaceOperationResponseJson>;
  invokeRaw(
    operationTag: string,
    requestKind: string,
    request: unknown,
  ): Promise<SurfaceOperationResponseJson>;
};

function makeTransport(options: MarrowSurfaceClientOptions): SurfaceTransport {
  const post = async (operationTag: string, request: unknown): Promise<unknown> => {
    const fetcher = options.fetch ?? (globalThis as unknown as { fetch?: SurfaceFetch }).fetch;
    if (!fetcher) {
      throw new Error("Marrow surface client requires a fetch implementation");
    }
    const response = await fetcher(surfaceUrl(options.baseUrl, operationPath(operationTag)), {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(options.headers ?? {}) },
      body: JSON.stringify(request),
    });
    const body = await response.json();
    if (!response.ok) {
      throw surfaceErrorFromBody(body);
    }
    return body;
  };
  return {
    async invoke(operationTag, resultKind, request) {
      const requestKind = REQUEST_KIND_BY_TAG[operationTag];
      const body = await post(operationTag, operationRequest(operationTag, requestKind, request));
      return expectEnvelope(body, operationTag, resultKind);
    },
    async invokeRaw(operationTag, requestKind, request) {
      const body = await post(operationTag, operationRequest(operationTag, requestKind, request));
      return body as SurfaceOperationResponseJson;
    },
  };
}

/// The raw escape hatch: serialize and POST one operation, returning the undecoded
/// `surface.operation.v1` envelope for callers who need the wire shape.
export function invokeRaw(
  options: MarrowSurfaceClientOptions,
  operationTag: string,
  request: unknown,
): Promise<SurfaceOperationResponseJson> {
  const requestKind = REQUEST_KIND_BY_TAG[operationTag];
  return makeTransport(options).invokeRaw(operationTag, requestKind, request);
}

function operationPath(operationTag: string): string {
  return `${ROUTE_PREFIX_BY_TAG[operationTag]}${operationTag}`;
}

function operationRequest(operationTag: string, requestKind: string, request: unknown): unknown {
  const body: { kind: string; request?: unknown } = { kind: requestKind };
  if (request !== undefined) {
    body.request = request;
  }
  return {
    profile_version: SURFACE_OPERATION_PROFILE_VERSION,
    operation_tag: operationTag,
    request: body,
  };
}

function identityFromBrand(brand: {
  __store: string;
  keys: SurfaceKeyJson[];
}): SurfaceIdentityWireJson {
  return { store_catalog_id: brand.__store, keys: brand.keys };
}

function fieldsByCatalogId(
  fields: ReadonlyArray<{ catalog_id?: string; member_catalog_id?: string; value: SurfaceWireValueJson | null }>,
): Map<string, SurfaceWireValueJson | null> {
  const byId = new Map<string, SurfaceWireValueJson | null>();
  for (const field of fields) {
    const id = field.catalog_id ?? field.member_catalog_id;
    if (typeof id === "string") {
      byId.set(id, field.value);
    }
  }
  return byId;
}

function presentField(
  fields: Map<string, SurfaceWireValueJson | null>,
  catalogId: string,
): { found: boolean; value: SurfaceWireValueJson | null } {
  if (!fields.has(catalogId)) {
    return { found: false, value: null };
  }
  return { found: true, value: fields.get(catalogId) ?? null };
}

function requiredValue(field: {
  found: boolean;
  value: SurfaceWireValueJson | null;
}): SurfaceWireValueJson {
  if (!field.found || field.value === null) {
    throw new Error("Marrow surface record is missing a required field value");
  }
  return field.value;
}

function optionalValue<T>(
  field: { found: boolean; value: SurfaceWireValueJson | null },
  decode: (raw: SurfaceWireValueJson) => T,
): T | null {
  if (!field.found) {
    throw new Error("Marrow surface record is missing a projected field");
  }
  if (field.value === null) {
    return null;
  }
  return decode(field.value);
}

function decodeIntValue(value: SurfaceWireValueJson): bigint {
  if (value.kind !== "int" || typeof value.value !== "string") {
    throw new Error("Marrow surface int value is malformed");
  }
  return BigInt(value.value);
}

function decodeBoolValue(value: SurfaceWireValueJson): boolean {
  if (value.kind !== "bool" || typeof value.value !== "boolean") {
    throw new Error("Marrow surface bool value is malformed");
  }
  return value.value;
}

function decodeStringValue(value: SurfaceWireValueJson): string {
  if (value.kind !== "string" || typeof value.value !== "string") {
    throw new Error("Marrow surface string value is malformed");
  }
  return value.value;
}

function decodeWireScalar(value: SurfaceWireValueJson, kind: string): string {
  if (value.kind !== kind) {
    throw new Error(`Marrow surface ${kind} value is malformed`);
  }
  const field = kind === "bytes" ? value.value_b64 : value.value;
  if (typeof field !== "string") {
    throw new Error(`Marrow surface ${kind} value is malformed`);
  }
  return field;
}

function decodeEnumValue<Member extends string>(
  value: SurfaceWireValueJson,
  byLabel: Record<string, string>,
): Member {
  if (value.kind !== "enum" || typeof value.member_catalog_id !== "string") {
    throw new Error("Marrow surface enum value is malformed");
  }
  for (const [label, catalogId] of Object.entries(byLabel)) {
    if (catalogId === value.member_catalog_id) {
      return label as Member;
    }
  }
  throw new Error("Marrow surface enum member is not in the generated catalog");
}

function decodeIdentityValue<Brand>(
  value: SurfaceWireValueJson,
  brand: (keys: SurfaceKeyJson[]) => Brand,
): Brand {
  if (value.kind !== "identity" || !Array.isArray(value.keys)) {
    throw new Error("Marrow surface identity value is malformed");
  }
  return brand(value.keys as SurfaceKeyJson[]);
}

function decodeSequence<T>(
  value: SurfaceWireValueJson,
  decode: (item: SurfaceWireValueJson) => T,
): T[] {
  if (value.kind !== "sequence" || !Array.isArray(value.values)) {
    throw new Error("Marrow surface sequence value is malformed");
  }
  return (value.values as SurfaceWireValueJson[]).map(decode);
}

function resourceValueOf(value: SurfaceWireValueJson): SurfaceResourceWireJson {
  if (value.kind !== "resource" || !Array.isArray(value.fields)) {
    throw new Error("Marrow surface resource value is malformed");
  }
  return { fields: value.fields as SurfaceResourceFieldWireJson[] };
}

function recordOf(envelope: SurfaceOperationResponseJson): SurfaceRecordWireJson {
  const record = (envelope.result as { record?: unknown }).record;
  if (record === null || typeof record !== "object") {
    throw new Error("Marrow surface record result is malformed");
  }
  return record as SurfaceRecordWireJson;
}

function optionalRecordOf<T>(
  envelope: SurfaceOperationResponseJson,
  decode: (record: SurfaceRecordWireJson) => T,
): T | null {
  const record = (envelope.result as { record?: unknown }).record;
  if (record === null || record === undefined) {
    return null;
  }
  if (typeof record !== "object") {
    throw new Error("Marrow surface optional record result is malformed");
  }
  return decode(record as SurfaceRecordWireJson);
}

function pageOf<Row, Cursor>(
  envelope: SurfaceOperationResponseJson,
  decode: (record: SurfaceRecordWireJson) => Row,
): Page<Row, Cursor> {
  const page = (envelope.result as { page?: unknown }).page;
  if (page === null || typeof page !== "object") {
    throw new Error("Marrow surface page result is malformed");
  }
  const rowsValue = (page as { rows?: unknown }).rows;
  if (!Array.isArray(rowsValue)) {
    throw new Error("Marrow surface page rows are malformed");
  }
  const next = (page as { next?: unknown }).next;
  return {
    rows: (rowsValue as SurfaceRecordWireJson[]).map(decode),
    next: (next ?? null) as Cursor | null,
  };
}

function actionOutput(envelope: SurfaceOperationResponseJson): { output: string; value: unknown } {
  const result = (envelope.result as { result?: unknown }).result as
    | { output?: unknown; value?: unknown }
    | undefined;
  if (!result || typeof result.output !== "string") {
    throw new Error("Marrow surface action result is malformed");
  }
  return { output: result.output, value: result.value };
}

/// An action that returns no domain value: its `value` is always null, `output` carries any printed
/// text. Decoding a present value here would be a wire contract violation, so reject it.
function actionResultVoid(envelope: SurfaceOperationResponseJson): { value: null; output: string } {
  const { output, value } = actionOutput(envelope);
  if (value !== null && value !== undefined) {
    throw new Error("Marrow surface action result carried an unexpected value");
  }
  return { value: null, output };
}

/// An action that returns a domain value: the value is required and decodes through `decode`.
function actionResultValue<T>(
  envelope: SurfaceOperationResponseJson,
  decode: (value: SurfaceWireValueJson) => T,
): { value: T; output: string } {
  const { output, value } = actionOutput(envelope);
  if (value === null || value === undefined) {
    throw new Error("Marrow surface action result is missing its value");
  }
  return { value: decode(value as SurfaceWireValueJson), output };
}

function computedReadResult(envelope: SurfaceOperationResponseJson): unknown {
  const result = (envelope.result as { result?: unknown }).result as
    | { value?: unknown }
    | undefined;
  if (!result) {
    throw new Error("Marrow surface computed read result is malformed");
  }
  return result.value;
}

/// A computed read that yields a domain value: required and decoded to the typed value (D6 drops the
/// always-empty `output`). A computed read forbids host effects, so the value is the whole result.
function computedReadValue<T>(
  envelope: SurfaceOperationResponseJson,
  decode: (value: SurfaceWireValueJson) => T,
): T {
  const value = computedReadResult(envelope);
  if (value === null || value === undefined) {
    throw new Error("Marrow surface computed read is missing its value");
  }
  return decode(value as SurfaceWireValueJson);
}

/// A computed read declared to yield no value: its result is always null.
function computedReadVoid(envelope: SurfaceOperationResponseJson): null {
  const value = computedReadResult(envelope);
  if (value !== null && value !== undefined) {
    throw new Error("Marrow surface computed read carried an unexpected value");
  }
  return null;
}

function encodeEnumWrite(member: string, byLabel: Record<string, string>): SurfaceWireValueJson {
  return { kind: "enum", member_catalog_id: requireMemberId(member, byLabel) } as never;
}

function encodeEnumMember(member: string, byLabel: Record<string, string>): SurfaceWireValueJson {
  return { kind: "enum_member", member_catalog_id: requireMemberId(member, byLabel) } as never;
}

function requireMemberId(member: string, byLabel: Record<string, string>): string {
  const catalogId = byLabel[member];
  if (typeof catalogId !== "string") {
    throw new Error("Marrow surface enum member is not in the generated catalog");
  }
  return catalogId;
}

function encodeIdentityWrite(brand: {
  __store: string;
  keys: SurfaceKeyJson[];
}): SurfaceWireValueJson {
  return identityFromBrand(brand) as never;
}

function encodeIdentityArgument(brand: {
  __store: string;
  keys: SurfaceKeyJson[];
}): SurfaceWireValueJson {
  return { kind: "identity", ...identityFromBrand(brand) } as never;
}

function encodeWriteValue(value: unknown): SurfaceWireValueJson {
  return value as SurfaceWireValueJson;
}

