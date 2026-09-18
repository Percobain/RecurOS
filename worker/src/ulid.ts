// ULID generation (48-bit ms timestamp + 80 random bits, Crockford base32).
// Monotonic within one isolate: two ids in the same millisecond increment the
// random part, so ids stay sortable in creation order.

const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

let lastTime = -1;
let lastRandom: Uint8Array = new Uint8Array(10);

function encodeTime(ms: number): string {
  let out = "";
  let t = ms;
  for (let i = 0; i < 10; i++) {
    out = ALPHABET[t % 32] + out;
    t = Math.floor(t / 32);
  }
  return out;
}

// 80 bits → 16 base32 chars, most significant first.
function encodeRandom(bytes: Uint8Array): string {
  let bits = 0n;
  for (const b of bytes) bits = (bits << 8n) | BigInt(b);
  let out = "";
  for (let i = 0; i < 16; i++) {
    out = ALPHABET[Number(bits & 31n)] + out;
    bits >>= 5n;
  }
  return out;
}

function increment(bytes: Uint8Array): Uint8Array {
  const next = bytes.slice();
  for (let i = next.length - 1; i >= 0; i--) {
    if (next[i]! < 255) {
      next[i]! += 1;
      return next;
    }
    next[i] = 0;
  }
  return next; // 2^80 ids in one ms: wrap is not a practical concern
}

export function ulid(now: number = Date.now()): string {
  if (now <= lastTime) {
    lastRandom = increment(lastRandom);
    now = lastTime;
  } else {
    lastRandom = new Uint8Array(10);
    crypto.getRandomValues(lastRandom);
    lastTime = now;
  }
  return encodeTime(now) + encodeRandom(lastRandom);
}
