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

export type MarrowRangeBound<T> = { value: T; inclusive: boolean };
export type MarrowRange<T> = { lower?: MarrowRangeBound<T>; upper?: MarrowRangeBound<T> };

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

/// Encode a decimal argument or write value into the one canonical spelling the server accepts: no
/// leading integer zeros, no trailing fraction zeros, and no signed zero. A decimal is carried as a
/// string because no JS number holds it faithfully, and the store keeps a single canonical form per
/// value, so `10.50` and `10.5` denote the same decimal and both serialize to `10.5`. A string that
/// is not a well-formed decimal throws rather than reaching the server as an opaque request fault.
function decimalValue(value: string): SurfaceWireValueJson {
  const match = /^(-?)([0-9]+)(?:\.([0-9]+))?$/.exec(value);
  if (!match) {
    throw new Error(`Marrow decimal must be a decimal number string, got ${JSON.stringify(value)}`);
  }
  const integer = match[2].replace(/^0+(?=[0-9])/, "");
  const fraction = (match[3] ?? "").replace(/0+$/, "");
  const magnitude = fraction === "" ? integer : `${integer}.${fraction}`;
  const canonical = integer === "0" && fraction === "" ? "0" : `${match[1]}${magnitude}`;
  return { kind: "decimal", value: canonical };
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
  const body: { kind: string; request: unknown } = {
    kind: requestKind,
    request: request === undefined ? {} : request,
  };
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

/// A surface `date` value carries its day count under `days_since_epoch`. The count is an i32, so a
/// JS number holds it exactly; richer date domain types are deferred, so the faithful value is the
/// raw day count.
function decodeDateValue(value: SurfaceWireValueJson): number {
  if (value.kind !== "date" || typeof value.days_since_epoch !== "number") {
    throw new Error("Marrow surface date value is malformed");
  }
  return value.days_since_epoch;
}

/// An `instant` or `duration` value carries its nanosecond count as a decimal string because the
/// count can exceed 2^53; decoding it as a JS number would silently truncate. The faithful value is
/// a bigint over that exact count.
function decodeNanosValue(value: SurfaceWireValueJson, kind: "instant" | "duration"): bigint {
  if (value.kind !== kind) {
    throw new Error(`Marrow surface ${kind} value is malformed`);
  }
  const nanos = kind === "instant" ? value.nanos_since_epoch : value.nanos;
  if (typeof nanos !== "string") {
    throw new Error(`Marrow surface ${kind} value is malformed`);
  }
  return BigInt(nanos);
}

/// A `decimal` value carries its canonical text under `value`; `bytes` carries base64 under
/// `value_b64`. Both decode to their faithful string form without loss.
function decodeWireScalar(value: SurfaceWireValueJson, kind: "decimal" | "bytes"): string {
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

/// An action whose returned value is optional (`T?`): an absent value decodes to null rather than an
/// error, and a present value decodes through `decode`. Absence rides the wire as a null result value.
function actionResultOptionalValue<T>(
  envelope: SurfaceOperationResponseJson,
  decode: (value: SurfaceWireValueJson) => T,
): { value: T | null; output: string } {
  const { output, value } = actionOutput(envelope);
  if (value === null || value === undefined) {
    return { value: null, output };
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

/// A computed read that yields a domain value: required and decoded to the typed value, dropping the
/// always-empty `output`. A computed read forbids host effects, so the value is the whole result.
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

/// A computed read whose result is optional (`T?`): an absent result decodes to null rather than an
/// error, and a present value decodes through `decode`. Absence rides the wire as a null result value.
function computedReadOptionalValue<T>(
  envelope: SurfaceOperationResponseJson,
  decode: (value: SurfaceWireValueJson) => T,
): T | null {
  const value = computedReadResult(envelope);
  if (value === null || value === undefined) {
    return null;
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

/// Encode an enum member into the request enum shape the server validates for write fields, index
/// keys, and unique-lookup keys: the enum catalog id alongside the member id.
function encodeEnum(
  member: string,
  enumCatalogId: string,
  byLabel: Record<string, string>,
): SurfaceWireValueJson {
  return {
    kind: "enum",
    enum_catalog_id: enumCatalogId,
    member_catalog_id: requireMemberId(member, byLabel),
  };
}

/// Encode an enum member into the entry argument shape an action or computed read decodes, which
/// tags the bare member id under `enum_member`.
function encodeEnumMember(member: string, byLabel: Record<string, string>): SurfaceWireValueJson {
  return { kind: "enum_member", member_catalog_id: requireMemberId(member, byLabel) };
}

function requireMemberId(member: string, byLabel: Record<string, string>): string {
  const catalogId = byLabel[member];
  if (typeof catalogId !== "string") {
    throw new Error("Marrow surface enum member is not in the generated catalog");
  }
  return catalogId;
}

/// Encode a branded identity into its wire value for a write field, index exact-key, or unique
/// lookup. These contexts decode each key as `SurfaceKeyJson`, the brand's stored key shape, so the
/// keys pass through unchanged.
function encodeIdentity(brand: {
  __store: string;
  keys: SurfaceKeyJson[];
}): SurfaceWireValueJson {
  const identity = identityFromBrand(brand);
  return { kind: "identity", store_catalog_id: identity.store_catalog_id, keys: identity.keys };
}

/// Encode a branded identity into the entry argument shape an action or computed read decodes. The
/// entry decoder reads each key as a uniform `{ kind, value }` scalar with the value's canonical
/// datum, which differs from the brand's `SurfaceKeyJson` form for `date` (day count vs canonical
/// text) and `bytes` (base64 vs hex); the remaining kinds already carry their datum under `value`.
function encodeIdentityArgument(brand: {
  __store: string;
  keys: SurfaceKeyJson[];
}): SurfaceWireValueJson {
  const identity = identityFromBrand(brand);
  return {
    kind: "identity",
    store_catalog_id: identity.store_catalog_id,
    keys: identity.keys.map(keyToEntryScalar),
  };
}

function keyToEntryScalar(key: SurfaceKeyJson): SurfaceWireValueJson {
  switch (key.kind) {
    case "int":
    case "string":
      return { kind: key.kind, value: key.value };
    case "bool":
      return { kind: "bool", value: key.value };
    case "duration":
      return { kind: "duration", value: key.nanos };
    case "instant":
      return { kind: "instant", value: key.nanos_since_epoch };
    case "date":
      return { kind: "date", value: dateText(key.days_since_epoch) };
    case "bytes":
      return { kind: "bytes", value: base64ToHex(key.value_b64) };
  }
}

/// Build the canonical `SurfaceKeyJson` for one key of a branded identity from the typed constructor
/// input. The brand stores this wire form so a decoded identity (which only ever has `SurfaceKeyJson`)
/// and a freshly constructed one are indistinguishable. Temporal and bytes keys take their faithful
/// wire datum — a day count, a nanosecond count, base64 — because the client cannot derive it from a
/// lossy display form without unsound conversion.
function intKey(value: MarrowIntInput): SurfaceKeyJson {
  return { kind: "int", value: encodeMarrowInt(value) };
}

function boolKey(value: boolean): SurfaceKeyJson {
  return { kind: "bool", value };
}

function stringKey(value: string): SurfaceKeyJson {
  return { kind: "string", value };
}

function dateKey(daysSinceEpoch: MarrowIntInput): SurfaceKeyJson {
  return { kind: "date", days_since_epoch: Number(encodeMarrowInt(daysSinceEpoch)) };
}

function durationKey(nanos: MarrowIntInput): SurfaceKeyJson {
  return { kind: "duration", nanos: encodeMarrowInt(nanos) };
}

function instantKey(nanosSinceEpoch: MarrowIntInput): SurfaceKeyJson {
  return { kind: "instant", nanos_since_epoch: encodeMarrowInt(nanosSinceEpoch) };
}

function bytesKey(valueBase64: string): SurfaceKeyJson {
  return { kind: "bytes", value_b64: valueBase64 };
}

/// Render a day count as canonical `YYYY-MM-DD`, the text the entry decoder parses for a `date`
/// argument. The conversion is Howard Hinnant's proleptic-Gregorian `civil_from_days`, exact for
/// every day count the server accepts.
function dateText(days: number): string {
  if (!Number.isInteger(days)) {
    throw new Error("Marrow surface date day count must be an integer");
  }
  const z = days + 719468;
  const era = Math.floor((z >= 0 ? z : z - 146096) / 146097);
  const doe = z - era * 146097;
  const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36524) - Math.floor(doe / 146096)) / 365);
  const y = yoe + era * 400;
  const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
  const mp = Math.floor((5 * doy + 2) / 153);
  const d = doy - Math.floor((153 * mp + 2) / 5) + 1;
  const m = mp < 10 ? mp + 3 : mp - 9;
  const year = m <= 2 ? y + 1 : y;
  if (year < 1 || year > 9999) {
    throw new Error("Marrow surface date is outside the canonical four-digit year range");
  }
  return `${pad(year, 4)}-${pad(m, 2)}-${pad(d, 2)}`;
}

function pad(value: number, width: number): string {
  return value.toString().padStart(width, "0");
}

const BASE64_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decode padded base64 to its bytes, the inverse of the server's encoding. The brand stores bytes
/// keys as base64; the entry decoder reads them as hex, so a bytes identity argument routes through
/// this decode and `bytesToHex`.
function base64ToBytes(text: string): number[] {
  if (text.length % 4 !== 0) {
    throw new Error("Marrow surface bytes value must be padded base64");
  }
  const bytes: number[] = [];
  for (let index = 0; index < text.length; index += 4) {
    const chunk = text.slice(index, index + 4);
    const pad = chunk.endsWith("==") ? 2 : chunk.endsWith("=") ? 1 : 0;
    let bits = 0;
    for (let offset = 0; offset < 4; offset += 1) {
      const character = chunk[offset];
      const sextet = character === "=" ? 0 : BASE64_ALPHABET.indexOf(character);
      if (sextet < 0) {
        throw new Error("Marrow surface bytes value must be padded base64");
      }
      bits = (bits << 6) | sextet;
    }
    bytes.push((bits >> 16) & 0xff);
    if (pad < 2) bytes.push((bits >> 8) & 0xff);
    if (pad < 1) bytes.push(bits & 0xff);
  }
  return bytes;
}

function base64ToHex(text: string): string {
  return base64ToBytes(text)
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

function encodeWriteValue(value: unknown): SurfaceWireValueJson {
  return value as SurfaceWireValueJson;
}

type SurfacePageRangeWireJson = {
  lower?: SurfaceWireValueJson;
  lower_inclusive?: boolean;
  upper?: SurfaceWireValueJson;
  upper_inclusive?: boolean;
};

/// Encode a typed range over a ranged index key into the page request `range` shape. Each supplied
/// bound carries its encoded key value and inclusivity flag; the server rejects a range with neither
/// bound, so a caller must pass at least a lower or an upper bound.
function encodeRange<T>(
  range: MarrowRange<T>,
  encode: (value: T) => SurfaceWireValueJson,
): SurfacePageRangeWireJson {
  const wire: SurfacePageRangeWireJson = {};
  if (range.lower !== undefined) {
    wire.lower = encode(range.lower.value);
    wire.lower_inclusive = range.lower.inclusive;
  }
  if (range.upper !== undefined) {
    wire.upper = encode(range.upper.value);
    wire.upper_inclusive = range.upper.inclusive;
  }
  return wire;
}
