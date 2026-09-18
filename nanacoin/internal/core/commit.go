package core

import (
	"fmt"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/ledger"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/storage"
)

// Receipt payload plus bounded identity fields. Allocated once, under mu.
const wireCapacity = MaxIdempotencyBytes + 256

func wireStrings(values ...string) int {
	n := 0
	for _, v := range values {
		if len(v) > 65535 {
			return wireCapacity + 1
		}
		n += 2 + len(v)
	}
	return n
}
func wireOptional(v *string) int {
	if v == nil {
		return 1
	}
	return 1 + wireStrings(*v)
}
func transactionWireSize(t *ledger.Transaction) int {
	n := 10 + wireStrings(string(t.ID), string(t.Kind), string(t.Actor), t.Description, t.Reference, string(t.Reverses))
	for i := range t.Postings {
		n += 8 + wireStrings(string(t.Postings[i].Account))
	}
	return n
}
func eventWireSize(event any) int {
	switch e := event.(type) {
	case *userCreatedEvent:
		u, a := &e.User, &e.Account
		return 16 + wireStrings(string(u.ID), u.Username, u.DisplayName, string(u.Role), string(u.Status), string(u.Account), u.Verifier, string(a.ID), string(a.UserID), a.Name, string(a.Status))
	case *userUpdatedEvent:
		return wireStrings(string(e.ID)) + wireOptional(e.DisplayName) + wireOptional((*string)(e.Status)) + wireOptional((*string)(e.Role)) + wireOptional(e.Verifier)
	case *transactionEvent:
		return transactionWireSize(&e.Txn)
	case *listingCreatedEvent:
		l := &e.Listing
		return 36 + wireStrings(string(l.ID), string(l.Seller), l.Title, l.Description, string(l.Status), string(l.Buyer), string(l.SoldTx), l.Kind, l.Currency)
	case *listingUpdatedEvent:
		n := 9 + wireStrings(string(e.ID)) + wireOptional(e.Title) + wireOptional(e.Description) + wireOptional((*string)(e.Status))
		if e.Price != nil {
			n += 8
		}
		return n
	case *listingPurchasedEvent:
		return 8 + wireStrings(string(e.ListingID), string(e.Buyer)) + transactionWireSize(&e.Txn)
	case *configUpdatedEvent:
		return 8 + wireStrings(e.Config.HouseholdName, e.Config.Currency)
	case *idempotencyEvent:
		return 10 + wireStrings(e.Key, string(e.UserID), e.Endpoint) + len(e.Result)
	default:
		return wireCapacity + 1
	}
}

// Append remains before application. Live events no longer allocate a second
// decoded object graph; replay uses exactly the same applyEvent transition.
func (s *Service) commitEvent(typ storage.RecordType, event any) error {
	n := eventWireSize(event)
	if n > wireCapacity {
		return fmt.Errorf("%w: journal event needs %d bytes (capacity %d)", ErrCapacity, n, wireCapacity)
	}
	if s.discard != nil {
		if _, err := s.discard.AppendDiscarded(typ, n); err != nil {
			return fmt.Errorf("journal append failed: %w", err)
		}
		return s.applyEvent(event)
	}
	b := s.wireBuf[:0]
	switch e := event.(type) {
	case *userCreatedEvent:
		b = encodeUserCreated(b, e)
	case *userUpdatedEvent:
		b = encodeUserUpdated(b, e)
	case *transactionEvent:
		b = encodeTransactionEvent(b, e)
	case *listingCreatedEvent:
		b = encodeListingCreated(b, e)
	case *listingUpdatedEvent:
		b = encodeListingUpdated(b, e)
	case *listingPurchasedEvent:
		b = encodeListingPurchased(b, e)
	case *configUpdatedEvent:
		b = encodeConfigUpdated(b, e)
	case *idempotencyEvent:
		b = encodeIdempotency(b, e)
	default:
		return ErrBadInput
	}
	if len(b) != n {
		panic("nanacoin: journal size calculation disagrees with codec")
	}
	if _, err := s.journal.Append(typ, b); err != nil {
		return fmt.Errorf("journal append failed: %w", err)
	}
	return s.applyEvent(event)
}
