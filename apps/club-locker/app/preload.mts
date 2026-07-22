// The preload: the only bridge between the isolated renderer and the trusted main.
//
// It runs in the isolated preload world (contextIsolation on) and exposes, through
// `contextBridge`, a fixed object of named domain methods. Each method forwards one
// typed request over the single `club:call` channel; there is no generic passthrough
// and no `ipcRenderer` handle in the page. The renderer can name only these domain
// calls, and the main re-validates the name against its own allowlist and the sender
// frame before anything runs.

import { contextBridge, ipcRenderer } from "electron";

const IPC_CHANNEL = "club:call";

/** The structured reply every bridged call resolves to. */
export type CallResult<T> = { ok: true; value: T } | { ok: false; error: string };

function call<T>(name: string, args: readonly unknown[]): Promise<CallResult<T>> {
  return ipcRenderer.invoke(IPC_CHANNEL, { name, args }) as Promise<CallResult<T>>;
}

/** A Result-sum reply, as the generated client shapes it. */
type Sum<Ok, Err> = { member: "ok"; payload: [Ok] } | { member: "err"; payload: [Err] };

const club = {
  // acts — registration and the loan lifecycle
  registerMember: (name: string, joined: string) =>
    call<bigint>("registerMember", [name, joined]),
  registerAsset: (tag: string, category: string, name: string) =>
    call<Sum<bigint, string>>("registerAsset", [tag, category, name]),
  checkout: (memberId: bigint, assetId: bigint, onDay: string) =>
    call<Sum<bigint, string>>("checkout", [memberId, assetId, onDay]),
  returnAsset: (memberId: bigint, assetId: bigint, onDay: string) =>
    call<Sum<bigint, string>>("returnAsset", [memberId, assetId, onDay]),
  setEmail: (memberId: bigint, email: string) => call<void>("setEmail", [memberId, email]),
  suspendMember: (memberId: bigint) => call<void>("suspendMember", [memberId]),
  reinstateMember: (memberId: bigint) => call<void>("reinstateMember", [memberId]),

  // reads — the member and asset views the UI renders
  memberName: (memberId: bigint) => call<string | null>("memberName", [memberId]),
  memberEmail: (memberId: bigint) => call<string | null>("memberEmail", [memberId]),
  memberExists: (memberId: bigint) => call<boolean>("memberExists", [memberId]),
  memberIsActive: (memberId: bigint) => call<boolean>("memberIsActive", [memberId]),
  memberHistory: (memberId: bigint) => call<string>("memberHistory", [memberId]),
  assetTag: (assetId: bigint) => call<string | null>("assetTag", [assetId]),
  assetExists: (assetId: bigint) => call<boolean>("assetExists", [assetId]),
  assetOnLoanTo: (assetId: bigint) => call<bigint | null>("assetOnLoanTo", [assetId]),
  loanNoFor: (memberId: bigint, assetId: bigint) =>
    call<bigint | null>("loanNoFor", [memberId, assetId]),
  tagTaken: (tag: string) => call<boolean>("tagTaken", [tag]),
  assetNameByTag: (tag: string) => call<string | null>("assetNameByTag", [tag]),
  assetsByCategory: (category: string) => call<string>("assetsByCategory", [category]),
};

export type ClubApi = typeof club;

contextBridge.exposeInMainWorld("club", club);
