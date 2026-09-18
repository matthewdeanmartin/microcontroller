// Package marketplace holds listings: things household members offer each
// other for NanaCoin.
package marketplace

import "github.com/matthewdeanmartin/microcontroller/nanacoin/internal/ledger"

type Status string

const (
	StatusActive    Status = "ACTIVE"
	StatusSold      Status = "SOLD"
	StatusCancelled Status = "CANCELLED"
)

// Listing is an offer. Quantity is present but fixed at 1 for v1; the field
// exists so that adding multi-quantity later changes this package only and
// never the ledger.
type Listing struct {
	ID          ledger.ListingID `json:"id"`
	Seller      ledger.AccountID `json:"seller"`
	Title       string           `json:"title"`
	Description string           `json:"description"`
	Price       ledger.Amount    `json:"price"`
	Quantity    uint32           `json:"quantity"`
	Status      Status           `json:"status"`
	CreatedAt   int64            `json:"created_at"`
	UpdatedAt   int64            `json:"updated_at"`

	// Buyer and SoldTx are set when the listing sells, linking the
	// marketplace object to the ledger transaction that paid for it.
	Buyer  ledger.AccountID     `json:"buyer,omitempty"`
	SoldTx ledger.TransactionID `json:"sold_tx,omitempty"`

	// Kind and Currency describe an external-currency listing (spec 14).
	// NanaCoin records only that NanaCoin changed hands; whether the $5 was
	// actually handed over is between the household and Nana.
	Kind       string `json:"kind,omitempty"`        // "" or "currency"
	Currency   string `json:"currency,omitempty"`    // e.g. "USD"
	MinorUnits int64  `json:"minor_units,omitempty"` // e.g. 500 for $5.00
}
