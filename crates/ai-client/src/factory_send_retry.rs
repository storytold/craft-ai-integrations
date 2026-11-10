//! How the worker-backed adapters a factory builds retry a send the provider refused.
//!
//! Gemini and the OpenAI-compatible adapters send from a worker thread after `submit` returns, so
//! the client's own send retry never sees their rate limits. Each worker retries its one refused
//! send by its config's `send_retry`. This sets that rule on the builders the factory made itself;
//! a builder an app registered is left alone.

use std::sync::Arc;

use craft_ai_core::Backoff;

use super::{ProviderFactory, build_gemini, build_openai};

impl ProviderFactory {
    /// Sets how the Gemini and OpenAI-compatible adapters this factory builds retry a send the
    /// provider refused (default [`Backoff::STANDARD`]; [`Backoff::NEVER`] turns it off). Only the
    /// factory's own builders change, keeping what was declared for them; a name an app has
    /// registered itself keeps its builder. [`AiClient::with_retries`](crate::AiClient::with_retries)
    /// calls this with its send rule.
    pub fn set_send_retry(&mut self, retry: Backoff) {
        let timeouts = self.timeouts;
        for name in self.standard_senders.clone() {
            let builder: Arc<super::BuildFn> = if name == "gemini" {
                Arc::new(move |c, s| build_gemini(c, s, timeouts, retry))
            } else {
                Arc::new(move |c, s| build_openai(c, s, timeouts, retry))
            };
            self.builders.insert(name, builder);
        }
    }
}
