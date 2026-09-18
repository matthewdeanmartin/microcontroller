package core

import (
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/auth"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/ledger"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/marketplace"
	"github.com/matthewdeanmartin/microcontroller/nanacoin/internal/storage"
)

var (
	ErrForbidden      = errors.New("not permitted")
	ErrUserNotFound   = errors.New("user not found")
	ErrAccountUnknown = errors.New("account not found")
	ErrListingUnknown = errors.New("listing not found")
	ErrListingClosed  = errors.New("listing is not active")
	ErrSelfDeal       = errors.New("cannot trade with yourself")
	ErrUsernameTaken  = errors.New("username already in use")
	ErrBadInput       = errors.New("invalid input")
	ErrDisabled       = errors.New("account is disabled")

	// ErrCapacity is a fixed table refusing to grow.
	//
	// The packed store preallocates everything, so "full" is a real and
	// reachable state rather than a theoretical one. Refusing is the correct
	// behaviour and the whole point of the design: a bounded system that
	// says no is usable, and an unbounded one that falls over is not.
	ErrCapacity = errors.New("storage is full")
)

// Limits on free text. Bounded so that a single memo cannot fill a flash
// partition, and so that MaxPayload is never the thing that rejects a record.
const (
	MaxMemoLen        = 140
	MaxTitleLen       = 80
	MaxDescriptionLen = 500
	MaxNameLen        = 40
	MaxIdempotencyKey = 80
)

// IDGen produces unique identifiers. Injectable so tests get deterministic IDs
// and the board can use a cheaper source than crypto/rand if it needs to.
type IDGen func(prefix string) string

// Service owns all mutable state. One mutex covers everything: at a household's
// transaction rate, finer-grained locking would be more code and more bugs for
// no measurable benefit, and a single lock makes "purchase is atomic" true by
// construction rather than by argument.
type Service struct {
	mu sync.Mutex
	// Lock order: idempotency stripe, then mu. No callback may nest Idempotent.
	idemGates [8]sync.Mutex

	journal storage.Journal
	discard storage.DiscardJournal
	book    *ledger.Book
	cfg     Config

	// Domain objects live in fixed, pointer-free arrays rather than maps of
	// pointers. See store.go for why: the maps were the fragmentation, not
	// the ledger.
	store *store

	// Fixed receipt storage; no cache-owned bytes escape the service lock.
	idem *receiptCache

	now   func() time.Time
	newID IDGen

	// wireBuf backs retained event encoding, reused across commits.
	// Discard-only journals leave it nil.
	//
	// Every mutation encodes one event; a fresh buffer per commit would be a
	// per-write allocation of exactly the kind this format exists to remove.
	// Held under the service lock like everything else here, so one buffer is
	// safe.
	//
	// Startup capacity includes a full retry receipt. Oversized events are
	// rejected before append or state changes; this buffer never grows.
	wireBuf []byte
}

type Options struct {
	Now   func() time.Time
	NewID IDGen
}

// New opens a service over a journal, replaying it to rebuild state. A fresh
// journal yields an unprovisioned service; see Provision.
func New(j storage.Journal, o Options) (*Service, error) {
	if o.Now == nil {
		o.Now = time.Now
	}
	if o.NewID == nil {
		o.NewID = defaultIDGen
	}
	// One intern table and one text arena, shared by the ledger and the
	// domain store so that an account ID mentioned in both is stored once.
	strs := ledger.NewStrings()
	arena := ledger.NewArena()

	s := &Service{
		journal: j,
		book:    ledger.NewBookWithAccountCapacity(strs, arena, MaxAccounts+1),
		cfg:     DefaultConfig(),
		store:   newStore(strs, arena),
		idem:    new(receiptCache),

		now:   o.Now,
		newID: o.NewID,
	}
	if d, ok := j.(storage.DiscardJournal); ok && d.DiscardsPayload() {
		s.discard = d
	} else {
		s.wireBuf = make([]byte, wireCapacity)
	}
	if err := j.Replay(s.applyRecord); err != nil {
		return nil, fmt.Errorf("replaying journal: %w", err)
	}
	// A ledger that does not balance after replay is not something to serve
	// requests against. Fail loudly at boot rather than quietly at audit
	// time.
	if err := s.book.CheckInvariants(); err != nil {
		return nil, fmt.Errorf("ledger invariants violated after replay: %w", err)
	}
	return s, nil
}

func defaultIDGen(prefix string) string {
	tok, err := auth.RandomToken(9)
	if err != nil {
		// crypto/rand failing is not a condition worth a recovery path;
		// a server that cannot generate a transaction ID cannot serve.
		panic("nanacoin: no randomness available: " + err.Error())
	}
	return prefix + "-" + tok
}

