//! Multi-agent orchestration for `@ TEAM` (spec sections 9 and 21).
//!
//! A team target is an explicit comma-separated roster, for example
//! `@ TEAM 'model-a, model-b' "Review this project."`. Every member gets
//! the same validated persona, context, thinking budget, and instruction.
//! Members run concurrently; their named responses are returned as one
//! deterministic report, and `POPULATING` writes that report once after all
//! members succeed. This avoids racing multiple agents onto one destination.
//!
//! Provider execution remains owned by `xact-tell`; resource constraints and
//! lifecycle scheduling are supplied by the Mesut adapter that invokes this
//! crate. `xact-team` owns only roster validation, fan-out/fan-in, and result
//! aggregation.

use std::fmt;
use std::path::PathBuf;
use std::thread;

use xact_ast::ResourceBudget;
use xact_tell::TellRequest;

#[derive(Debug)]
pub struct TeamError(String);

impl fmt::Display for TeamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TeamError {}

/// A fully resolved team request. `members` deliberately contains model
/// names rather than provider details: resolving a name is `xact-tell`'s
/// responsibility, exactly as it is for `@ TELL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamRequest {
    pub members: Vec<String>,
    pub persona: Option<String>,
    pub reading: Option<PathBuf>,
    pub populating: Option<PathBuf>,
    pub instruction: String,
    pub think: Option<u32>,
}

impl TeamRequest {
    /// Parses the grammar's one target string as a roster. Requiring at least
    /// two distinct entries ensures `TEAM` is not a misleading spelling of
    /// `TELL`; use `@ TELL` for a single provider.
    pub fn from_roster(
        roster: &str,
        persona: Option<String>,
        reading: Option<PathBuf>,
        populating: Option<PathBuf>,
        instruction: String,
        think: Option<u32>,
    ) -> Result<Self, TeamError> {
        let members: Vec<String> =
            roster.split(',').map(str::trim).filter(|member| !member.is_empty()).map(str::to_owned).collect();

        if members.len() < 2 {
            return Err(TeamError(
                "TEAM requires at least two comma-separated agent targets; use TELL for one agent.".into(),
            ));
        }
        let mut unique = members.clone();
        unique.sort();
        unique.dedup();
        if unique.len() != members.len() {
            return Err(TeamError("TEAM roster contains duplicate agent targets.".into()));
        }

        Ok(Self { members, persona, reading, populating, instruction, think })
    }

    fn member_request(&self, model: String) -> TellRequest {
        TellRequest {
            model,
            persona: self.persona.clone(),
            reading: self.reading.clone(),
            instruction: self.instruction.clone(),
            think: self.think,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamResponse {
    pub member: String,
    pub response: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamResult {
    pub responses: Vec<TeamResponse>,
    pub report: String,
    pub populated: Option<PathBuf>,
}

/// Executes all members concurrently and joins them in roster order. A
/// member failure fails the team and suppresses `POPULATING`; successful
/// member outputs are never presented as a fabricated complete team report.
pub fn execute(request: &TeamRequest, budget: &ResourceBudget) -> Result<TeamResult, TeamError> {
    execute_with(request, budget, |member, budget| {
        xact_tell::tell(&member, None, budget).map_err(|err| err.to_string())
    })
}

fn execute_with<F>(request: &TeamRequest, budget: &ResourceBudget, run: F) -> Result<TeamResult, TeamError>
where
    F: Fn(TellRequest, &ResourceBudget) -> Result<String, String> + Send + Sync + Copy + 'static,
{
    let mut workers = Vec::with_capacity(request.members.len());
    for member in &request.members {
        let member_name = member.clone();
        let member_request = request.member_request(member_name.clone());
        let budget = budget.clone();
        workers.push(thread::spawn(move || {
            let response = run(member_request, &budget);
            (member_name, response)
        }));
    }

    let mut responses = Vec::with_capacity(workers.len());
    for worker in workers {
        let (member, response) = worker.join().map_err(|_| TeamError("a TEAM member worker panicked.".into()))?;
        let response = response.map_err(|err| TeamError(format!("TEAM member '{member}' failed: {err}")))?;
        responses.push(TeamResponse { member, response });
    }

    let report = render_report(&responses);
    if let Some(destination) = &request.populating {
        xact_bank::establish(destination)
            .map_err(|err| TeamError(format!("failed to establish {}: {err}", destination.display())))?;
        std::fs::write(destination, &report)
            .map_err(|err| TeamError(format!("failed to write {}: {err}", destination.display())))?;
    }
    Ok(TeamResult { responses, report, populated: request.populating.clone() })
}

fn render_report(responses: &[TeamResponse]) -> String {
    responses
        .iter()
        .map(|response| format!("## {}\n\n{}", response.member, response.response))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(roster: &str) -> TeamRequest {
        TeamRequest::from_roster(roster, None, None, None, "Review this.".into(), None).unwrap()
    }

    #[test]
    fn roster_requires_two_distinct_members() {
        assert!(TeamRequest::from_roster("one", None, None, None, "go".into(), None).is_err());
        assert!(TeamRequest::from_roster("one, one", None, None, None, "go".into(), None).is_err());
    }

    #[test]
    fn members_run_and_aggregate_in_roster_order() {
        let result = execute_with(&request("first, second"), &ResourceBudget::default(), |member, _| {
            Ok(format!("response from {}", member.model))
        })
        .unwrap();
        assert_eq!(result.responses[0].member, "first");
        assert_eq!(result.responses[1].member, "second");
        assert_eq!(result.report, "## first\n\nresponse from first\n\n## second\n\nresponse from second");
    }

    #[test]
    fn a_member_failure_fails_the_team_without_a_report() {
        let result = execute_with(&request("first, second"), &ResourceBudget::default(), |member, _| {
            if member.model == "second" {
                Err("nope".into())
            } else {
                Ok("ok".into())
            }
        });
        assert!(result.unwrap_err().to_string().contains("second"));
    }
}
