// Package core is NanaCoin's application layer: the one place where a request
// becomes a durable change.
//
// Every mutation follows the same shape:
//
//	validate against the RAM model
//	encode an event
//	append it to the journal   <- the durability point
//	apply it to the RAM model
//
// The order matters. Applying first and journalling second would let a flash
// failure leave a balance in RAM that no record supports, and the next reboot
// would quietly undo a transfer the user was told had succeeded. Journalling
// first means the worst case is a transfer that was recorded and reported as
// failed, which Nana can see in the ledger and resolve.
package core

import (
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/ledger"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/marketplace"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/storage"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/users"
)

// Event payloads. These are the durable schema: changing a field here changes
// what a ten-year-old journal means, so fields are added, never repurposed.
//
// JSON rather than a packed binary encoding. At ten events a week the size
// difference is irrelevant, and being able to read a household's ledger with
// `strings` on a dead board is worth more than the bytes.

type userCreatedEvent struct {
	User    users.User    `json:"user"`
	Account users.Account `json:"account"`
}

type userUpdatedEvent struct {
	ID          ledger.UserID `json:"id"`
	DisplayName *string       `json:"display_name,omitempty"`
	Status      *users.Status `json:"status,omitempty"`
	Role        *users.Role   `json:"role,omitempty"`
	Verifier    *string       `json:"verifier,omitempty"`
}

type transactionEvent struct {
	Txn ledger.Transaction `json:"txn"`
}

type listingCreatedEvent struct {
	Listing marketplace.Listing `json:"listing"`
}

type listingUpdatedEvent struct {
	ID          ledger.ListingID    `json:"id"`
	Title       *string             `json:"title,omitempty"`
	Description *string             `json:"description,omitempty"`
	Price       *ledger.Amount      `json:"price,omitempty"`
	Status      *marketplace.Status `json:"status,omitempty"`
	UpdatedAt   int64               `json:"updated_at"`
}

// listingPurchasedEvent carries both halves of a purchase - the money and the
// listing state change - in one record, because a purchase must be atomic
// (spec 13). Two records could half-land; one cannot.
type listingPurchasedEvent struct {
	ListingID ledger.ListingID   `json:"listing_id"`
	Buyer     ledger.AccountID   `json:"buyer"`
	Txn       ledger.Transaction `json:"txn"`
	UpdatedAt int64              `json:"updated_at"`
}

type configUpdatedEvent struct {
	Config Config `json:"config"`
}

// idempotencyEvent records that a key was used and what it produced. It is
// journalled so that a retry after reboot still returns the original result
// rather than moving money twice (spec 20).
type idempotencyEvent struct {
	Key      string        `json:"key"`
	UserID   ledger.UserID `json:"user_id"`
	Endpoint string        `json:"endpoint"`
	Result   []byte        `json:"result"`
	At       int64         `json:"at"`
}

// Config is household policy. It is journalled like everything else so that
// changing the initial grant is an auditable act, not an invisible one.
type Config struct {
	HouseholdName string        `json:"household_name"`
	InitialGrant  ledger.Amount `json:"initial_grant"`
	Currency      string        `json:"currency"` // display name, e.g. "NanaCoin"
}

func DefaultConfig() Config {
	return Config{HouseholdName: "Household", InitialGrant: 100, Currency: "NanaCoin"}
}

var _ = storage.TypeUserCreated // keep the storage import meaningful to readers
