pub fn to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (hue, if max == 0.0 { 0.0 } else { delta / max }, max)
}
pub fn color(h: f32, s: f32, v: f32) -> slint::Color {
    let h = h.rem_euclid(360.0);
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    slint::Color::from_rgb_u8(
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rgb_round_trips() {
        for (r, g, b) in [
            (0, 0, 0),
            (255, 255, 255),
            (128, 128, 128),
            (24, 26, 29),
            (255, 0, 0),
            (17, 148, 237),
        ] {
            let (h, s, v) = to_hsv(r, g, b);
            let c = color(h, s, v);
            assert_eq!((c.red(), c.green(), c.blue()), (r, g, b));
        }
    }
    #[test]
    fn clamps_and_wraps() {
        let c = color(720.0, 2.0, 2.0);
        assert_eq!((c.red(), c.green(), c.blue()), (255, 0, 0));
        let c = color(0.0, 1.0, -1.0);
        assert_eq!((c.red(), c.green(), c.blue()), (0, 0, 0));
    }
}
