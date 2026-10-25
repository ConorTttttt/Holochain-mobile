//! Synthetic WP3 kart-state samples for the bounded 20 Hz MoQ diagnostic.
//! Uses the *original* desktop codec; it is deliberately not game physics.

use anyhow::{ensure, Context, Result};
use holochain_moq_kart_wire::{item, KartState, KART_STATE_LEN};

/// Five seconds at 20 Hz. No unbounded traffic from a diagnostic button.
pub const DIAGNOSTIC_HZ: u32 = 20;
pub const DIAGNOSTIC_SECONDS: u32 = 5;
pub const DIAGNOSTIC_FRAMES: u32 = DIAGNOSTIC_HZ * DIAGNOSTIC_SECONDS;
pub const DIAGNOSTIC_INTERVAL_MS: u64 = 1000 / DIAGNOSTIC_HZ as u64;

/// A deterministic slow 0.4 m/s movement for observing remote WP3 updates.
/// These samples MUST NOT be treated as authoritative simulation or scoring.
/// `tick` is monotonically advanced by the publisher for the entire race.
pub fn encode_sample(tick: u32) -> Result<[u8; KART_STATE_LEN]> {
    ensure!(tick > 0, "synthetic WP3 tick must be nonzero");
    let frame = (tick - 1) % DIAGNOSTIC_FRAMES;
    let state = KartState {
        flags: 0,
        tick,
        pos: [(frame as f32) * 0.02, 0.0, 0.0],
        vel: [0.4, 0.0, 0.0],
        heading: 0.0,
        drift: 0,
        item: item::NONE,
        item_count: 0,
        lap: 0,
        track_t: (frame as f32) / 10_000.0,
    };
    let bytes = state.encode().context("encode original desktop WP3 KartState")?;
    ensure!(bytes.len() == KART_STATE_LEN, "KartState wire size changed");
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_duration_and_20hz_cadence() {
        assert_eq!(DIAGNOSTIC_FRAMES, 100);
        assert_eq!(DIAGNOSTIC_INTERVAL_MS, 50);
    }

    #[test]
    fn desktop_codec_round_trip_and_monotonic_ticks() {
        let mut previous = None;
        for tick in 1..=DIAGNOSTIC_FRAMES {
            let bytes = encode_sample(tick).unwrap();
            assert_eq!(bytes.len(), 42);
            let state = KartState::decode(&bytes).unwrap();
            assert_eq!(state.tick, tick);
            assert_eq!(state.item, item::NONE);
            assert!((0.0..1.0).contains(&state.track_t));
            if let Some(prev) = previous {
                assert!(state.pos[0] > prev, "test position must advance");
            }
            previous = Some(state.pos[0]);
        }
        assert!(encode_sample(0).is_err());
    }
}