// Replay decodes persisted bytes; live commits apply their already-typed event.
// Both paths share applyEvent so the state transition cannot drift.
func (s *Service) applyRecord(r *storage.Record) error {
	switch r.Type {
	case storage.TypeUserCreated:
		var e userCreatedEvent
		if err := decodeUserCreated(r.Payload, &e); err != nil {
			return err
		}
		return s.applyEvent(&e)
	case storage.TypeUserUpdated:
		var e userUpdatedEvent
		if err := decodeUserUpdated(r.Payload, &e); err != nil {
			return err
		}
		return s.applyEvent(&e)
	case storage.TypeTransactionCreated:
		var e transactionEvent
		if err := decodeTransactionEvent(r.Payload, &e); err != nil {
			return err
		}
		return s.applyEvent(&e)
	case storage.TypeListingCreated:
		var e listingCreatedEvent
		if err := decodeListingCreated(r.Payload, &e); err != nil {
			return err
		}
		return s.applyEvent(&e)
	case storage.TypeListingUpdated, storage.TypeListingCancelled:
		var e listingUpdatedEvent
		if err := decodeListingUpdated(r.Payload, &e); err != nil {
			return err
		}
		return s.applyEvent(&e)
	case storage.TypeListingPurchased:
		var e listingPurchasedEvent
		if err := decodeListingPurchased(r.Payload, &e); err != nil {
			return err
		}
		return s.applyEvent(&e)
	case storage.TypeConfigUpdated:
		var e configUpdatedEvent
		if err := decodeConfigUpdated(r.Payload, &e); err != nil {
			return err
		}
		return s.applyEvent(&e)
	case storage.TypeIdempotency:
		var e idempotencyEvent
		if err := decodeIdempotency(r.Payload, &e); err != nil {
			return err
		}
		return s.applyEvent(&e)
	case storage.TypeSnapshot:
		return nil
	default:
		return fmt.Errorf("unknown record type %s", r.Type)
	}
}

func (s *Service) applyEvent(event any) error {
	switch e := event.(type) {
	case *userCreatedEvent:
		u := e.User
		a := e.Account
		if _, ok := s.store.putUser(&u); !ok {
			return fmt.Errorf("%w: household is full (%d users)",
				ErrCapacity, MaxUsers)
		}
		if _, ok := s.store.putAccount(&a); !ok {
			return fmt.Errorf("%w: no room for another account (%d)",
				ErrCapacity, MaxAccounts)
		}
	case *userUpdatedEvent:
		i := s.store.findUser(e.ID)
		if i < 0 {
			return fmt.Errorf("update for unknown user %s", e.ID)
		}
		// Unpack, apply, repack. The record is rewritten in place, so no
		// new slot is consumed and nothing is left behind.
		u := s.store.unpackUser(i)
		if e.DisplayName != nil {
			u.DisplayName = *e.DisplayName
		}
		if e.Status != nil {
			u.Status = *e.Status
		}
		if e.Role != nil {
			u.Role = *e.Role
		}
		if e.Verifier != nil {
			u.Verifier = *e.Verifier
		}
		s.store.writeUser(i, u)
	case *transactionEvent:
		t := e.Txn
		if _, err := s.book.Replay(&t); err != nil {
			return fmt.Errorf("replaying transaction %s: %w", t.ID, err)
		}
	case *listingCreatedEvent:
		l := e.Listing
		if _, ok := s.store.putListing(&l); !ok {
			return fmt.Errorf("%w: marketplace is full (%d listings, all active)",
				ErrCapacity, MaxListings)
		}
	case *listingUpdatedEvent:
		i := s.store.findListing(e.ID)
		if i < 0 {
			return fmt.Errorf("update for unknown listing %s", e.ID)
		}
		l := s.store.unpackListing(i)
		if e.Title != nil {
			l.Title = *e.Title
		}
		if e.Description != nil {
			l.Description = *e.Description
		}
		if e.Price != nil {
			l.Price = *e.Price
		}
		if e.Status != nil {
			l.Status = *e.Status
		}
		l.UpdatedAt = e.UpdatedAt
		if !s.store.writeListing(i, l) {
			return ErrCapacity
		}
	case *listingPurchasedEvent:
		i := s.store.findListing(e.ListingID)
		if i < 0 {
			return fmt.Errorf("purchase of unknown listing %s", e.ListingID)
		}
		t := e.Txn
		if _, err := s.book.Replay(&t); err != nil {
			return fmt.Errorf("replaying purchase %s: %w", t.ID, err)
		}
		l := s.store.unpackListing(i)
		l.Status = marketplace.StatusSold
		l.Buyer = e.Buyer
		l.SoldTx = t.ID
		l.UpdatedAt = e.UpdatedAt
		if !s.store.writeListing(i, l) {
			return ErrCapacity
		}
	case *configUpdatedEvent:
		s.cfg = e.Config
	case *idempotencyEvent:
		// Through the bounded setter, so replaying a long journal cannot
		// rebuild an unbounded map at boot.
		s.idem.remember(e.UserID, e.Endpoint, e.Key, e.Result)
	default:
		return fmt.Errorf("unknown event type %T", event)
	}
	return nil
}
