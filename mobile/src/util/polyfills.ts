// Runtime polyfills for the React Native (Hermes) JS engine. Import this FIRST,
// before any other app module, so the globals exist before anything uses them.

// Provides `crypto.getRandomValues` (Hermes has no Web Crypto).
import 'react-native-get-random-values';

// Hermes also has no `crypto.randomUUID` (only `getRandomValues` after the
// import above). Polyfill it as an RFC-4122 v4 UUID built from random bytes, so
// every `crypto.randomUUID()` call site — and any dependency that expects it —
// works. No-op where a real implementation already exists.
const g = globalThis as unknown as {
  crypto?: { getRandomValues?: (a: Uint8Array) => Uint8Array; randomUUID?: () => string };
};

if (g.crypto && typeof g.crypto.randomUUID !== 'function' && g.crypto.getRandomValues) {
  g.crypto.randomUUID = function randomUUID(): string {
    const b = new Uint8Array(16);
    g.crypto!.getRandomValues!(b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10xx
    const h = Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
  };
}
