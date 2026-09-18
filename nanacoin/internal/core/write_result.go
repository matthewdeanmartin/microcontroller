package core

import "github.com/matthewdeanmartin/microcontroller/nanacoin/internal/ledger"

// Request-owned storage. Returned transactions alias it; reuse only after the
// response is encoded. Traditional callers may omit it and own their result.
type WriteResult struct {
	Transaction ledger.Transaction
	Postings    [ledger.MaxInlinePostings]ledger.Posting
}

//go:noinline
func writeResult(storage []*WriteResult) *WriteResult {
	if len(storage) > 0 && storage[0] != nil {
		return storage[0]
	}
	return new(WriteResult)
}
