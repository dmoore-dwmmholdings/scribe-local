/**
 * Crypto polyfills for React Native / Hermes.
 *
 * `react-native-get-random-values` installs `crypto.getRandomValues`, but it
 * does NOT install `crypto.randomUUID`, and Hermes does not provide one
 * either.  Call sites that use `crypto.randomUUID()` therefore throw
 * "crypto.randomUUID is not a function" on device.
 *
 * Importing this module installs both.  Import it instead of importing
 * `react-native-get-random-values` directly.
 */

import 'react-native-get-random-values';

if (typeof crypto.randomUUID !== 'function') {
  // RFC 4122 §4.4 — random UUID v4, built on the getRandomValues polyfill.
  crypto.randomUUID = function randomUUID() {
    const bytes = crypto.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10xx

    let hex = '';
    for (let i = 0; i < bytes.length; i++) {
      hex += bytes[i].toString(16).padStart(2, '0');
    }

    return [
      hex.slice(0, 8),
      hex.slice(8, 12),
      hex.slice(12, 16),
      hex.slice(16, 20),
      hex.slice(20, 32),
    ].join('-') as ReturnType<Crypto['randomUUID']>;
  };
}
