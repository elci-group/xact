//! `@ TELL <model>` over the real `ollama` binary — the original,
//! default backend: anything `dispatch` (in `lib.rs`) doesn't resolve to
//! a registered harness or a live provider's model list is assumed to
//! name a local Ollama model tag, passed straight through to `ollama run
//! <model>`.
//!
//! `THINK`'s numeric budget has no numeric equivalent in `ollama` — its
//! real `--think` flag only accepts `true`/`false`/`low`/`medium`/`high`.
//! Rather than inventing a fabricated number-to-level mapping, any
//! `THINK` value enables the real thinking-mode flag (`--think true`);
//! the stated number itself is already folded into the prompt text by
//! `build_prompt` (in `lib.rs`, shared across every backend), letting the
//! model interpret it in its own words.

use std::process::Command;

use xact_ast::ResourceBudget;

use crate::TellError;

/// Every model tag `ollama list` currently reports installed, or an
/// error if `ollama` itself can't be run — kept distinct from
/// [`installed_models_uncached`] because `dispatch`'s fast-fail check
/// needs to know *why* nothing came back (report it), while
/// `catalog::known_targets` just wants a plain list for completion and
/// treats any failure as "nothing to add here".
pub fn installed_models() -> Result<Vec<String>, TellError> {
    let output =
        Command::new("ollama").arg("list").output().map_err(|err| TellError(format!("failed to run ollama list: {err}")))?;
    if !output.status.success() {
        return Err(TellError(format!(
            "ollama list exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect())
}

/// [`installed_models`], with any failure (no `ollama` on this machine,
/// etc.) folded into an empty list — the shape `catalog::known_targets`
/// needs for its best-effort aggregation.
pub fn installed_models_uncached() -> Vec<String> {
    installed_models().unwrap_or_default()
}

/// Whether `requested` (what `TELL` named) refers to `installed` (one
/// row from [`installed_models`]) — either verbatim, or, when
/// `requested` carries no `:tag` of its own, against `installed`'s
/// implicit `:latest` default, exactly how `ollama run` itself resolves
/// a bare name. A `requested` that names a *different* explicit tag than
/// what's installed (`gemma3:7b` when only `gemma3:latest` exists) is
/// correctly not a match — that tag really isn't installed.
pub fn model_name_matches(requested: &str, installed: &str) -> bool {
    requested == installed || (!requested.contains(':') && installed == format!("{requested}:latest"))
}

/// Runs `prompt` through `model` via the real `ollama` binary,
/// constrained by `budget`. Fails fast — before spawning anything — for
/// a model that isn't actually installed, rather than letting `ollama
/// run` try to *pull* it and fail minutes later with a raw pull-manifest
/// error.
pub fn run(model: &str, prompt: &str, think: bool, budget: &ResourceBudget) -> Result<String, TellError> {
    let installed = installed_models()?;
    if !installed.iter().any(|candidate| model_name_matches(model, candidate)) {
        return Err(TellError(if installed.is_empty() {
            format!(
                "model '{model}' is not installed and no models are installed locally \
                 (try `ollama pull <model>` first)."
            )
        } else {
            format!("model '{model}' is not installed locally. Installed models: {}", installed.join(", "))
        }));
    }

    let mut command = Command::new("ollama");
    command.arg("run").arg(model).arg(prompt);
    if think {
        command.arg("--think").arg("true");
    }

    let _resource_guard = xact_resource::apply(&mut command, budget)
        .map_err(|err| TellError(format!("cannot honour the active resource policy: {err}")))?;

    command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let child = command.spawn().map_err(|err| TellError(format!("failed to run ollama: {err}")))?;
    let _cancel_guard = xact_cancel::register(child.id());
    let output = child.wait_with_output().map_err(|err| TellError(format!("failed to wait for ollama: {err}")))?;
    if !output.status.success() {
        return Err(TellError(format!(
            "ollama exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_name_matches_bare_name_against_its_latest_tag() {
        assert!(model_name_matches("gemma3", "gemma3:latest"));
        assert!(model_name_matches("gemma3:latest", "gemma3:latest"));
        assert!(!model_name_matches("gemma3:7b", "gemma3:latest"));
        assert!(!model_name_matches("gemma3", "gemma4:latest"));
    }
}
