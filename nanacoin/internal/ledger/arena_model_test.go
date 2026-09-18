package ledger

import (
	"fmt"
	"math/rand"
	"testing"
)

func TestArenaChurnPreservesOtherOwners(t *testing.T) {
	a := NewArena()
	rng := rand.New(rand.NewSource(73))
	var slots [80]Slot
	var texts [80]string
	for step := 0; step < 5000; step++ {
		i := rng.Intn(len(slots))
		a.Release(slots[i])
		texts[i] = fmt.Sprintf("owner-%d-step-%d-value-%d", i, step, rng.Int63())
		slots[i] = a.Put(texts[i])
		if step%31 != 0 {
			continue
		}
		expected := 0
		for j := range slots {
			expected += len(texts[j])
			if got := a.Get(slots[j]); got != texts[j] {
				t.Fatalf("step %d owner %d corrupted: %q", step, j, got)
			}
		}
		used, _, truncated := a.Stats()
		if used != expected || truncated != 0 {
			t.Fatalf("accounting: used=%d expected=%d truncated=%d", used, expected, truncated)
		}
	}
	for i := range slots {
		a.Release(slots[i])
	}
	if used, _, _ := a.Stats(); used != 0 {
		t.Fatal("released arena leaks text", used)
	}
	if !a.CanStore(string(make([]byte, ArenaSize)), "") {
		t.Fatal("arena cannot reuse full capacity")
	}
}
