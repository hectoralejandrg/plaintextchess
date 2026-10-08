//! Fischer clock math (spec "Server Time Control and Clock", design D2):
//! pure millisecond arithmetic. The room actor feeds it monotonic marks
//! from its `TimeSource`, so the math is testable with fake values and the
//! real clock never drifts from the server's own measurements.

/// Both sides' remaining time in milliseconds. The side to move counts
/// down from its last settled value; the waiting side is frozen. Index 0 is
/// White, 1 is Black.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clocks {
    /// Last settled remaining time per side.
    remaining_ms: [u64; 2],
    /// The side whose clock is currently running.
    side_to_move: u8,
    /// Monotonic mark at which the side-to-move clock was last settled.
    turn_started_ms: u64,
    /// Whether the clock is counting. Set to `false` by [`Self::stop`] when the
    /// game reaches a terminal result, so every later snapshot reports the
    /// same frozen times.
    running: bool,
}

impl Clocks {
    /// A clock that just started: both sides at `base_ms`, `first_side` to
    /// move, settled at `started_ms`.
    pub fn new(base_ms: u64, first_side: u8, started_ms: u64) -> Self {
        Self {
            remaining_ms: [base_ms, base_ms],
            side_to_move: first_side,
            turn_started_ms: started_ms,
            running: true,
        }
    }

    /// The side to move (0 = White, 1 = Black).
    pub fn side_to_move(self) -> u8 {
        self.side_to_move
    }

    /// Remaining time of `side` at `now_ms`, clamped at zero: the side to
    /// move has been counting down since `turn_started_ms`; the waiting
    /// side is frozen at its settled value.
    pub fn remaining_ms(self, side: u8, now_ms: u64) -> u64 {
        if self.running && side == self.side_to_move {
            self.remaining_ms[side as usize]
                .saturating_sub(now_ms.saturating_sub(self.turn_started_ms))
        } else {
            self.remaining_ms[side as usize]
        }
    }

    /// Freeze the clock because the game just reached a terminal result: the
    /// side to move's elapsed time is settled one last time and no side counts
    /// down afterwards, so every snapshot after the ending reports the same
    /// remaining times regardless of when it is built.
    pub fn stop(&mut self, now_ms: u64) {
        if !self.running {
            return;
        }
        let side = self.side_to_move as usize;
        let elapsed = now_ms.saturating_sub(self.turn_started_ms);
        self.remaining_ms[side] = self.remaining_ms[side].saturating_sub(elapsed);
        self.running = false;
    }

    /// The monotonic mark at which the side to move's clock reaches zero.
    pub fn deadline_ms(self) -> u64 {
        self.turn_started_ms
            .saturating_add(self.remaining_ms[self.side_to_move as usize])
    }

    /// Whether the side to move's clock has already reached zero. A stopped
    /// clock (game over) is never flagged.
    pub fn is_flagged(self, now_ms: u64) -> bool {
        self.running && now_ms >= self.deadline_ms()
    }

