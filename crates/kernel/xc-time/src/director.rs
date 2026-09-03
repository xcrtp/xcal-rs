// director.rs — 时间推进控制
//
// Director 是唯一可推进时间的对象。advance / seek / set_speed / pause / resume
// 全部可序列化、可复制。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Director {
    pub global_t: f64,
    pub speed: f64,
    pub playing: bool,
}

impl Default for Director {
    fn default() -> Self {
        Self::new()
    }
}

impl Director {
    pub fn new() -> Self {
        Self {
            global_t: 0.0,
            speed: 1.0,
            playing: true,
        }
    }

    /// 时间推进（只在 playing 时累积）。
    pub fn advance(&mut self, dt: f64) {
        if self.playing {
            self.global_t += dt * self.speed;
        }
    }

    /// 直接置位（纯 seek，不累积当前状态）。
    pub fn seek(&mut self, t: f64) {
        self.global_t = t;
    }

    pub fn set_speed(&mut self, s: f64) {
        self.speed = s;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn resume(&mut self) {
        self.playing = true;
    }

    pub fn toggle(&mut self) {
        self.playing = !self.playing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_accumulates() {
        let mut d = Director::new();
        d.advance(1.0);
        d.advance(0.5);
        assert!((d.global_t - 1.5).abs() < 1e-9);
    }

    #[test]
    fn seek_overrides() {
        let mut d = Director::new();
        d.advance(5.0);
        d.seek(1.0);
        assert!((d.global_t - 1.0).abs() < 1e-9);
        d.advance(1.0);
        assert!((d.global_t - 2.0).abs() < 1e-9);
    }

    #[test]
    fn pause_stops() {
        let mut d = Director::new();
        d.pause();
        d.advance(100.0);
        assert!((d.global_t - 0.0).abs() < 1e-9);
        d.resume();
        d.advance(1.0);
        assert!((d.global_t - 1.0).abs() < 1e-9);
    }

    #[test]
    fn negative_speed_plays_backwards() {
        let mut d = Director::new();
        d.seek(10.0);
        d.set_speed(-1.0);
        d.advance(3.0);
        assert!((d.global_t - 7.0).abs() < 1e-9);
    }

    #[test]
    fn serde_roundtrip() {
        let d = Director {
            global_t: std::f64::consts::PI,
            speed: 2.5,
            playing: false,
        };
        let json = serde_json::to_string(&d).unwrap();
        let restored: Director = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.global_t, d.global_t);
        assert_eq!(restored.speed, d.speed);
        assert_eq!(restored.playing, d.playing);
    }
}
