//! Every real name `@ TELL` could currently resolve — for live completion
//! only (`xact-completion-graph` calls [`known_targets`]); `tell` itself
//! re-resolves independently at execution time and never trusts this
//! list as authorization to run something.
//!
//! Each network/subprocess-backed source is cached per-process with a
//! TTL, the same shape as `xact-resolve`'s `GLS_CACHE`
//! (`crates/xact-resolve/src/lib.rs`) — a
//! `LazyLock<Mutex<HashMap<...>>>` reused until it goes stale — except a
//! live API response has no filesystem mtime to invalidate against, so
//! an `Instant` + fixed TTL stands in for that. A source that's
//! unavailable (no API key set, network down, binary missing) simply
//! contributes nothing; it never fails the whole catalog for the other
//! sources.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::{anthropic, groq, harness, moonshot, ollama, openai, openrouter, xai, zhipu};

/// Long enough that opening the completion menu repeatedly while typing
/// one command never re-hits the network, short enough that a model
/// pulled or a key exported mid-session shows up without restarting
/// `xact`.
const TTL: Duration = Duration::from_secs(300);

/// When each source was last fetched, and what it returned.
type Cache = HashMap<&'static str, (Instant, Vec<String>)>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// `fetch`'s result for `key`, reused until [`TTL`] elapses.
fn cached(key: &'static str, fetch: impl FnOnce() -> Vec<String>) -> Vec<String> {
    if let Some((fetched_at, values)) = CACHE.lock().unwrap().get(key) {
        if fetched_at.elapsed() < TTL {
            return values.clone();
        }
    }
    let values = fetch();
    CACHE.lock().unwrap().insert(key, (Instant::now(), values.clone()));
    values
}

/// Installed Ollama models, cached — the same list [`super::ollama::run`]
/// checks against at execution time, so completion and execution can
/// never disagree about what's actually installed.
pub fn ollama_models() -> Vec<String> {
    cached("ollama", ollama::installed_models_uncached)
}

/// OpenAI's models for the active `OPENAI_API_KEY`, cached — empty if
/// that key isn't set.
pub fn openai_models() -> Vec<String> {
    cached("openai", openai::models)
}

/// Anthropic's models for the active `ANTHROPIC_API_KEY`, cached — empty
/// if that key isn't set.
pub fn anthropic_models() -> Vec<String> {
    cached("anthropic", anthropic::models)
}

/// Groq's models for the active `GROQ_API_KEY`, cached — empty if that
/// key isn't set.
pub fn groq_models() -> Vec<String> {
    cached("groq", groq::models)
}

/// xAI's models for the active `XAI_API_KEY`, cached — empty if that key
/// isn't set.
pub fn xai_models() -> Vec<String> {
    cached("xai", xai::models)
}

/// OpenRouter's catalog, cached — empty if `OPENROUTER_API_KEY` isn't
/// set (see `openrouter`'s own doc comment for why that gate stays even
/// though the underlying endpoint is actually public).
pub fn openrouter_models() -> Vec<String> {
    cached("openrouter", openrouter::models)
}

/// Moonshot's (Kimi's API, not the `kimi` harness) models for the active
/// `MOONSHOT_API_KEY`, cached — empty if that key isn't set.
pub fn moonshot_models() -> Vec<String> {
    cached("moonshot", moonshot::models)
}

/// z.ai's (Zhipu's) known GLM models, cached — empty if `ZAI_API_KEY`
/// isn't set. Cached the same way as every live-fetched source even
/// though this one's a hardcoded list (see `zhipu`'s own doc comment) —
/// consistent behavior matters more here than the trivial cost saved.
pub fn zhipu_models() -> Vec<String> {
    cached("zhipu", zhipu::models)
}

/// Every real target name `TELL` could resolve right now: installed
/// Ollama models, models from any provider whose API key is set, and any
/// registered harness found on `$PATH` — real, not a fabricated
/// "supported models" list independent of what's actually reachable.
pub fn known_targets() -> Vec<String> {
    let mut targets = ollama_models();
    targets.extend(openai_models());
    targets.extend(anthropic_models());
    targets.extend(groq_models());
    targets.extend(xai_models());
    targets.extend(openrouter_models());
    targets.extend(moonshot_models());
    targets.extend(zhipu_models());
    targets.extend(harness::available());
    targets
}

/// The selection list `xact-cli` forces the moment `@ TELL` is entered:
/// installed Ollama models plus every key-provisioned provider's models —
/// the same real sources as [`known_targets`] minus the CLI harnesses
/// (`claude`, `kimi`, ...), which are coding agents rather than models
/// and so don't belong in a *model* picker. Sorted (case-insensitive,
/// then case-sensitive as a tiebreak) and deduped so the list a user is
/// forced to select from is byte-for-byte deterministic for a given set
/// of installed models and keys, no matter what order `ollama list` or a
/// provider's API happened to answer in.
pub fn selectable_models() -> Vec<String> {
    let mut models = ollama_models();
    models.extend(openai_models());
    models.extend(anthropic_models());
    models.extend(groq_models());
    models.extend(xai_models());
    models.extend(openrouter_models());
    models.extend(moonshot_models());
    models.extend(zhipu_models());
    models.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    models.dedup();
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_reuses_the_first_fetch_within_the_ttl() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CALLS: AtomicU32 = AtomicU32::new(0);

        let first = cached("test-key-reuse", || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            vec!["a".to_string()]
        });
        let second = cached("test-key-reuse", || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            vec!["b".to_string()]
        });

        assert_eq!(first, vec!["a".to_string()]);
        assert_eq!(second, vec!["a".to_string()], "second call within the TTL must reuse the cached value");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "fetch must only run once while the cache is fresh");
    }

    #[test]
    fn selectable_models_is_sorted_deduped_and_deterministic() {
        let first = selectable_models();
        let second = selectable_models();
        assert_eq!(first, second, "the forced-selection list must be identical across calls");

        let mut sorted = first.clone();
        sorted.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
        assert_eq!(first, sorted, "selectable_models must already be in its deterministic order");

        let mut deduped = first.clone();
        deduped.dedup();
        assert_eq!(first, deduped, "the same model reachable via two sources must appear once");
    }

    /// Whatever `ollama list` really returns on this machine is by
    /// definition installed and therefore selectable — real, not asserted
    /// against a fabricated snapshot. Skipped only where no local models
    /// exist at all, so the test stays meaningful on a fresh machine.
    #[test]
    fn selectable_models_contains_every_installed_ollama_model() {
        let installed = ollama_models();
        let selectable = selectable_models();
        for model in installed {
            assert!(selectable.contains(&model), "'{model}' is installed, so the forced model list must offer it");
        }
    }

    #[test]
    fn selectable_models_omits_harnesses_and_stays_within_known_targets() {
        let selectable = selectable_models();
        let known = known_targets();
        for model in &selectable {
            assert!(known.contains(model), "'{model}' must still be a real dispatchable TELL target");
            assert!(
                !harness::available().contains(model),
                "'{model}' looks like a harness name; the model picker must not offer agents"
            );
        }
    }
}
