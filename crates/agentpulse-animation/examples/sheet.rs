//! Writes `assets/cats/sprite-sheet.png`: one row per state (plus the wind-up and
//! skid one-shots) and palette, all frames side by side, on a half-light /
//! half-dark background. Also writes one animated PNG per motion sequence.

use std::{fs::File, io::BufWriter, path::Path};

use agentpulse_animation::{
    CatState, Sprite, frames, palette, rasterize, skid_frames, windup_frames,
};
use agentpulse_core::Provider;

const SCALE: usize = 8;
const GAP: usize = 8;
const LIGHT: [u8; 3] = [0xf2, 0xf2, 0xf2];
const DARK: [u8; 3] = [0x1e, 0x1e, 0x1e];
const MID: [u8; 3] = [0x80, 0x80, 0x80];

fn encoder(path: &Path, w: usize, h: usize) -> png::Encoder<'static, BufWriter<File>> {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut enc = png::Encoder::new(
        BufWriter::new(File::create(path).unwrap()),
        w as u32,
        h as u32,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc
}

fn main() {
    let rows_of: Vec<(&str, &[Sprite])> = vec![
        ("sleeping", frames(CatState::Sleeping)),
        ("sitting", frames(CatState::Sitting)),
        ("looking", frames(CatState::LookingAround)),
        ("running", frames(CatState::Running)),
        ("sprinting", frames(CatState::Sprinting)),
        ("happy", frames(CatState::Happy)),
        ("surprised", frames(CatState::Surprised)),
        ("windup", windup_frames()),
        ("skid", skid_frames()),
    ];
    let providers = [Provider::ClaudeCode, Provider::Codex, Provider::OpenCode];
    let first = &frames(CatState::Sitting)[0];
    let (cw, ch) = (first.width() * SCALE, first.height() * SCALE);
    let cols = rows_of.iter().map(|(_, f)| f.len()).max().unwrap();
    let rows = rows_of.len() * providers.len();
    let (w, h) = (GAP + cols * (cw + GAP), GAP + rows * (ch + GAP));

    let mut img = [MID[0], MID[1], MID[2], 255].repeat(w * h);
    let mut row = 0;
    for provider in providers {
        let pal = palette(provider);
        for (name, fs) in &rows_of {
            // Rows with speed lines and dust get an early split so those grey pixels
            // show on both halves.
            let split = if matches!(*name, "sprinting" | "windup" | "skid") {
                cw / 4
            } else {
                cw / 2
            };
            for (col, sprite) in fs.iter().enumerate() {
                let (ox, oy) = (GAP + col * (cw + GAP), GAP + row * (ch + GAP));
                let px = rasterize(sprite, &pal, SCALE);
                for y in 0..ch {
                    for x in 0..cw {
                        let bg = if x < split { LIGHT } else { DARK };
                        let src = &px[(y * cw + x) * 4..][..4];
                        let dst = ((oy + y) * w + ox + x) * 4;
                        let rgb = if src[3] == 0 { &bg[..] } else { &src[..3] };
                        img[dst..dst + 3].copy_from_slice(rgb);
                    }
                }
            }
            row += 1;
        }
    }

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/cats");
    let path = dir.join("sprite-sheet.png");
    encoder(&path, w, h)
        .write_header()
        .unwrap()
        .write_image_data(&img)
        .unwrap();
    println!("wrote {}", path.display());

    // Animated previews, orange palette, transparent background. Delays are the real
    // frame intervals (Running at mid activity).
    let pal = palette(Provider::ClaudeCode);
    for (name, delay_ms) in [
        ("running", 90u16),
        ("sprinting", 50),
        ("windup", 70),
        ("skid", 70),
    ] {
        let fs = rows_of.iter().find(|(n, _)| *n == name).unwrap().1;
        let path = dir.join(format!("anim-{name}.png"));
        let mut enc = encoder(&path, cw, ch);
        enc.set_animated(fs.len() as u32, 0).unwrap();
        let mut writer = enc.write_header().unwrap();
        for sprite in fs {
            writer.set_frame_delay(delay_ms, 1000).unwrap();
            writer
                .write_image_data(&rasterize(sprite, &pal, SCALE))
                .unwrap();
        }
        writer.finish().unwrap();
        println!("wrote {}", path.display());
    }
}
