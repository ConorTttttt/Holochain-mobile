//! Restricted gameplay DTO. No identity material or conductor operations.
use anyhow::{ensure, Context, Result};
use holochain_moq_kart_wire::{KartState, KART_STATE_LEN};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GameFrame {
    pub flags: u8,
    pub tick: u32,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub heading: f32,
    pub drift: u8,
    pub item: u8,
    pub item_count: u8,
    pub lap: u8,
    pub track_t: f32,
}

impl GameFrame {
    pub fn parse(raw: &str) -> Result<Self> {
        ensure!(raw.len() <= 2048, "game frame exceeds bridge limit");
        serde_json::from_str(raw).context("invalid gameplay frame")
    }
    pub fn encode(self) -> Result<[u8; KART_STATE_LEN]> {
        KartState {
            flags: self.flags,
            tick: self.tick,
            pos: self.pos,
            vel: self.vel,
            heading: self.heading,
            drift: self.drift,
            item: self.item,
            item_count: self.item_count,
            lap: self.lap,
            track_t: self.track_t,
        }
        .encode()
        .context("original WP3 encoder rejected game state")
    }
}

pub fn state_json(state: &KartState) -> Value {
    json!({"flags": state.flags, "tick": state.tick, "pos": state.pos, "vel": state.vel,
        "heading": state.heading, "drift": state.drift, "item": state.item,
        "itemCount": state.item_count, "lap": state.lap, "trackT": state.track_t})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> Value {
        json!({"flags":0,"tick":3,"pos":[1.,2.,3.],"vel":[4.,0.,5.],"heading":0.2,
            "drift":0,"item":0,"itemCount":0,"lap":1,"trackT":0.25})
    }
    #[test]
    fn actual_state_round_trips_original_42_byte_codec() {
        let raw = frame();
        let bytes = GameFrame::parse(&raw.to_string())
            .unwrap()
            .encode()
            .unwrap();
        assert_eq!(bytes.len(), 42);
        assert_eq!(
            state_json(&KartState::decode(&bytes).unwrap())["pos"],
            raw["pos"]
        );
    }
    #[test]
    fn rejects_commands_wrong_shapes_and_bad_values() {
        let mut raw = frame();
        raw["conductor"] = json!("anything");
        assert!(GameFrame::parse(&raw.to_string()).is_err());
        raw = frame();
        raw["pos"] = json!([0, 1]);
        assert!(GameFrame::parse(&raw.to_string()).is_err());
        raw = frame();
        raw["trackT"] = json!(1.5);
        assert!(GameFrame::parse(&raw.to_string())
            .unwrap()
            .encode()
            .is_err());
        assert!(GameFrame::parse(&"x".repeat(2049)).is_err());
    }
}
