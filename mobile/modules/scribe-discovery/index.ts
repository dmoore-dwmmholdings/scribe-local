/**
 * Find Scribe servers on the local network.
 *
 * Pairing otherwise means reading a URL off the server's terminal. The server
 * advertises `_scribe._tcp` with a TXT record naming the URL it wants to be
 * reached on — its tailnet address — so a phone on the same Wi-Fi can discover
 * where the server lives and then talk to it over the tailnet as usual. The LAN
 * is only used to ask the question, never to carry data.
 *
 * iOS only. `isAvailable` is false elsewhere; callers fall back to typing a URL.
 *
 * Requires `NSLocalNetworkUsageDescription` and `NSBonjourServices` in the
 * Info.plist (set in app.json) — without them iOS returns an empty list rather
 * than an error.
 */

import { requireOptionalNativeModule } from 'expo';

export interface DiscoveredServer {
  /** Bonjour instance name — the server's machine name. */
  name: string;
  /** The base URL the server wants to be reached on (its tailnet address). */
  url: string;
  /** Server version, when advertised. */
  version?: string;
  /**
   * Which credential the server wants:
   * `tailnet` — none needed, it authenticates by tailnet identity;
   * `token`   — a device key is still required.
   */
  auth?: 'tailnet' | 'token';
}

interface ScribeDiscoveryNativeModule {
  discover(timeoutMs: number): Promise<DiscoveredServer[]>;
  stop(): void;
}

const native = requireOptionalNativeModule<ScribeDiscoveryNativeModule>('ScribeDiscovery');

/** Whether local-network discovery is available on this platform. */
export const isAvailable = native != null;

/**
 * Browse for servers for `timeoutMs`, then resolve with what was seen.
 *
 * Resolves empty rather than throwing when nothing answers — "none found" is a
 * normal outcome (wrong network, server not advertising, permission declined),
 * and the caller shows guidance for it. It rejects only when the browse itself
 * fails.
 */
export async function discover(timeoutMs = 3000): Promise<DiscoveredServer[]> {
  if (!native) return [];
  return native.discover(timeoutMs);
}

/** Cancel an in-flight browse. */
export function stop(): void {
  native?.stop();
}
