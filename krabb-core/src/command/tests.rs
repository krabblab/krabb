mod sh;
mod structure;
mod windows;

use super::{Dialect, Kind, classify};

pub(super) fn check(dialect: Dialect, cases: &[(&str, Kind, &str)]) {
    for (command, kind, reason) in cases {
        let verdict = classify(command, dialect);
        let reasons: Vec<String> = verdict.reasons.iter().map(ToString::to_string).collect();

        assert_eq!(verdict.kind, *kind, "{dialect:?} {command:?}: {reasons:?}");

        assert!(
            reasons.iter().any(|found| found == reason),
            "{dialect:?} {command:?}: {reasons:?} lacks {reason:?}"
        );
    }
}
