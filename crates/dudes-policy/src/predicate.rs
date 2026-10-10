//! Gate 4: `Done` is observed, never declared (invariant 3).

use dudes_proto::LoopDecision;

use crate::input::PredicateInput;

/// `Some(Done)` iff the latest predicate result is `Observed` and passed. An `Inferred` pass —
/// an agent's self-report, a silence — is evidence, never a verdict, so it yields `None`.
#[must_use]
pub fn judge(result: PredicateInput) -> Option<LoopDecision> {
    (result.provenance.is_observed() && result.pass).then_some(LoopDecision::Done)
}

#[cfg(test)]
mod tests {
    use dudes_types::Provenance;

    use super::*;

    /// Invariant 3: of the four (pass, provenance) combinations only `Observed` + pass is `Done`.
    #[test]
    fn only_an_observed_pass_is_done() {
        for provenance in [Provenance::Inferred, Provenance::Observed] {
            for pass in [false, true] {
                let verdict = judge(PredicateInput {
                    pass,
                    provenance,
                    not_run: false,
                });
                let done = pass && provenance.is_observed();
                assert_eq!(
                    verdict,
                    done.then_some(LoopDecision::Done),
                    "pass={pass} provenance={provenance:?}"
                );
            }
        }
    }
}
