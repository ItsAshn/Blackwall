//! The palette, mirroring the design system's `tokens.json` (void theme).
//! One place for every color: the scene converts these to linear light, the
//! HUD uses them as sRGB. Black is nothing; everything else stands for data.

pub const VOID: u32 = 0x000000;
pub const SIGNAL: u32 = 0xe8f6ff;
pub const SIGNAL_DIM: u32 = 0x5a80c4;
pub const SELECT: u32 = 0xffffff;
/// Cold steel: the healthy majority stays quiet so issues can shout.
pub const HEALTH_OK: u32 = 0x5f8fbf;
pub const HEALTH_WATCH: u32 = 0xb25cff;
pub const HEALTH_ISSUE: u32 = 0xff2440;
pub const KERNEL: u32 = 0x7a6cff;
pub const WALL_CALM: u32 = 0xff2bd6;
pub const WALL_HOT: u32 = 0xff1f3d;
pub const WALL_HEAD: u32 = 0xfff0fb;
/// An unlit slot: free RAM, the empty part of a meter.
pub const DOT_OFF: u32 = 0x0a1430;

pub const fn srgb8(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

/// sRGB hex to linear RGB, for emissive scene colors.
pub fn linear(hex: u32) -> [f32; 3] {
    srgb8(hex).map(|c| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    })
}

pub fn linear4(hex: u32, w: f32) -> bevy::math::Vec4 {
    let [r, g, b] = linear(hex);
    bevy::math::Vec4::new(r, g, b, w)
}
