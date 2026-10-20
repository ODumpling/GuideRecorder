// ULIDs for guides and steps made on the phone, in the format
// guiderecorder_core's `Ulid` parses (as app/src/lib/ulid.ts): 26 Crockford
// base32 characters, a 48-bit millisecond time and 80 random bits.

import { getRandomValues } from "expo-crypto";

const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

export function ulid(now: number = Date.now(), random: (bytes: Uint8Array) => void = (bytes) => void getRandomValues(bytes)): string {
  let time = "";
  for (let i = 0, t = now; i < 10; i++, t = Math.floor(t / 32)) {
    time = ALPHABET[t % 32] + time;
  }
  // 80 bits: 16 characters of 5 bits from 10 random bytes.
  const bytes = new Uint8Array(10);
  random(bytes);
  let bits = 0n;
  for (const byte of bytes) bits = (bits << 8n) | BigInt(byte);
  let rest = "";
  for (let i = 0; i < 16; i++, bits >>= 5n) rest = ALPHABET[Number(bits & 31n)] + rest;
  return time + rest;
}
