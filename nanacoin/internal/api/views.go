package api

import (
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/core"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/ledger"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/marketplace"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/users"
)

// The view types below are what the API actually serialises. They exist so
// that adding a field to a domain struct - a password verifier, say - cannot
// leak it through an endpoint by accident. Nothing in a handler serialises a
// domain object directly.

type userView struct {
	ID          ledger.UserID    `json:"id"`
	Username    string           `json:"username"`
	DisplayName string           `json:"display_name"`
	Role        users.Role       `json:"role"`
	Status      users.Status     `json:"status"`
	Account     ledger.AccountID `json:"account"`
	CreatedAt   int64            `json:"created_at"`

	// Balance is filled only where the caller is entitled to see it.
	Balance *ledger.Amount `json:"balance,omitempty"`
}

func viewUser(u *users.User, balance *ledger.Amount) userView {
	return userView{
		ID: u.ID, Username: u.Username, DisplayName: u.DisplayName,
		Role: u.Role, Status: u.Status, Account: u.Account,
		CreatedAt: u.CreatedAt, Balance: balance,
	}
}

type accountView struct {
	ID      ledger.AccountID `json:"id"`
	UserID  ledger.UserID    `json:"user_id"`
	Name    string           `json:"name"`
	Status  users.Status     `json:"status"`
	Balance ledger.Amount    `json:"balance"`
}

type postingView struct {
	Account ledger.AccountID `json:"account"`
	Name    string           `json:"name"`
	Amount  ledger.Amount    `json:"amount"`
}

type transactionView struct {
	ID          ledger.TransactionID   `json:"id"`
	Kind        ledger.TransactionKind `json:"kind"`
	CreatedAt   int64                  `json:"created_at"`
	Actor       ledger.UserID          `json:"actor"`
	Description string                 `json:"description"`
	Reference   string                 `json:"reference,omitempty"`
	Reverses    ledger.TransactionID   `json:"reverses,omitempty"`
	ReversedBy  ledger.TransactionID   `json:"reversed_by,omitempty"`
	Postings    []postingView          `json:"postings"`
}

type listingView struct {
	ID          ledger.ListingID     `json:"id"`
	Seller      ledger.AccountID     `json:"seller"`
	SellerName  string               `json:"seller_name"`
	Title       string               `json:"title"`
	Description string               `json:"description"`
	Price       ledger.Amount        `json:"price"`
	Status      marketplace.Status   `json:"status"`
	CreatedAt   int64                `json:"created_at"`
	UpdatedAt   int64                `json:"updated_at"`
	Buyer       ledger.AccountID     `json:"buyer,omitempty"`
	BuyerName   string               `json:"buyer_name,omitempty"`
	SoldTx      ledger.TransactionID `json:"sold_tx,omitempty"`
	Kind        string               `json:"kind,omitempty"`
	Currency    string               `json:"currency,omitempty"`
	MinorUnits  int64                `json:"minor_units,omitempty"`
}

// namer resolves an account to a display name for the views. Transactions are
// stored with account IDs only; putting names in the ledger would mean a
// rename rewrote history.
type namer struct{ svc *core.Service }

func (n namer) name(id ledger.AccountID) string {
	if id == ledger.SystemIssuance {
		return "Issuance"
	}
	return n.svc.AccountName(id)
}

func (n namer) transaction(t *ledger.Transaction) transactionView {
	postings := make([]postingView, len(t.Postings))
	for i, p := range t.Postings {
		postings[i] = postingView{Account: p.Account, Name: n.name(p.Account), Amount: p.Amount}
	}
	v := transactionView{
		ID: t.ID, Kind: t.Kind, CreatedAt: t.CreatedAt, Actor: t.Actor,
		Description: t.Description, Reference: t.Reference,
		Reverses: t.Reverses, Postings: postings,
	}
	if rev, ok := n.svc.ReversalOf(t.ID); ok {
		v.ReversedBy = rev
	}
	return v
}

func (n namer) transactions(ts []*ledger.Transaction) []transactionView {
	out := make([]transactionView, len(ts))
	for i, t := range ts {
		out[i] = n.transaction(t)
	}
	return out
}

func (n namer) listing(l *marketplace.Listing) listingView {
	v := listingView{
		ID: l.ID, Seller: l.Seller, SellerName: n.name(l.Seller),
		Title: l.Title, Description: l.Description, Price: l.Price,
		Status: l.Status, CreatedAt: l.CreatedAt, UpdatedAt: l.UpdatedAt,
		Buyer: l.Buyer, SoldTx: l.SoldTx,
		Kind: l.Kind, Currency: l.Currency, MinorUnits: l.MinorUnits,
	}
	if l.Buyer != "" {
		v.BuyerName = n.name(l.Buyer)
	}
	return v
}

func (n namer) listings(ls []*marketplace.Listing) []listingView {
	out := make([]listingView, len(ls))
	for i, l := range ls {
		out[i] = n.listing(l)
	}
	return out
}

// lockedNamer is namer for use inside an Each* callback.
//
// namer calls svc.Account and svc.ReversalOf, each of which takes the service
// lock. Inside a walk the lock is already held, so using namer there would
// deadlock. This resolves through the *Locked accessors instead.
//
// The two exist separately rather than one growing a flag because the
// distinction is not a preference - getting it wrong is a hang on the board,
// which is indistinguishable from the network failures this project has
// already spent a long time chasing.
type lockedNamer struct{ svc *core.Service }

func (n lockedNamer) name(id ledger.AccountID) string {
	return n.svc.NameLocked(id)
}

func (n lockedNamer) transaction(t *ledger.Transaction) transactionView {
	// Postings are built per transaction and handed straight to the encoder,
	// which encodes and discards before the next record is unpacked. So this
	// allocation is live for one element, not for the page.
	postings := make([]postingView, len(t.Postings))
	for i, p := range t.Postings {
		postings[i] = postingView{Account: p.Account, Name: n.name(p.Account), Amount: p.Amount}
	}
	v := transactionView{
		ID: t.ID, Kind: t.Kind, CreatedAt: t.CreatedAt, Actor: t.Actor,
		Description: t.Description, Reference: t.Reference,
		Reverses: t.Reverses, Postings: postings,
	}
	if rev, ok := n.svc.ReversalOfLocked(t.ID); ok {
		v.ReversedBy = rev
	}
	return v
}

func (n lockedNamer) listing(l *marketplace.Listing) listingView {
	v := listingView{
		ID: l.ID, Seller: l.Seller, SellerName: n.name(l.Seller),
		Title: l.Title, Description: l.Description, Price: l.Price,
		Status: l.Status, CreatedAt: l.CreatedAt, UpdatedAt: l.UpdatedAt,
		Buyer: l.Buyer, SoldTx: l.SoldTx,
		Kind: l.Kind, Currency: l.Currency, MinorUnits: l.MinorUnits,
	}
	if l.Buyer != "" {
		v.BuyerName = n.name(l.Buyer)
	}
	return v
}
