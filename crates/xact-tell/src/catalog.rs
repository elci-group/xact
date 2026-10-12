//! Every real name `@ TELL` could currently resolve — for live completion
//! (`xact-completion-graph` calls [`known_targets`]/[`selectable_models`])
//! and, through [`models_now`], for execution-time dispatch. `tell` itself
//! re-resolves independently at execution time and never trusts the
//! completion list as authorization to run something.
//!
//! Each network/subprocess-backed source is cached per-process with a TTL
//! (an `Instant` + fixed TTL, since a live API response has no mtime to
//! invalidate against). The two consumers read that cache differently:
//!
//! - **Completion never blocks.** [`known_targets`]/[`selectable_models`]
//!   return whatever is cached right now — possibly stale, possibly empty —
//!   and start a background refresh for any source that is missing or
//!   stale. A slow or unreachable provider can therefore never freeze the
//!   line editor (spec section 28; previously the first keystroke after
//!   `@ TELL` made synchronous HTTP calls with a 120 s timeout each).
//!   [`prewarm`] starts those refreshes at REPL startup so the first menu
//!   is usually already populated.
//! - **Dispatch blocks.** [`models_now`] fetches synchronously when the
//!   cache is cold or stale, because deciding which backend owns a name
//!   must use the real list, not a possibly-empty snapshot.
//!
//! Remote providers are skipped entirely — not contacted at all — while
//! [`set_remote_lookups_allowed`] is `false`, which `xact-core` sets
//! whenever the session's policy forbids `network` or `agent-remote`.
//! A source that's unavailable (no API key set, network down, binary
//! missing) contributes nothing; it never fails the catalog for the others.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::{anthropic, groq, harness, moonshot, ollama, openai, openrouter, xai, zhipu};

/// Long enough that opening the completion menu repeatedly while typing
/// one command never re-hits the network, short enough that a model
/// pulled or a key exported mid-session shows up without restarting
/// `xact`.
const TTL: Duration = Duration::from_secs(300);

pub(crate) type ModelsFn = fn() -> Vec<String>;

/// Every remote provider's catalog key and its real model-list fetch.
pub(crate) const REMOTE_SOURCES: &[(&str, ModelsFn)] = &[
    ("openai", openai::models),
    ("anthropic", anthropic::models),
    ("groq", groq::models),
    ("xai", xai::models),
    ("openrouter", openrouter::models),
    ("moonshot", moonshot::models),
    ("zhipu", zhipu::models),
];

const OLLAMA: (&str, ModelsFn) = ("ollama", ollama::installed_models_uncached);

/// When each source was last fetched, and what it returned.
type Cache = HashMap<&'static str, (Instant, Vec<String>)>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static IN_FLIGHT: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static REMOTE_LOOKUPS_ALLOWED: AtomicBool = AtomicBool::new(true);

/// Whether completion may contact remote providers at all. Set from the
/// session's policy (`! WITHOUT network`/`agent-remote` turn it off), so
/// a forbidden provider is not even asked for its model list.
pub fn set_remote_lookups_allowed(allowed: bool) {
    REMOTE_LOOKUPS_ALLOWED.store(allowed, Ordering::SeqCst);
}

fn remote_lookups_allowed() -> bool {
    REMOTE_LOOKUPS_ALLOWED.load(Ordering::SeqCst)
}

fn fresh(key: &str) -> Option<Vec<String>> {
    let cache = CACHE.lock().unwrap();
    cache.get(key).filter(|(at, _)| at.elapsed() < TTL).map(|(_, values)| values.clone())
}

fn store(key: &'static str, values: Vec<String>) {
    CACHE.lock().unwrap().insert(key, (Instant::now(), values));
}

/// `fetch`'s result for `key`, fetched synchronously if the cache is cold
/// or stale. For execution-time dispatch only.
pub(crate) fn models_now(key: &'static str, fetch: ModelsFn) -> Vec<String> {
    if let Some(values) = fresh(key) {
        return values;
    }
    let values = fetch();
    store(key, values.clone());
    values
}

