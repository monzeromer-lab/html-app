//! Fetching pinned remote modules (PRD §8.5).
//!
//! This is the *only* network access the runtime performs on its own behalf, it happens only when a
//! document's manifest names an import that is not already cached, and the response is refused
//! unless it matches the integrity hash the manifest pinned. §4.2 N5 — "the runtime never phones
//! home, never auto-updates, never fetches a remote runtime" — holds: nothing here is discretionary.

use htmlapp_engine::imports::{ImportError, ModuleFetcher};

pub struct HttpFetcher {
    agent: ureq::Agent,
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFetcher {
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(30)))
                .user_agent(concat!("htmlapp/", env!("CARGO_PKG_VERSION")))
                .build()
                .into(),
        }
    }
}

impl ModuleFetcher for HttpFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, ImportError> {
        // Modules are code. Fetching one over plaintext would let anyone on the path substitute
        // it, and while the integrity hash would catch that, failing closed here is clearer.
        if !url.starts_with("https://") {
            return Err(ImportError::Fetch {
                url: url.to_string(),
                reason: "imports must be fetched over https".into(),
            });
        }

        let mut response = self.agent.get(url).call().map_err(|e| ImportError::Fetch {
            url: url.to_string(),
            reason: e.to_string(),
        })?;

        // Bounded so a hostile or misconfigured server cannot exhaust memory before the integrity
        // check ever gets a chance to reject the body.
        const MAX_MODULE_BYTES: u64 = 32 * 1024 * 1024;
        response
            .body_mut()
            .with_config()
            .limit(MAX_MODULE_BYTES)
            .read_to_vec()
            .map_err(|e| ImportError::Fetch {
                url: url.to_string(),
                reason: e.to_string(),
            })
    }
}
