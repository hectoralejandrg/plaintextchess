//! Time controls for online rooms (spec "Server Time Control and Clock"):
//! a fixed set of presets, each a base time with a Fischer increment (the
//! increment is added when a move is completed).

/// A time control: base time plus Fischer increment, both in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeControl {
    pub base_ms: u64,
    pub increment_ms: u64,
}

impl TimeControl {
    /// The supported presets (spec): 15+10, 10+0, 5+0, 3+2, 1+0.
    pub const PRESETS: [TimeControl; 5] = [
        TimeControl {
            base_ms: 15 * 60_000,
            increment_ms: 10_000,
        },
        TimeControl {
            base_ms: 10 * 60_000,
            increment_ms: 0,
        },
        TimeControl {
            base_ms: 5 * 60_000,
            increment_ms: 0,
        },
        TimeControl {
            base_ms: 3 * 60_000,
            increment_ms: 2_000,
        },
        TimeControl {
            base_ms: 60_000,
            increment_ms: 0,
        },
    ];

    /// The control used when a create-room message carries no (or an
    /// unrecognized) time control: keeps older clients working (design D5).
    pub const DEFAULT: TimeControl = TimeControl {
        base_ms: 15 * 60_000,
        increment_ms: 10_000,
    };

    /// Parse the wire label (`m+i`: base minutes plus increment seconds).
    /// Only the supported presets parse; anything else is `None`.
    pub fn parse(label: &str) -> Option<Self> {
        let (base, increment) = label.split_once('+')?;
        let base_minutes: u64 = base.parse().ok()?;
        let increment_seconds: u64 = increment.parse().ok()?;
        Self::PRESETS.iter().copied().find(|tc| {
            tc.base_ms == base_minutes * 60_000 && tc.increment_ms == increment_seconds * 1_000
        })
    }

    /// The wire label of this control (e.g. `"15+10"`).
    pub fn label(self) -> String {
        format!("{}+{}", self.base_ms / 60_000, self.increment_ms / 1_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_round_trips_through_its_label() {
        for control in TimeControl::PRESETS {
            let label = control.label();
            assert_eq!(
                TimeControl::parse(&label),
                Some(control),
                "`{label}` should round-trip"
            );
        }
    }

    #[test]
    fn the_presets_are_the_five_supported_controls() {
        let labels: Vec<String> = TimeControl::PRESETS.iter().map(|tc| tc.label()).collect();
        assert_eq!(labels, vec!["15+10", "10+0", "5+0", "3+2", "1+0"]);
    }

    #[test]
    fn the_default_is_15_plus_10() {
        assert_eq!(TimeControl::DEFAULT.label(), "15+10");
        // A missing label parses to nothing, so callers fall back to DEFAULT.
        assert_eq!(TimeControl::parse(""), None);
    }

    #[test]
    fn unsupported_or_malformed_labels_do_not_parse() {
        for label in ["12+3", "15+11", "2+1", "abc", "15", "15+", "+10", "15+10+5"] {
            assert_eq!(
                TimeControl::parse(label),
                None,
                "`{label}` should not parse"
            );
        }
    }
}
