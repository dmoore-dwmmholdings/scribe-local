//! LAN discovery — announce this server over mDNS so the app can find it.
//!
//! Pairing used to require reading a URL and a 64-character token off the
//! server's terminal. Tailnet identity removed the token; this removes the URL,
//! which is the half that still forced a trip to the server.
//!
//! **What is advertised is the tailnet URL, not this machine's LAN address.**
//! The service record exists only so a phone on the same network can learn
//! where the server lives; the phone then talks to it over the tailnet as it
//! always has. That keeps `api.bind` loopback-only — the property the
//! `Tailscale-User-Login` trust in [`crate::auth`] depends on — instead of
//! trading it away for discoverability. Nothing secret is broadcast: the TXT
//! record carries a public URL, a version, and which credential the server
//! wants. Device tokens are never advertised.
//!
//! Off unless `api.advertise_lan` is set. mDNS is link-local multicast, so a
//! container on Docker's default bridge network cannot send it — the service
//! needs host networking for this to reach the LAN at all.

use mdns_sd::{ServiceDaemon, ServiceInfo};

use scribe_core::config::Config;

/// The service type browsed for by the app.
const SERVICE_TYPE: &str = "_scribe._tcp.local.";

/// A running advertisement. Dropping it retracts the record.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        // Best-effort: a shutting-down server that fails to retract its record
        // leaves a stale entry that resolves to nothing for a few minutes. Not
        // worth failing shutdown over.
        let _ = self.daemon.unregister(&self.fullname);
    }
}

/// Start advertising, or return `None` when disabled or unavailable.
///
/// Never fails the server: discovery is a convenience, and a machine with no
/// usable multicast interface should still serve the API. Problems are logged
/// and swallowed.
pub fn advertise(cfg: &Config) -> Option<Advertiser> {
    if !cfg.api.advertise_lan {
        return None;
    }

    let url = cfg.api.public_base_url.trim();
    if url.is_empty() {
        tracing::warn!("api.advertise_lan is on but api.public_base_url is empty; not advertising");
        return None;
    }
    // Advertising a loopback URL would send phones to their own device. It is
    // the default value of public_base_url, so this is the likely misconfig.
    if url.contains("127.0.0.1") || url.contains("localhost") {
        tracing::warn!(
            %url,
            "api.public_base_url is loopback; not advertising (set it to the tailnet URL)"
        );
        return None;
    }

    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "mDNS unavailable; LAN discovery disabled");
            return None;
        }
    };

    let host = hostname();
    // `auth` tells the app whether it needs a device key before it asks the
    // user for one — the difference between "tap to connect" and "paste a key".
    let auth = if cfg.auth.trust_tailscale_identity {
        "tailnet"
    } else {
        "token"
    };
    let props: [(&str, &str); 3] = [
        ("url", url),
        ("version", scribe_core::VERSION),
        ("auth", auth),
    ];

    let port = cfg
        .api
        .bind
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8443);

    let service = match ServiceInfo::new(
        SERVICE_TYPE,
        // Instance name is what a person sees in the app's list, so use the
        // machine name rather than something generic.
        &host,
        &format!("{host}.local."),
        "",
        port,
        &props[..],
    ) {
        Ok(s) => s.enable_addr_auto(),
        Err(e) => {
            tracing::warn!(error = %e, "could not build the mDNS service record");
            return None;
        }
    };

    let fullname = service.get_fullname().to_string();
    if let Err(e) = daemon.register(service) {
        tracing::warn!(error = %e, "could not register the mDNS service");
        return None;
    }

    tracing::info!(%fullname, %url, %auth, "advertising on the local network");
    Some(Advertiser { daemon, fullname })
}

/// The machine's short hostname, sanitised for use as a DNS label.
fn hostname() -> String {
    let raw = std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "scribe".to_string());

    // Keep the first label only, and restrict to characters legal in one.
    let short: String = raw
        .split('.')
        .next()
        .unwrap_or("scribe")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let trimmed = short.trim_matches('-');
    if trimmed.is_empty() {
        "scribe".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(advertise: bool, url: &str) -> Config {
        let mut c = Config::default();
        c.api.advertise_lan = advertise;
        c.api.public_base_url = url.to_string();
        c
    }

    #[test]
    fn disabled_by_default() {
        assert!(advertise(&Config::default()).is_none());
    }

    #[test]
    fn refuses_to_advertise_a_loopback_url() {
        // The default public_base_url is loopback, so enabling the flag without
        // setting a real URL would otherwise point every phone at itself.
        assert!(advertise(&cfg_with(true, "http://127.0.0.1:8443")).is_none());
        assert!(advertise(&cfg_with(true, "http://localhost:8443")).is_none());
    }

    #[test]
    fn refuses_to_advertise_an_empty_url() {
        assert!(advertise(&cfg_with(true, "   ")).is_none());
    }

    #[test]
    fn hostname_is_a_single_legal_dns_label() {
        let h = hostname();
        assert!(!h.is_empty());
        assert!(!h.contains('.'), "got {h}");
        assert!(!h.starts_with('-') && !h.ends_with('-'), "got {h}");
        assert!(
            h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "got {h}"
        );
    }
}
