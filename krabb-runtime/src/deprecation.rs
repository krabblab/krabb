//! Code that keeps an old interface working, each with the release that removes it.
//!
//! DEPRECATIONS.md holds the policy and the ledger of what is pending. The registry exists only
//! for its test, which fails once a bridge outlives its release.

/// One bridge: what it keeps working, and the release that removes it.
pub(crate) struct Bridge {
    pub what: &'static str,
    pub removed_in: &'static str,
}

/// Every bridge in the code; DEPRECATIONS.md lists the same ones.
pub(crate) const BRIDGES: &[Bridge] = &[];

mod tests {
    use super::*;

    #[test]
    fn every_bridge_is_listed_and_goes_in_its_release() {
        let ledger = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../DEPRECATIONS.md"),
        )
        .unwrap();

        for bridge in BRIDGES {
            assert!(
                crate::releases::valid_version(bridge.removed_in),
                "{} names an invalid removal release.",
                bridge.what
            );

            assert_eq!(
                crate::releases::compare(crate::VERSION, bridge.removed_in),
                std::cmp::Ordering::Less,
                "Remove the bridge for {}: it was due in {}.",
                bridge.what,
                bridge.removed_in
            );

            assert!(
                ledger.contains(&format!("| {} |", bridge.what)),
                "List the bridge for {} in DEPRECATIONS.md.",
                bridge.what
            );
        }
    }
}
