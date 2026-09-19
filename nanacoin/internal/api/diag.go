package api

import (
	"net/http"
)

// Diagnostics is what the host knows about its own health and its last
// failure. The desktop has nothing to report; the board has the thing that
// matters most.
//
// # Why this is an endpoint and not a log line
//
// The board runs downstairs next to the router with no cable attached,
// because that is the only place the WiFi is good enough to serve from. So
// the serial console - which is where every diagnostic in this program used
// to go - reaches nobody. The event log helps, but it lives in RAM and is
// gone after the reboot that follows a crash.
//
// That leaves a gap exactly where the information is most valuable: the board
// dies, reboots, comes back up empty, and the only evidence of what happened
// is a health header on a response that was never sent. This endpoint closes
// it by reporting what the *previous* run recorded before it stopped.
type Diagnostics struct {
	// LastBoot describes how the previous run ended, in one sentence.
	LastBoot string `json:"last_boot"`

	// Crashed reports whether a usable crash record survived. False means a
	// power cycle or a fresh flash, which is not a crash and must not be
	// displayed as one.
	Crashed bool `json:"crashed"`

	// Boots counts resets since the last power cycle. A number climbing on
	// its own is a board rebooting in a loop - which from a browser looks
	// identical to a board that is merely slow.
	Boots uint32 `json:"boots"`

	// Phase and Route say what was in flight when the previous run stopped.
	Phase string `json:"phase,omitempty"`
	Route string `json:"route,omitempty"`

	// ServedBeforeCrash is how many connections the previous run handled. It
	// separates "died on the third request" from "died after four hundred",
	// which point at completely different causes.
	ServedBeforeCrash uint32 `json:"served_before_crash"`

	// HeapFreeAtCrash is the headroom recorded when the fatal phase was
	// entered, in bytes.
	HeapFreeAtCrash uint64 `json:"heap_free_at_crash"`

	// AllocFailures counts responses this run could not write, with the
	// worst headroom seen at those moments. Non-zero here with a healthy
	// HeapFree elsewhere is the signature of fragmentation rather than
	// exhaustion.
	AllocFailures uint32 `json:"alloc_failures"`
	WorstHeadroom uint64 `json:"worst_headroom"`

	// Uptime and Health are the current run's figures.
	UptimeSeconds int64  `json:"uptime_seconds"`
	Health        string `json:"health,omitempty"`

	// The heap trend: where it was, where it is, and how fast it is moving.
	//
	// This is the "what happened just before it went down" answer. The board
	// cannot report its own death - an out-of-memory aborts before anything
	// can run - but it samples the heap on every connection and keeps the
	// last few dozen readings, so a client polling this endpoint as it loads
	// sees the approach rather than only the arrival.
	//
	// Read FreeNow against FragNow. Free bytes holding steady while Frag
	// falls toward 100 means the heap is shattering into single-block
	// objects: the totals look fine and the next large allocation still
	// fails. That is the state this board dies in, and no single number
	// shows it.
	FreeNow    uint64 `json:"free_now"`
	FreeOldest uint64 `json:"free_oldest"`
	FreeDrop   int64  `json:"free_drop"`
	Samples    int    `json:"samples"`

	// ObjectsNow is live allocation count; FragNow is average blocks per
	// object x100, so 100 is one block each.
	ObjectsNow uint64 `json:"objects_now"`
	FragNow    uint32 `json:"frag_now"`
	FragOldest uint32 `json:"frag_oldest"`

	// GCsNow and GCRate say whether the collector is keeping up. A rate
	// climbing while free falls means it is running constantly and losing.
	GCsNow  uint32 `json:"gcs_now"`
	GCDelta uint32 `json:"gc_delta"`
}

// DiagnosticsFunc reports host diagnostics. Registered by the host; a nil one
// means the endpoint reports only what the API layer itself knows.
type DiagnosticsFunc func() Diagnostics

// SetDiagnostics registers the host's reporter.
func (s *Server) SetDiagnostics(fn DiagnosticsFunc) {
	s.diag = fn
}

// handleDiag serves the crash record and current health.
//
// Unauthenticated, like /logs and for the same reason: the commonest thing to
// diagnose is a board that will not let anyone authenticate, and a diagnostic
// behind authentication is unavailable exactly when it is needed. It carries
// no balances, no usernames and no tokens - a phase, a route class, and some
// counters.
func (s *Server) handleDiag(w http.ResponseWriter, r *http.Request) {
	var d Diagnostics
	if s.diag != nil {
		d = s.diag()
	}
	if d.Health == "" {
		d.Health = s.healthLine()
	}
	encodeJSON(w, http.StatusOK, func(j *jsonw) { j.diagnostics(&d) })
}
