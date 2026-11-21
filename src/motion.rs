pub const BAR_COUNT: usize = 32;

#[derive(Clone, Copy, Default, Debug)]
pub struct Bar {
    pub x: f32,
    pub height: f32,
    pub alpha: f32,
}

pub struct Frame {
    pub width: f32,
    pub height: f32,
    pub alpha: f32,
    pub offset_y: f32,
    pub mic_x: f32,
    pub mic_scale: f32,
    pub mic_alpha: f32,
    pub bars: [Bar; BAR_COUNT],
}

pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

// Keep the closing curve's position when the hotkey changes direction. The
// stream clock is frozen while contracting and resumes once it has expanded.
pub struct Playback {
    source_time: f32,
    changed_at: f32,
    phase: f32,
    closing: bool,
}

impl Playback {
    pub fn new() -> Self {
        Self {
            source_time: 0.0,
            changed_at: 0.0,
            phase: 1.0,
            closing: false,
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing
    }

    fn phase_at(&self, time: f32) -> f32 {
        let travel = (time - self.changed_at).max(0.0) / 0.5;
        (self.phase + if self.closing { -travel } else { travel }).clamp(0.0, 1.0)
    }

    pub fn sample(&self, time: f32) -> (f32, f32) {
        let source_time = if self.closing {
            self.source_time
        } else {
            self.source_time + (time - self.changed_at - (1.0 - self.phase) * 0.5).max(0.0)
        };
        (source_time, ease(self.phase_at(time)))
    }

    pub fn set_closing(&mut self, time: f32, closing: bool) {
        if self.closing == closing {
            return;
        }
        self.source_time = self.sample(time).0;
        self.phase = self.phase_at(time);
        self.changed_at = time;
        self.closing = closing;
    }
}

// Evaluate from elapsed time instead of integrating per frame. Missed frames never
// slow the animation down or change the relative timing of the icon and stream.
pub fn frame(time: f32, exit: f32) -> Frame {
    let exit = exit.clamp(0.0, 1.0);
    let source_mic_x = 112.0 - 68.0 * ease((time - 0.035) / 0.465);
    let mic_x = 112.0 + (source_mic_x - 112.0) * exit;
    let tail_alpha = ease(exit / 0.25);
    let mut result = Frame {
        width: 22.0 + 170.0 * ease(time / 0.5) * exit,
        height: 7.0 + 47.0 * ease(time / 0.34) * exit,
        alpha: ease(time / 0.16) * tail_alpha,
        offset_y: 0.0,
        mic_x,
        mic_scale: 0.72 + 0.28 * ease((time - 0.035) / 0.3) * exit,
        mic_alpha: ease((time - 0.035) / 0.19) * tail_alpha,
        bars: [Bar::default(); BAR_COUNT],
    };
    let last = ((time - 0.12) / 0.047).floor() as i64;
    let right = 132.0 + 59.0 * ease((time - 0.12) / 0.63);
    for (slot, i) in ((last - BAR_COUNT as i64 + 1)..=last).enumerate() {
        if i < 0 {
            continue;
        }
        let born = 0.12 + i as f32 * 0.047;
        let age = time - born;
        if age < 0.0 {
            continue;
        }
        let origin = 112.0 - 68.0 * ease((born - 0.035) / 0.465) + 18.0;
        let x = origin + age * 187.0;
        if x < source_mic_x + 20.0 || x > right + 7.0 {
            continue;
        }
        let envelope = ease(age / 0.095) * ((right + 7.0 - x) / 14.0).clamp(0.0, 1.0);
        let seed = 0.5 + 0.5 * (i as f32 * 2.39996 + 0.8).sin();
        let pulse = 0.78 + 0.22 * (time * 7.5 - i as f32 * 0.74).sin();
        result.bars[slot] = Bar {
            x: mic_x + (x - source_mic_x) * exit,
            height: (5.0 + 22.0 * seed * seed) * pulse * envelope * exit,
            alpha: 0.9 * envelope * ease(exit / 0.4),
        };
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_same_frame(a: Frame, b: Frame) {
        for (a, b) in [
            (a.width, b.width),
            (a.height, b.height),
            (a.alpha, b.alpha),
            (a.mic_x, b.mic_x),
            (a.mic_scale, b.mic_scale),
            (a.mic_alpha, b.mic_alpha),
        ] {
            assert!((a - b).abs() < 0.001, "frame jumped: {a} -> {b}");
        }
        for (a, b) in a.bars.iter().zip(&b.bars) {
            assert!((a.x - b.x).abs() < 0.001);
            assert!((a.height - b.height).abs() < 0.001);
            assert!((a.alpha - b.alpha).abs() < 0.001);
        }
    }

    #[test]
    fn pressing_again_reverses_without_resetting_any_visible_element() {
        for released_at in [0.08, 0.25, 0.6, 2.0] {
            for delay in [0.01, 0.15, 0.4] {
                let mut playback = Playback::new();
                playback.set_closing(released_at, true);
                let pressed_at = released_at + delay;
                let (t, exit) = playback.sample(pressed_at);
                let before = frame(t, exit);
                playback.set_closing(pressed_at, false);
                let (t, resumed_exit) = playback.sample(pressed_at);
                assert_same_frame(before, frame(t, resumed_exit));
                assert!(playback.sample(pressed_at + 0.01).1 > exit);
                let (t, exit) = playback.sample(pressed_at + delay + 0.1);
                assert!((t - (released_at + 0.1)).abs() < 0.001);
                assert_eq!(exit, 1.0);
            }
        }
    }

    #[test]
    fn rapid_direction_changes_and_duplicate_events_preserve_the_current_frame() {
        let mut playback = Playback::new();
        for (time, closing) in [
            (0.25, true),
            (0.32, false),
            (0.33, false),
            (0.35, true),
            (0.36, true),
            (0.38, false),
            (0.4, true),
        ] {
            let (t, exit) = playback.sample(time);
            let before = frame(t, exit);
            playback.set_closing(time, closing);
            let (t, exit) = playback.sample(time);
            assert_same_frame(before, frame(t, exit));
        }
        assert_eq!(playback.sample(0.9).1, 0.0);
        assert!(playback.is_closing());
    }

    #[test]
    fn uninterrupted_playback_keeps_original_entrance_and_half_second_exit() {
        let mut playback = Playback::new();
        assert_eq!(playback.sample(0.25), (0.25, 1.0));
        playback.set_closing(1.5, true);
        assert_eq!(playback.sample(1.75), (1.5, 0.5));
        assert_eq!(playback.sample(2.0), (1.5, 0.0));
    }
    #[test]
    fn closing_returns_icon_and_waves_to_center_without_an_initial_jump() {
        let open = frame(1.5, 1.0);
        let middle = frame(1.5, 0.5);
        let closed = frame(1.5, 0.0);
        assert_eq!(middle.width, 107.0);
        assert_eq!(middle.mic_x, 78.0);
        assert_eq!(middle.alpha, open.alpha);
        assert_eq!(closed.width, 22.0);
        assert_eq!(closed.height, 7.0);
        assert_eq!(closed.mic_x, 112.0);
        assert_eq!(closed.alpha, 0.0);
        for (source, shrinking) in open.bars.iter().zip(middle.bars.iter()) {
            if source.alpha > 0.0 {
                assert!((shrinking.x - middle.mic_x - (source.x - open.mic_x) * 0.5).abs() < 0.001);
                assert!((shrinking.height - source.height * 0.5).abs() < 0.001);
            }
        }
    }
    #[test]
    fn entrance_starts_small_and_settles_at_half_a_second() {
        let start = frame(0.0, 1.0);
        assert_eq!(start.alpha, 0.0);
        assert_eq!(start.width, 22.0);
        assert_eq!(start.mic_x, 112.0);
        let settled = frame(0.5, 1.0);
        assert!((settled.width - 192.0).abs() < 0.001);
        assert!((settled.mic_x - 44.0).abs() < 0.001);
        assert_eq!(settled.height, 54.0);
    }
    #[test]
    fn stream_continues_unfolding_after_shell_has_settled() {
        let extent = |t| {
            frame(t, 1.0)
                .bars
                .iter()
                .filter(|b| b.alpha > 0.1)
                .map(|b| b.x)
                .fold(0.0_f32, f32::max)
        };
        assert!(extent(0.75) > extent(0.5));
        assert_eq!(frame(0.75, 1.0).width, frame(0.5, 1.0).width);
    }
    #[test]
    fn hidden_exit_is_fully_transparent_and_frames_stay_bounded() {
        assert_eq!(frame(1.0, 0.0).alpha, 0.0);
        for n in 0..10_000 {
            let f = frame(n as f32 / 144.0, 1.0);
            assert!(f.width.is_finite());
            for b in f.bars {
                assert!(b.x.is_finite() && b.height.is_finite());
                assert!((0.0..=1.0).contains(&b.alpha));
                assert!(b.height >= 0.0 && b.height <= 27.0);
            }
        }
    }
}