    /// A move by `mover` completes at `now_ms` (design D2): the elapsed
    /// thinking time is settled off the mover's clock, the Fischer increment
    /// is added (so it is available on the mover's next move), and the side
    /// to move switches.
    pub fn settle_move(&mut self, mover: u8, increment_ms: u64, now_ms: u64) {
        debug_assert_eq!(mover, self.side_to_move);
        let elapsed = now_ms.saturating_sub(self.turn_started_ms);
        self.remaining_ms[mover as usize] = self.remaining_ms[mover as usize]
            .saturating_sub(elapsed)
            .saturating_add(increment_ms);
        self.side_to_move = 1 - mover;
        self.turn_started_ms = now_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 60_000;
    const INCR: u64 = 10_000;

    #[test]
    fn a_fresh_clock_starts_at_base_for_both_sides() {
        let clocks = Clocks::new(BASE, 0, 0);
        assert_eq!(clocks.side_to_move(), 0);
        assert_eq!(clocks.remaining_ms(0, 0), BASE);
        assert_eq!(clocks.remaining_ms(1, 0), BASE);
    }

    #[test]
    fn the_side_to_move_counts_down_and_the_waiting_side_is_frozen() {
        let clocks = Clocks::new(BASE, 0, 1_000);
        assert_eq!(clocks.remaining_ms(0, 6_000), BASE - 5_000);
        assert_eq!(clocks.remaining_ms(1, 6_000), BASE, "Black is not on the clock yet");
    }

    #[test]
    fn a_completed_move_settles_elapsed_time_and_adds_the_increment() {
        // White thinks 20 s on a 15+10-style control: 60 s − 20 s + 10 s.
        let mut clocks = Clocks::new(BASE, 0, 0);
        clocks.settle_move(0, INCR, 20_000);
        assert_eq!(clocks.side_to_move(), 1, "Black is now on the clock");
        assert_eq!(clocks.remaining_ms(0, 20_000), BASE - 20_000 + INCR);
        assert_eq!(clocks.remaining_ms(1, 20_000), BASE, "Black's clock starts at base");
        // Black's clock now runs from its own settle mark.
        assert_eq!(clocks.remaining_ms(1, 25_000), BASE - 5_000);
    }

    #[test]
    fn the_increment_becomes_available_on_the_movers_next_move() {
        let mut clocks = Clocks::new(BASE, 0, 0);
        clocks.settle_move(0, INCR, 1_000); // White's first move: +INCR
        clocks.settle_move(1, INCR, 2_000); // Black's reply
        // White's clock (now on move) reflects the increment earned earlier.
        assert_eq!(clocks.remaining_ms(0, 2_000), BASE - 1_000 + INCR);
    }

    #[test]
    fn the_flag_falls_when_the_deadline_is_reached() {
        let clocks = Clocks::new(10_000, 0, 0);
        let deadline = clocks.deadline_ms();
        assert_eq!(deadline, 10_000);
        assert!(!clocks.is_flagged(deadline - 1));
        assert!(clocks.is_flagged(deadline));
        assert!(clocks.is_flagged(deadline + 5_000));
    }

    #[test]
    fn zero_increment_hands_off_the_remaining_time() {
        // Blind bullet: no increment, so after a 9 s move White keeps 1 s
        // while the waiting side starts its own clock.
        let mut clocks = Clocks::new(10_000, 0, 0);
        clocks.settle_move(0, 0, 9_000);
        assert_eq!(clocks.remaining_ms(0, 9_000), 1_000);
        assert_eq!(
            clocks.remaining_ms(0, 20_000),
            1_000,
            "White is frozen while Black moves"
        );
        assert_eq!(clocks.deadline_ms(), 19_000, "Black now has its full base");
        assert!(!clocks.is_flagged(18_999));
        assert!(clocks.is_flagged(19_000));
    }

    #[test]
    fn remaining_time_never_goes_below_zero() {
        let clocks = Clocks::new(10_000, 0, 0);
        assert_eq!(clocks.remaining_ms(0, 10_000 + 250), 0);
        assert_eq!(clocks.remaining_ms(0, 1_000_000), 0);
    }

    #[test]
    fn stopping_the_clock_settles_once_and_then_freezes_both_sides() {
        let mut clocks = Clocks::new(BASE, 0, 0);
        // White has been thinking for 12 s when the game ends.
        clocks.stop(12_000);
        assert_eq!(clocks.remaining_ms(0, 12_000), BASE - 12_000);
        assert_eq!(
            clocks.remaining_ms(0, 60_000),
            BASE - 12_000,
            "a stopped clock does not keep counting down"
        );
        assert_eq!(clocks.remaining_ms(1, 60_000), BASE, "the waiting side stays put");
        assert!(!clocks.is_flagged(1_000_000), "a stopped clock never flags");
        // Stopping twice is a no-op (a later snapshot must not settle again).
        clocks.stop(90_000);
        assert_eq!(clocks.remaining_ms(0, 90_000), BASE - 12_000);
    }
}
