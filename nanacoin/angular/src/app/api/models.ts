// Wire types for the NanaCoin API.
//
// These mirror the view types the Go server serialises in
// internal/api/views.go. They are hand-written rather than generated: the API
// is small, stable and versioned under /api/v1, and a generator would be more
// machinery than the whole client.

export type AccountId = string;
export type UserId = string;
export type TransactionId = string;
export type ListingId = string;

export type Role = 'nana' | 'user';
export type UserStatus = 'ACTIVE' | 'DISABLED';
export type ListingStatus = 'ACTIVE' | 'SOLD' | 'CANCELLED';

export type TransactionKind =
  | 'ISSUE'
  | 'RETIRE'
  | 'TRANSFER'
  | 'PURCHASE'
  | 'REVERSAL';

export interface User {
  id: UserId;
  username: string;
  display_name: string;
  role: Role;
  status: UserStatus;
  account: AccountId;
  created_at: number;
  /** Present only where the caller is entitled to see it. */
  balance?: number;
}

export interface Posting {
  account: AccountId;
  name: string;
  /** Signed. Negative is money leaving the account. */
  amount: number;
}

export interface Transaction {
  id: TransactionId;
  kind: TransactionKind;
  created_at: number;
  actor: UserId;
  description: string;
  reference?: string;
  /** Set on a REVERSAL: the transaction being undone. */
  reverses?: TransactionId;
  /** Set on a transaction that has been reversed. */
  reversed_by?: TransactionId;
  postings: Posting[];
}

export interface Listing {
  id: ListingId;
  seller: AccountId;
  seller_name: string;
  title: string;
  description: string;
  price: number;
  status: ListingStatus;
  created_at: number;
  updated_at: number;
  buyer?: AccountId;
  buyer_name?: string;
  sold_tx?: TransactionId;
  /** '' or 'item' or 'service' or 'currency'. */
  kind?: string;
  /** For a currency listing: the code, e.g. 'USD'. */
  currency?: string;
  /** For a currency listing: minor units, e.g. 500 for $5.00. */
  minor_units?: number;
}

export interface Status {
  provisioned: boolean;
  household: string;
  currency: string;
  users: number;
  transactions: number;
  active_listings: number;
  circulation: number;
  journal_used: number;
  journal_capacity: number;
  /** False means the ledger does not add up and nobody should trust it. */
  ledger_balanced: boolean;
}

export interface LogEvent {
  seq: number;
  /** Unix seconds, or 0 on a board with no clock - use seq for ordering. */
  at: number;
  level: 'info' | 'warn' | 'error';
  /** An HTTP status, or a decision name like 'cors-refuse'. */
  kind: string;
  detail: string;
}

export interface LogPage {
  events: LogEvent[];
  /** Everything ever recorded; higher than events.length once the ring wraps. */
  total: number;
  /** The host's own health - heap figures on the board, absent on a desktop. */
  health?: string;
}

export interface Config {
  household_name: string;
  initial_grant: number;
  currency: string;
}

export interface AccountHistory {
  account: AccountId;
  balance: number;
  transactions: Transaction[];
}

export interface LedgerPage {
  transactions: Transaction[];
  circulation: number;
}

export interface TokenResponse {
  access_token: string;
  token_type: string;
  expires_in: number;
  user: User;
}

export interface PurchaseResult {
  listing: Listing;
  transaction: Transaction;
}

/** The server's error shape: a stable code plus a human sentence. */
export interface ApiErrorBody {
  error: string;
  message: string;
}
