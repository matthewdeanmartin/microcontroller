"""A small pure-Python AES-128, used ONLY by the local dev server.

The board does not import this. MicroPython ships `cryptolib`, which does AES
in C; compat.py reaches for that first and only falls back to this file when
running under CPython, where the standard library has no AES at all.

Why not just `pip install pycryptodome`? Because `python dev_server.py` should
work on a fresh checkout with nothing installed, the same way the rest of this
repo does. ~120 lines of table-driven AES is a smaller price than a dependency
that only exists to preview a page.

Speed is irrelevant here: it encrypts a handful of short messages on a desktop
CPU. Do not use this on the board - it would be roughly a thousand times
slower than cryptolib, on the machine that can least afford it.

This implements the AES block cipher only. The mode of operation (CTR) lives
in compat.py, shared by both backends, so the two paths cannot drift.
"""

# The AES S-box and its inverse are fixed constants from the specification.
# Built at import rather than pasted as 256-entry literals: it is less code to
# read, and the derivation doubles as documentation of where they come from.

def _build_tables():
    sbox = [0] * 256
    inv = [0] * 256
    p = q = 1
    while True:
        # Multiply p by 3 in GF(2^8).
        p = p ^ ((p << 1) & 0xFF) ^ (0x1B if p & 0x80 else 0)

        # Divide q by 3 (multiply by the inverse of 3).
        q ^= (q << 1) & 0xFF
        q ^= (q << 2) & 0xFF
        q ^= (q << 4) & 0xFF
        if q & 0x80:
            q ^= 0x09

        x = q ^ ((q << 1) | (q >> 7)) & 0xFF
        x ^= ((q << 2) | (q >> 6)) & 0xFF
        x ^= ((q << 3) | (q >> 5)) & 0xFF
        x ^= ((q << 4) | (q >> 4)) & 0xFF
        x = (x ^ 0x63) & 0xFF

        sbox[p] = x
        inv[x] = p

        if p == 1:
            break

    sbox[0] = 0x63
    inv[0x63] = 0
    return sbox, inv


SBOX, INV_SBOX = _build_tables()

# Round constants for the key schedule: 2^i in GF(2^8).
RCON = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36]


def _xtime(a):
    """Multiply by 2 in GF(2^8)."""
    a <<= 1
    if a & 0x100:
        a = (a ^ 0x1B) & 0xFF
    return a


def _mul(a, b):
    """Multiply two bytes in GF(2^8). Only used with small constants."""
    result = 0
    while b:
        if b & 1:
            result ^= a
        a = _xtime(a)
        b >>= 1
    return result & 0xFF


class AES128:
    """AES-128 single-block encrypt/decrypt.

    CTR mode only ever calls encrypt_block (it encrypts the counter, never the
    data), so decrypt_block exists for completeness and is not on the hot path.
    """

    def __init__(self, key):
        if len(key) != 16:
            raise ValueError("AES128 needs a 16-byte key")
        self._round_keys = self._expand_key(key)

    @staticmethod
    def _expand_key(key):
        """Derive 11 round keys of 16 bytes each from the 16-byte key."""
        words = [list(key[i * 4:i * 4 + 4]) for i in range(4)]

        for i in range(4, 44):
            temp = list(words[i - 1])
            if i % 4 == 0:
                # Rotate, substitute, then XOR in the round constant.
                temp = temp[1:] + temp[:1]
                temp = [SBOX[b] for b in temp]
                temp[0] ^= RCON[i // 4 - 1]
            words.append([words[i - 4][j] ^ temp[j] for j in range(4)])

        return [
            bytes(b for w in words[r * 4:r * 4 + 4] for b in w)
            for r in range(11)
        ]

    @staticmethod
    def _add_round_key(state, round_key):
        return [state[i] ^ round_key[i] for i in range(16)]

    @staticmethod
    def _shift_rows(s):
        # The state is column-major: byte i is row i%4, column i//4.
        return [
            s[0], s[5], s[10], s[15],
            s[4], s[9], s[14], s[3],
            s[8], s[13], s[2], s[7],
            s[12], s[1], s[6], s[11],
        ]

    @staticmethod
    def _inv_shift_rows(s):
        return [
            s[0], s[13], s[10], s[7],
            s[4], s[1], s[14], s[11],
            s[8], s[5], s[2], s[15],
            s[12], s[9], s[6], s[3],
        ]

    @staticmethod
    def _mix_columns(s):
        out = []
        for c in range(4):
            col = s[c * 4:c * 4 + 4]
            out.append(_mul(col[0], 2) ^ _mul(col[1], 3) ^ col[2] ^ col[3])
            out.append(col[0] ^ _mul(col[1], 2) ^ _mul(col[2], 3) ^ col[3])
            out.append(col[0] ^ col[1] ^ _mul(col[2], 2) ^ _mul(col[3], 3))
            out.append(_mul(col[0], 3) ^ col[1] ^ col[2] ^ _mul(col[3], 2))
        return out

    @staticmethod
    def _inv_mix_columns(s):
        out = []
        for c in range(4):
            col = s[c * 4:c * 4 + 4]
            out.append(_mul(col[0], 14) ^ _mul(col[1], 11) ^ _mul(col[2], 13) ^ _mul(col[3], 9))
            out.append(_mul(col[0], 9) ^ _mul(col[1], 14) ^ _mul(col[2], 11) ^ _mul(col[3], 13))
            out.append(_mul(col[0], 13) ^ _mul(col[1], 9) ^ _mul(col[2], 14) ^ _mul(col[3], 11))
            out.append(_mul(col[0], 11) ^ _mul(col[1], 13) ^ _mul(col[2], 9) ^ _mul(col[3], 14))
        return out

    def encrypt_block(self, block):
        """Encrypt exactly 16 bytes."""
        state = self._add_round_key(list(block), self._round_keys[0])

        for rnd in range(1, 10):
            state = [SBOX[b] for b in state]
            state = self._shift_rows(state)
            state = self._mix_columns(state)
            state = self._add_round_key(state, self._round_keys[rnd])

        # The final round omits MixColumns.
        state = [SBOX[b] for b in state]
        state = self._shift_rows(state)
        state = self._add_round_key(state, self._round_keys[10])

        return bytes(state)

    def decrypt_block(self, block):
        """Decrypt exactly 16 bytes. Unused by CTR mode; kept for symmetry."""
        state = self._add_round_key(list(block), self._round_keys[10])

        for rnd in range(9, 0, -1):
            state = self._inv_shift_rows(state)
            state = [INV_SBOX[b] for b in state]
            state = self._add_round_key(state, self._round_keys[rnd])
            state = self._inv_mix_columns(state)

        state = self._inv_shift_rows(state)
        state = [INV_SBOX[b] for b in state]
        state = self._add_round_key(state, self._round_keys[0])

        return bytes(state)