/// Whatever is cached for `key` right now (stale or not), starting one
/// background refresh if it's missing or stale. Never blocks on `fetch`.
fn snapshot(key: &'static str, fetch: ModelsFn) -> Vec<String> {
    let (current, needs_refresh) = {
        let cache = CACHE.lock().unwrap();
        match cache.get(key) {
            Some((at, values)) => (values.clone(), at.elapsed() >= TTL),
            None => (Vec::new(), true),
        }
    };
    if needs_refresh && IN_FLIGHT.lock().unwrap().insert(key) {
        std::thread::spawn(move || {
            let values = fetch();
            store(key, values);
            IN_FLIGHT.lock().unwrap().remove(key);
        });
    }
    current
}

/// Starts background refreshes for every source completion reads, so the
/// first `@ TELL` menu is usually already populated.
pub fn prewarm() {
    let _ = completion_models();
}

/// Installed Ollama models plus, when allowed, every remote provider's
/// models — non-blocking snapshots.
fn completion_models() -> Vec<String> {
    let mut models = snapshot(OLLAMA.0, OLLAMA.1);
    if remote_lookups_allowed() {
        for (key, fetch) in REMOTE_SOURCES {
            models.extend(snapshot(key, *fetch));
        }
    }
    models
}

/// Installed Ollama models, fetched synchronously when the cache is cold.
#[cfg(test)]
fn ollama_models() -> Vec<String> {
    models_now(OLLAMA.0, OLLAMA.1)
}

/// Every real target name `TELL` could resolve right now: installed
/// Ollama models, models from any provider whose API key is set (when
/// remote lookups are allowed), and any registered harness found on
/// `$PATH`. Never blocks on a provider — see the module doc.
pub fn known_targets() -> Vec<String> {
    let mut targets = completion_models();
    targets.extend(harness::available());
    targets
}

/// The selection list `xact-cli` forces the moment `@ TELL` is entered:
/// the same real sources as [`known_targets`] minus the CLI harnesses
/// (`claude`, `kimi`, ...), which are coding agents rather than models
/// and so don't belong in a *model* picker. Sorted (case-insensitive,
/// then case-sensitive as a tiebreak) and deduped so the list is
/// deterministic for a given cache state, no matter what order `ollama
/// list` or a provider's API answered in.
pub fn selectable_models() -> Vec<String> {
    let mut models = completion_models();
    models.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    models.dedup();
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_now_reuses_the_first_fetch_within_the_ttl() {
        use std::sync::atomic::AtomicU32;
        static CALLS: AtomicU32 = AtomicU32::new(0);
        fn first_fetch() -> Vec<String> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            vec!["a".to_string()]
        }
        fn second_fetch() -> Vec<String> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            vec!["b".to_string()]
        }

        let first = models_now("test-key-reuse", first_fetch);
        let second = models_now("test-key-reuse", second_fetch);

        assert_eq!(first, vec!["a".to_string()]);
        assert_eq!(second, vec!["a".to_string()], "second call within the TTL must reuse the cached value");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "fetch must only run once while the cache is fresh");
    }

    /// The interactive-path guarantee: a provider that takes seconds to
    /// answer must not hold up completion. The first snapshot returns
    /// immediately (empty), and the value appears once the background
    /// refresh lands.
    #[test]
    fn snapshot_never_blocks_on_a_slow_source() {
        fn slow_fetch() -> Vec<String> {
            std::thread::sleep(Duration::from_millis(400));
            vec!["slow-model".to_string()]
        }
        let started = Instant::now();
        assert!(snapshot("test-key-slow", slow_fetch).is_empty());
        assert!(started.elapsed() < Duration::from_millis(100), "snapshot must not wait for the fetch");

        let deadline = Instant::now() + Duration::from_secs(5);
        while snapshot("test-key-slow", slow_fetch).is_empty() {
            assert!(Instant::now() < deadline, "background refresh never landed");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(snapshot("test-key-slow", slow_fetch), vec!["slow-model".to_string()]);
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
        // Blocking fetch first, so the non-blocking snapshot below is warm.
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
