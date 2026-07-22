// The Club Locker renderer. Plain DOM over the `window.club` bridge the preload
// exposes; no framework, no direct Node or Electron reach. Every mutation goes
// through the trusted main, which runs it against the local store; this file only
// collects domain arguments, renders replies, and logs activity.

import type { ClubApi, CallResult } from "../app/preload.mts";

declare global {
  interface Window {
    club: ClubApi;
  }
}

const club = window.club;

// --- small DOM helpers ------------------------------------------------------

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (node === null) throw new Error(`missing #${id}`);
  return node as T;
}

function field(form: HTMLFormElement, name: string): string {
  const data = new FormData(form);
  return String(data.get(name) ?? "").trim();
}

function bigintField(form: HTMLFormElement, name: string): bigint | null {
  const raw = field(form, name);
  if (!/^\d+$/.test(raw)) return null;
  return BigInt(raw);
}

const logList = el<HTMLOListElement>("log");

function log(kind: "ok" | "err", message: string, detail?: string): void {
  const item = document.createElement("li");
  item.className = kind;
  item.textContent = message;
  if (detail !== undefined && detail.length > 0) {
    const span = document.createElement("span");
    span.className = "detail";
    span.textContent = ` — ${detail}`;
    item.append(span);
  }
  logList.prepend(item);
}

/** The sentinel a bridged call returns when the transport itself failed or the main
 * refused it, distinct from a legitimate `null` domain value (e.g. an absent name). */
const FAILED = Symbol("failed");

/** Unwrap a bridged reply, logging on a transport/refusal error and returning the
 * `FAILED` sentinel so callers never confuse it with a real `null` value. */
function unwrap<T>(result: CallResult<T>, context: string): T | typeof FAILED {
  if (result.ok) return result.value;
  log("err", context, result.error);
  return FAILED;
}

/** Whether a Result-sum reply is the `ok` variant, logging the domain error text.
 * Returns `FAILED` on a transport error or an `err` variant. */
type Sum<Ok, Err> = { member: "ok"; payload: [Ok] } | { member: "err"; payload: [Err] };
function okOf<Ok>(sum: Sum<Ok, string> | typeof FAILED, context: string): Ok | typeof FAILED {
  if (sum === FAILED) return FAILED;
  if (sum.member === "ok") return sum.payload[0];
  log("err", context, sum.payload[0]);
  return FAILED;
}

// --- members ----------------------------------------------------------------

el<HTMLFormElement>("register-member").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget as HTMLFormElement;
  const name = field(form, "name");
  const joined = field(form, "joined");
  const id = unwrap(await club.registerMember(name, joined), "register member");
  if (id !== FAILED) {
    log("ok", `Registered ${name}`, `member #${id}`);
    form.reset();
    await openMember(id);
  }
});

el<HTMLFormElement>("lookup-member").addEventListener("submit", async (event) => {
  event.preventDefault();
  const memberId = bigintField(event.currentTarget as HTMLFormElement, "memberId");
  if (memberId === null) {
    log("err", "Open member", "enter a numeric member #");
    return;
  }
  await openMember(memberId);
});

async function openMember(memberId: bigint): Promise<void> {
  const name = unwrap(await club.memberName(memberId), "load member");
  if (name === FAILED) return;
  if (name === null) {
    log("err", "Open member", `no member #${memberId}`);
    return;
  }
  const active = unwrap(await club.memberIsActive(memberId), "load standing");
  const email = unwrap(await club.memberEmail(memberId), "load email");
  const history = unwrap(await club.memberHistory(memberId), "load history");

  el("member-card-title").textContent = `#${memberId} · ${name}`;
  el("member-standing").textContent =
    active === FAILED || active === null ? "—" : active ? "active" : "suspended";
  el("member-email").textContent = email === FAILED ? "—" : (email ?? "—");
  el("member-history").textContent = history === FAILED ? "—" : formatHistory(history);
  const card = el<HTMLDivElement>("member-card");
  card.hidden = false;
  card.dataset.memberId = String(memberId);
}

function formatHistory(history: string | null): string {
  if (history === null || history.length === 0) return "no events yet";
  return history
    .split(";")
    .filter((part) => part.length > 0)
    .join(", ")
    .replace("+more", "…");
}

el<HTMLFormElement>("member-email-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget as HTMLFormElement;
  const card = el<HTMLDivElement>("member-card");
  const memberId = card.dataset.memberId;
  if (memberId === undefined) return;
  const email = field(form, "email");
  const done = unwrap(await club.setEmail(BigInt(memberId), email), "set email");
  if (done !== FAILED) {
    log("ok", `Updated email for member #${memberId}`);
    form.reset();
    await openMember(BigInt(memberId));
  }
});

// --- assets -----------------------------------------------------------------

el<HTMLFormElement>("register-asset").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget as HTMLFormElement;
  const tag = field(form, "tag");
  const id = okOf(unwrap(await club.registerAsset(tag, field(form, "category"), field(form, "name")), "register asset"), "register asset");
  if (id !== FAILED) {
    log("ok", `Registered asset ${tag}`, `asset #${id}`);
    form.reset();
  }
});

el<HTMLFormElement>("lookup-category").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget as HTMLFormElement;
  const category = field(form, "category");
  const tags = unwrap(await club.assetsByCategory(category), "list category");
  if (tags === FAILED) return;
  const box = el<HTMLDivElement>("category-list");
  const shown = tags.split(";").filter((t) => t.length > 0);
  box.textContent = shown.length === 0 ? `No assets in ${category}.` : `${category}: ${shown.join(", ").replace("+more", "…")}`;
  box.hidden = false;
});

// --- loan desk --------------------------------------------------------------

el<HTMLFormElement>("checkout").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget as HTMLFormElement;
  const memberId = bigintField(form, "memberId");
  const assetId = bigintField(form, "assetId");
  const onDay = field(form, "onDay");
  if (memberId === null || assetId === null) {
    log("err", "Check out", "member # and asset # must be numeric");
    return;
  }
  const loanNo = okOf(unwrap(await club.checkout(memberId, assetId, onDay), "check out"), "check out");
  if (loanNo !== FAILED) {
    log("ok", `Checked out asset #${assetId} to member #${memberId}`, `loan #${loanNo}`);
    form.reset();
  }
});

el<HTMLFormElement>("return").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget as HTMLFormElement;
  const memberId = bigintField(form, "memberId");
  const assetId = bigintField(form, "assetId");
  const onDay = field(form, "onDay");
  if (memberId === null || assetId === null) {
    log("err", "Return", "member # and asset # must be numeric");
    return;
  }
  const loanNo = okOf(unwrap(await club.returnAsset(memberId, assetId, onDay), "return"), "return");
  if (loanNo !== FAILED) {
    log("ok", `Returned asset #${assetId} from member #${memberId}`, `closed loan #${loanNo}`);
    form.reset();
  }
});
