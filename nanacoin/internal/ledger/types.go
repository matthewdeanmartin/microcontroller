// Package ledger holds NanaCoin's money model: an append-only sequence of
// transactions, each a set of postings that sum to zero.
//
// There is no balance field anywhere in this package's persistent model. A
// balance is a fold over postings, and the cached copies in Book exist purely
// so that reads are cheap; they are rebuilt from the transaction list on boot
// and are never the authority for anything.
package ledger

// Amount is a whole NanaCoin. Integers only - see spec 2.3. Nothing in this
// codebase may represent money as a float, including in JSON, where an int64
// larger than 2^53 would lose precision. At household scale that ceiling is
// unreachable, but the type stays int64 so the ledger can never silently
// round.
type Amount = int64

type (
	AccountID     string
	UserID        string
	TransactionID string
	ListingID     string
)

// SystemIssuance is the account new coin is created from and retired into. It
// is the one account permitted to go arbitrarily negative: its balance is the
// negation of all NanaCoin in circulation, which gives the invariant that
// every posting in the book sums to zero across all accounts.
//
// It is not a user account and must never be exposed as a transfer target.
const SystemIssuance AccountID = "account:system-issuance"

type TransactionKind string

const (
	KindIssue    TransactionKind = "ISSUE"
	KindRetire   TransactionKind = "RETIRE"
	KindTransfer TransactionKind = "TRANSFER"
	KindPurchase TransactionKind = "PURCHASE"
	KindReversal TransactionKind = "REVERSAL"
)

// Posting is one side of a transaction: a signed change to one account.
type Posting struct {
	Account AccountID `json:"account"`
	Amount  Amount    `json:"amount"`
}

// Transaction is immutable once appended. Corrections are made by appending a
// reversal, never by editing or deleting - see spec 11.
type Transaction struct {
	ID          TransactionID   `json:"id"`
	Kind        TransactionKind `json:"kind"`
	CreatedAt   int64           `json:"created_at"` // unix seconds
	Actor       UserID          `json:"actor"`
	Description string          `json:"description"`

	// Reference points at whatever non-ledger object caused this
	// transaction - a listing ID for a purchase, say. Free-form so that
	// future features need no ledger schema change.
	Reference string `json:"reference,omitempty"`

	// Reverses is set on a KindReversal transaction and names the
	// transaction being undone. The reverse direction (original -> its
	// reversal) is an in-memory index, not a stored field, because stored
	// records are never rewritten.
	Reverses TransactionID `json:"reverses,omitempty"`

	Postings []Posting `json:"postings"`
}

// Sum returns the total of all postings. For every transaction kind this must
// be zero; issuance balances against SystemIssuance rather than being exempt.
func (t *Transaction) Sum() Amount {
	var total Amount
	for _, p := range t.Postings {
		total += p.Amount
	}
	return total
}

// Affects reports whether the transaction touches an account, so history
// queries do not need to care which side of it a user was on.
func (t *Transaction) Affects(acct AccountID) bool {
	for _, p := range t.Postings {
		if p.Account == acct {
			return true
		}
	}
	return false
}
