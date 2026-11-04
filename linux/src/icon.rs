//! The tray icon, drawn in the channel's color so the development build never looks like the
//! production one.

use zib_core::Channel;

/// An ARGB32 image in network byte order, as the `StatusNotifierItem` interface takes it: a rounded
/// square in the channel's color with a white "Z", and for the development build, a white bar
/// across the bottom.
// Pixel arithmetic on small sizes, where floats and narrowing casts are exact enough.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]
pub fn pixmap(channel: Channel, size: usize) -> Vec<u8> {
    let accent = channel.accent();
    let [r, g, b] = [(accent >> 16) as u8, (accent >> 8) as u8, accent as u8];
    let s = size as f32;
    let radius = s * 0.22;
    let stroke = (s * 0.12).max(1.0);
    let (left, right, top, bottom) = (s * 0.27, s * 0.73, s * 0.25, s * 0.68);
    let mut data = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = rounded(px, py, s, radius);
            if !inside {
                data.extend([0, 0, 0, 0]);
                continue;
            }
            let on_top = (py - top).abs() < stroke / 2.0 && px > left && px < right;
            let on_bottom = (py - bottom).abs() < stroke / 2.0 && px > left && px < right;
            let on_diagonal = py > top && py < bottom && {
                let t = (py - top) / (bottom - top);
                let x_on = right - t * (right - left);
                (px - x_on).abs() < stroke * 0.7
            };
            let on_bar = channel == Channel::Development
                && py > s * 0.8
                && py < s * 0.9
                && px > s * 0.15
                && px < s * 0.85;
            if on_top || on_bottom || on_diagonal || on_bar {
                data.extend([255, 255, 255, 255]);
            } else {
                data.extend([255, r, g, b]);
            }
        }
    }
    data
}

fn rounded(x: f32, y: f32, size: f32, radius: f32) -> bool {
    let cx = x.clamp(radius, size - radius);
    let cy = y.clamp(radius, size - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_draw_different_icons() {
        let production = pixmap(Channel::Production, 32);
        let development = pixmap(Channel::Development, 32);
        assert_eq!(production.len(), 32 * 32 * 4);
        assert_ne!(production, development);
        // The corners are transparent.
        assert_eq!(&production[..4], &[0, 0, 0, 0]);
    }
}
