/**
 * Deep-link pairing — `scribe://pair?url=…&key=…`.
 *
 * Onboarding used to mean copying a 64-hex-character device token onto a phone
 * keyboard, which is not a flow any real user completes. A pairing link carries
 * the same values in something scannable, and the server prints it as a QR code
 * on startup.
 *
 * `key` is optional on purpose. When the API runs with
 * `auth.trust_tailscale_identity`, a phone on the tailnet needs no token at all,
 * so the link degenerates to just the server URL.
 *
 * Nothing here trusts the link blindly: a pairing URL is a credential, and
 * anything that can open a URL on the phone can offer one. `parsePairingLink`
 * validates the shape and the caller confirms with the user before saving.
 */

import * as Linking from 'expo-linking';

export interface PairingPayload {
  baseUrl: string;
  /** Empty when the server authenticates by tailnet identity instead. */
  deviceKey: string;
}

/**
 * Parse a `scribe://pair` URL, or return null when it is not one / is malformed.
 *
 * Rejects a non-HTTPS `url=` unless it points at loopback: the pairing link is
 * how the phone learns where to send every recording, and silently accepting
 * `http://` would let a stray link downgrade the transport. Loopback stays
 * allowed so the simulator can talk to a dev server.
 */
export function parsePairingLink(link: string): PairingPayload | null {
  let parsed: ReturnType<typeof Linking.parse>;
  try {
    parsed = Linking.parse(link);
  } catch {
    return null;
  }

  // expo-router serves `scribe://pair` as hostname `pair` with an empty path,
  // but `scribe:///pair` as path `/pair` — accept both rather than depending on
  // which form the QR encoder emitted.
  const target = (parsed.hostname ?? '') || (parsed.path ?? '').replace(/^\//, '');
  if (target !== 'pair') return null;

  const rawUrl = firstParam(parsed.queryParams?.url);
  if (!rawUrl) return null;

  let url: URL;
  try {
    url = new URL(rawUrl);
  } catch {
    return null;
  }

  const isLoopback = url.hostname === 'localhost' || url.hostname === '127.0.0.1';
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && isLoopback)) {
    return null;
  }

  return {
    // Trailing slash stripped to match what Settings persists, so a paired URL
    // and a typed one compare equal.
    baseUrl: rawUrl.trim().replace(/\/$/, ''),
    deviceKey: (firstParam(parsed.queryParams?.key) ?? '').trim(),
  };
}

/** expo-linking gives `string | string[] | undefined` per query param. */
function firstParam(v: string | string[] | undefined | null): string | null {
  if (Array.isArray(v)) return v[0] ?? null;
  return v ?? null;
}

/**
 * A short, human-checkable description of what a link would connect to, for the
 * confirmation prompt. The key is never shown in full — the point is to let the
 * user recognise their own server, not to put the secret back on screen.
 */
export function describePairing(p: PairingPayload): string {
  const auth = p.deviceKey
    ? `Device key ending …${p.deviceKey.slice(-4)}`
    : 'No device key — the server authenticates this phone by its tailnet identity.';
  return `${p.baseUrl}\n\n${auth}`;
}
