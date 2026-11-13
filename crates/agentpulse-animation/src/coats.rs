//! Coats and eye colours a user can pick for an agent's cat.
//!
//! A coat is a palette plus an optional pattern. Patterns recolour body, shade
//! and (for some) belly pixels per pixel, keyed to the sprite position and to
//! the body's bounding box in that frame, so they apply to every frame of every
//! state and the same frame always paints the same way. The coats and their
//! maths are a port of the prototype renderer that produced the coat catalogue.

use std::sync::LazyLock;

use agentpulse_core::Provider;

use crate::{DUST, Ink, PINK, Palette, Sprite};

pub const GROUPS: [&str; 7] = [
    "Classic coats",
    "Tabbies",
    "White markings",
    "Tortoiseshell and calico",
    "Colourpoints",
    "Spotted and ticked",
    "Fantasy",
];

#[derive(Debug, PartialEq)]
pub struct Coat {
    /// Stable key stored in settings.
    pub id: &'static str,
    pub name: &'static str,
    /// One of [`GROUPS`].
    pub group: &'static str,
    /// Colours before the pattern; the eye is the coat's own.
    pub palette: Palette,
    pattern: Pattern,
}

#[derive(Debug, PartialEq)]
pub struct Eye {
    pub id: &'static str,
    pub name: &'static str,
    pub color: [u8; 4],
}

#[derive(Debug, PartialEq)]
enum Pattern {
    None,
    /// White paws (bottom two rows).
    Paws,
    /// White lower body and paws.
    Bicolor,
    /// White except head and tail.
    Van,
    /// Body-colour patches on white, belly included.
    Cow,
    /// Generated tortoiseshell / calico patches; shade pixels darkened.
    Patches {
        cols: [[u8; 4]; 4],
        cell: f64,
        k: u32,
    },
    /// Body fading to the point colour at the extremities and head.
    Point([u8; 4]),
    /// Point with white paws.
    Birman([u8; 4]),
    Spots([u8; 4]),
    Rosette {
        spot: [u8; 4],
        rim: [u8; 4],
    },
    Ticked([u8; 4]),
    Stars {
        star: [u8; 4],
        nebula: [u8; 4],
    },
    Rainbow,
}

const fn rgb(v: u32) -> [u8; 4] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
}

#[allow(clippy::too_many_arguments)]
const fn coat(
    id: &'static str,
    name: &'static str,
    group: usize,
    body: u32,
    shade: u32,
    belly: u32,
    outline: u32,
    eye: u32,
    pattern: Pattern,
) -> Coat {
    Coat {
        id,
        name,
        group: GROUPS[group],
        palette: Palette {
            outline: rgb(outline),
            body: rgb(body),
            shade: rgb(shade),
            belly: rgb(belly),
            eye: rgb(eye),
            accent: PINK,
            dust: DUST,
        },
        pattern,
    }
}

impl Coat {
    const fn accent(mut self, v: u32) -> Self {
        self.palette.accent = rgb(v);
        self
    }

    /// See-through body, shade and belly.
    const fn alpha(mut self, a: u8) -> Self {
        self.palette.body[3] = a;
        self.palette.shade[3] = a;
        self.palette.belly[3] = a;
        self
    }
}

pub static COATS: &[Coat] = &[
    coat(
        "ginger-tabby",
        "Ginger tabby",
        0,
        0xe88c38,
        0xb0581c,
        0xfadeb0,
        0x964e1e,
        0x1e1814,
        Pattern::None,
    ),
    coat(
        "cream",
        "Cream",
        0,
        0xf2d3a2,
        0xd9b07a,
        0xfff3e0,
        0x9a7650,
        0x3d2b1a,
        Pattern::None,
    ),
    coat(
        "black",
        "Black",
        0,
        0x24232a,
        0x33323b,
        0x2e2d36,
        0x8a8a98,
        0xe8c440,
        Pattern::None,
    ),
    coat(
        "white",
        "White",
        0,
        0xf4f2ee,
        0xdcd8d0,
        0xffffff,
        0x8e8a84,
        0x4a90d9,
        Pattern::None,
    ),
    // Not in the prototype catalogue: the OpenCode cat as it was before coats.
    coat(
        "cloud",
        "Cloud",
        0,
        0xe2e4e8,
        0xb0b4be,
        0xffffff,
        0x6e7078,
        0x242830,
        Pattern::None,
    ),
    coat(
        "blue-british-shorthair",
        "Blue (British Shorthair)",
        0,
        0x8a94a6,
        0x737d90,
        0xa4adbd,
        0x4c5464,
        0xe89c38,
        Pattern::None,
    ),
    coat(
        "russian-blue",
        "Russian Blue",
        0,
        0x7b8aa0,
        0x6d7b92,
        0x93a0b4,
        0x424c5e,
        0x4ec26a,
        Pattern::None,
    ),
    coat(
        "chocolate",
        "Chocolate",
        0,
        0x6b4130,
        0x583425,
        0x835444,
        0x3a2118,
        0xe8a838,
        Pattern::None,
    ),
    coat(
        "lilac",
        "Lilac",
        0,
        0xc2b2b6,
        0xab9aa0,
        0xd8ccd0,
        0x786a70,
        0xe8a838,
        Pattern::None,
    ),
    coat(
        "cinnamon",
        "Cinnamon",
        0,
        0xa8623a,
        0x8e4f2c,
        0xc88a62,
        0x5c3018,
        0x3a9a5a,
        Pattern::None,
    ),
    coat(
        "smoke",
        "Smoke",
        0,
        0x3a3a44,
        0x5a5a68,
        0xb8bcc8,
        0x9a9cac,
        0xe8c440,
        Pattern::None,
    ),
    coat(
        "brown-tabby",
        "Brown tabby",
        1,
        0xa87c4c,
        0x5e3e22,
        0xe2cba4,
        0x4a2e18,
        0x7aa83a,
        Pattern::None,
    ),
    coat(
        "silver-tabby",
        "Silver tabby",
        1,
        0xc4c8cc,
        0x4a4e56,
        0xeceef0,
        0x3e4248,
        0x5ab04a,
        Pattern::None,
    ),
    coat(
        "blue-tabby",
        "Blue tabby",
        1,
        0x98a2b2,
        0x5c6678,
        0xc6ccd6,
        0x3e4656,
        0xe8a838,
        Pattern::None,
    ),
    coat(
        "cream-tabby",
        "Cream tabby",
        1,
        0xf0d4a8,
        0xc99a62,
        0xfff4e2,
        0x96704a,
        0xc87a2a,
        Pattern::None,
    ),
    coat(
        "red-tabby-bold-stripes",
        "Red tabby (bold stripes)",
        1,
        0xf09040,
        0x9a3c10,
        0xffe0b8,
        0x8a3a10,
        0x3a2a14,
        Pattern::None,
    ),
    coat(
        "tuxedo",
        "Tuxedo",
        2,
        0x22222a,
        0x2c2c36,
        0xf6f3ee,
        0x8a8a98,
        0x9ad04a,
        Pattern::Paws,
    ),
    coat(
        "ginger-and-white",
        "Ginger and white",
        2,
        0xe88c38,
        0xb0581c,
        0xf6f3ee,
        0x964e1e,
        0x1e1814,
        Pattern::Bicolor,
    ),
    coat(
        "grey-and-white",
        "Grey and white",
        2,
        0x7c8494,
        0x687080,
        0xf6f3ee,
        0x454c5a,
        0xe8b038,
        Pattern::Bicolor,
    ),
    coat(
        "cow-cat",
        "Cow cat",
        2,
        0x26252c,
        0x26252c,
        0xf6f3ee,
        0x6a6a76,
        0x9ad04a,
        Pattern::Cow,
    ),
    coat(
        "van",
        "Van",
        2,
        0xe88c38,
        0xb0581c,
        0xf6f3ee,
        0x9a7a64,
        0x4a90d9,
        Pattern::Van,
    ),
    coat(
        "mitted",
        "Mitted",
        2,
        0x7a6a5c,
        0x5e5044,
        0xf6f3ee,
        0x45382e,
        0x4a90d9,
        Pattern::Paws,
    ),
    coat(
        "tortoiseshell",
        "Tortoiseshell",
        3,
        0x2a2224,
        0x1e1818,
        0x3a2c26,
        0x0e0a0a,
        0xe8a838,
        Pattern::Patches {
            cols: [rgb(0x2a2224), rgb(0xc0662c), rgb(0x2a2224), rgb(0xe09048)],
            cell: 3.2,
            k: 2,
        },
    ),
    coat(
        "calico",
        "Calico",
        3,
        0xf6f3ee,
        0xdcd8d0,
        0xf6f3ee,
        0x6a5a50,
        0xc8a030,
        Pattern::Patches {
            cols: [rgb(0xf6f3ee), rgb(0xe08a3c), rgb(0xf6f3ee), rgb(0x2a2628)],
            cell: 4.0,
            k: 9,
        },
    ),
    coat(
        "dilute-calico",
        "Dilute calico",
        3,
        0xf6f3ee,
        0xdcd8d0,
        0xf6f3ee,
        0x7a7078,
        0xc8a030,
        Pattern::Patches {
            cols: [rgb(0xf6f3ee), rgb(0x9aa0b0), rgb(0xf6f3ee), rgb(0xf0d0a4)],
            cell: 4.0,
            k: 4,
        },
    ),
    coat(
        "torbie",
        "Torbie",
        3,
        0xa87c4c,
        0x5e3e22,
        0xe2cba4,
        0x3a2616,
        0x9ad04a,
        Pattern::Patches {
            cols: [rgb(0x8a6440), rgb(0xd07a34), rgb(0x5e3e22), rgb(0xa87c4c)],
            cell: 2.6,
            k: 13,
        },
    ),
    coat(
        "seal-point-siamese",
        "Seal point (Siamese)",
        4,
        0xefe4d2,
        0xe2d4bc,
        0xf8f0e4,
        0x5a4434,
        0x3a8ae0,
        Pattern::Point(rgb(0x4a3428)),
    ),
    coat(
        "blue-point",
        "Blue point",
        4,
        0xeceef2,
        0xdcdfe6,
        0xf8f9fb,
        0x5a6070,
        0x3a8ae0,
        Pattern::Point(rgb(0x6c7588)),
    ),
    coat(
        "chocolate-point",
        "Chocolate point",
        4,
        0xf2e8da,
        0xe6d8c4,
        0xfbf5ec,
        0x6a4a34,
        0x3a8ae0,
        Pattern::Point(rgb(0x8a5a3c)),
    ),
    coat(
        "lilac-point",
        "Lilac point",
        4,
        0xf2eef0,
        0xe4dee2,
        0xfbf9fa,
        0x867a82,
        0x3a8ae0,
        Pattern::Point(rgb(0xb0a2aa)),
    ),
    coat(
        "flame-point",
        "Flame point",
        4,
        0xf6ead8,
        0xecdcc4,
        0xfdf6ec,
        0x9a5a2c,
        0x3a8ae0,
        Pattern::Point(rgb(0xe0803a)),
    ),
    coat(
        "birman",
        "Birman",
        4,
        0xefe2cc,
        0xe2d2b6,
        0xf8efe0,
        0x5a4434,
        0x3a8ae0,
        Pattern::Birman(rgb(0x4a3428)),
    ),
    coat(
        "bengal",
        "Bengal",
        5,
        0xe0a85a,
        0xc48c40,
        0xf6e2bc,
        0x6a4420,
        0x5ab04a,
        Pattern::Rosette {
            spot: rgb(0x3a2414),
            rim: rgb(0x8a5a2a),
        },
    ),
    coat(
        "snow-bengal",
        "Snow Bengal",
        5,
        0xf0eadc,
        0xe0d6c2,
        0xfbf8f0,
        0x7a6a58,
        0x3a8ae0,
        Pattern::Rosette {
            spot: rgb(0x8a7a68),
            rim: rgb(0xc4b49e),
        },
    ),
    coat(
        "egyptian-mau",
        "Egyptian Mau",
        5,
        0xc8ccd0,
        0xaeb2b8,
        0xeceef0,
        0x4a4e56,
        0x8ac83a,
        Pattern::Spots(rgb(0x3a3e46)),
    ),
    coat(
        "ocicat",
        "Ocicat",
        5,
        0xd8b88a,
        0xbc9a6a,
        0xf2e2c8,
        0x5a3e22,
        0xc8a030,
        Pattern::Spots(rgb(0x5a3a1c)),
    ),
    coat(
        "abyssinian",
        "Abyssinian",
        5,
        0xc07a40,
        0xa86834,
        0xe8b888,
        0x6a3a1a,
        0xc8a030,
        Pattern::Ticked(rgb(0x8a4e22)),
    ),
    coat(
        "savannah",
        "Savannah",
        5,
        0xe6c46e,
        0xd0aa52,
        0xf8ecc4,
        0x5e4618,
        0xc8a030,
        Pattern::Rosette {
            spot: rgb(0x2a1e10),
            rim: rgb(0x8a6a2a),
        },
    ),
    coat(
        "midnight-neon",
        "Midnight neon",
        6,
        0x1a1830,
        0x2a2650,
        0x24204a,
        0x3ae0e0,
        0xff4fd8,
        Pattern::None,
    )
    .accent(0xff4fd8),
    coat(
        "galaxy",
        "Galaxy",
        6,
        0x2a1e5a,
        0x1e1640,
        0x3a2a74,
        0x9a86f0,
        0xf6e05e,
        Pattern::Stars {
            star: rgb(0xffffff),
            nebula: rgb(0xa04ac8),
        },
    ),
    coat(
        "ghost",
        "Ghost",
        6,
        0xeef2ff,
        0xd8e0f6,
        0xffffff,
        0xa8b4dc,
        0x5a6aa8,
        Pattern::None,
    )
    .alpha(150),
    coat(
        "pastel-pink",
        "Pastel pink",
        6,
        0xf6b8c8,
        0xe89aae,
        0xfde4ec,
        0xb06a80,
        0x4a3a5a,
        Pattern::None,
    ),
    coat(
        "mint",
        "Mint",
        6,
        0xa8e6c8,
        0x86d0ae,
        0xe0f8ec,
        0x4a9a78,
        0x2a4a3e,
        Pattern::None,
    ),
    coat(
        "gold",
        "Gold",
        6,
        0xe8b830,
        0xc08a18,
        0xf8e08a,
        0x7a5410,
        0x2a1e08,
        Pattern::None,
    ),
    coat(
        "chrome-robot",
        "Chrome robot",
        6,
        0xc8ced8,
        0x9aa2b0,
        0xe8ecf2,
        0x4a5262,
        0x3ae0ff,
        Pattern::None,
    )
    .accent(0x3ae0ff),
    coat(
        "rainbow",
        "Rainbow",
        6,
        0xe5484d,
        0xe5484d,
        0xfff6e8,
        0x4a3a5a,
        0x2a2030,
        Pattern::Rainbow,
    ),
    coat(
        "codex-night",
        "Codex night",
        6,
        0x343640,
        0x4e5260,
        0x5a5e6c,
        0x96a0b4,
        0x78e678,
        Pattern::None,
    ),
];

pub static EYES: [Eye; 7] = [
    Eye {
        id: "green",
        name: "Green",
        color: rgb(0x5ab04a),
    },
    Eye {
        id: "gold",
        name: "Gold",
        color: rgb(0xe8c440),
    },
    Eye {
        id: "amber",
        name: "Amber",
        color: rgb(0xe8962a),
    },
    Eye {
        id: "copper",
        name: "Copper",
        color: rgb(0xc0602a),
    },
    Eye {
        id: "blue",
        name: "Blue",
        color: rgb(0x3a8ae0),
    },
    Eye {
        id: "hazel",
        name: "Hazel",
        color: rgb(0x9a8a3a),
    },
    Eye {
        id: "dark",
        name: "Dark",
        color: rgb(0x1e1814),
    },
];

pub fn find_coat(id: &str) -> Option<&'static Coat> {
    COATS.iter().find(|c| c.id == id)
}

pub fn find_eye(id: &str) -> Option<&'static Eye> {
    EYES.iter().find(|e| e.id == id)
}

/// A coat and an eye colour; `eye: None` keeps the coat's own eyes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub coat: &'static Coat,
    pub eye: Option<&'static Eye>,
}

impl Look {
    /// Each agent's cat as it looked before coats existed.
    pub fn default_for(provider: Provider) -> Look {
        let id = match provider {
            Provider::ClaudeCode => "ginger-tabby",
            Provider::Codex => "codex-night",
            Provider::OpenCode => "cloud",
        };
        Look {
            coat: find_coat(id).expect("default coat exists"),
            eye: None,
        }
    }

    /// The flat colours: coat palette with the chosen eye.
    pub fn palette(&self) -> Palette {
        Palette {
            eye: self.eye.map_or(self.coat.palette.eye, |e| e.color),
            ..self.coat.palette
        }
    }

    /// Each pixel's RGBA (None for clear), row-major, pattern applied.
    pub fn paint(&self, sprite: &Sprite) -> Vec<Option<[u8; 4]>> {
        let pal = self.palette();
        let r = Region::of(sprite);
        let (w, h) = (sprite.width(), sprite.height());
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let ink = sprite.ink(x, y);
                let flat = pal.color(ink)?;
                Some(
                    r.as_ref()
                        .and_then(|r| {
                            self.coat
                                .pattern
                                .color(x as i64, y as i64, ink, r, pal.body)
                        })
                        .unwrap_or(flat),
                )
            })
            .collect()
    }
}

/// Bounding box and centre of the cat's pixels in one frame (dust excluded).
struct Region {
    x0: f64,
    y0: f64,
    y1: f64,
    w: f64,
    h: f64,
    cx: f64,
    cy: f64,
    far: f64,
}

impl Region {
    fn of(s: &Sprite) -> Option<Region> {
        let pts: Vec<(f64, f64)> = (0..s.height())
            .flat_map(|y| (0..s.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| !matches!(s.ink(x, y), Ink::Clear | Ink::Dust))
            .map(|(x, y)| (x as f64, y as f64))
            .collect();
        let n = pts.len() as f64;
        let min = |f: fn(&(f64, f64)) -> f64| pts.iter().map(f).fold(f64::INFINITY, f64::min);
        let max = |f: fn(&(f64, f64)) -> f64| pts.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
        let (x0, x1, y0, y1) = (min(|p| p.0), max(|p| p.0), min(|p| p.1), max(|p| p.1));
        let cx = pts.iter().map(|p| p.0).sum::<f64>() / n;
        let cy = pts.iter().map(|p| p.1).sum::<f64>() / n;
        let far = pts
            .iter()
            .map(|p| (p.0 - cx).hypot(p.1 - cy))
            .fold(f64::NEG_INFINITY, f64::max);
        (!pts.is_empty()).then_some(Region {
            x0,
            y0,
            y1,
            w: x1 - x0 + 1.0,
            h: y1 - y0 + 1.0,
            cx,
            cy,
            far,
        })
    }

    fn head(&self, x: f64, y: f64) -> bool {
        x >= self.x0 + 0.66 * self.w && y <= self.y0 + 0.62 * self.h
    }

    fn tail(&self, x: f64, y: f64) -> bool {
        x <= self.x0 + 0.2 * self.w && y <= self.y0 + 0.75 * self.h
    }

    fn paw(&self, y: f64) -> bool {
        y >= self.y1 - 1.0
    }
}

const WHITE: [u8; 4] = rgb(0xf6f3ee);
const BLACK: [u8; 4] = rgb(0x000000);
const BANDS: [[u8; 4]; 6] = [
    rgb(0xe5484d),
    rgb(0xf5a524),
    rgb(0xf2d22e),
    rgb(0x46b05f),
    rgb(0x3b82f6),
    rgb(0x8b5cf6),
];

impl Pattern {
    /// The patterned colour of one pixel, or None for the flat palette colour.
    fn color(&self, x: i64, y: i64, ink: Ink, r: &Region, body: [u8; 4]) -> Option<[u8; 4]> {
        let bs = matches!(ink, Ink::Body | Ink::Shade);
        let bsw = bs || ink == Ink::Belly;
        let (fx, fy) = (x as f64, y as f64);
        let point = |p| {
            let d = (fx - r.cx).hypot(fy - r.cy) / r.far;
            let mut t = ((d - 0.42) / 0.33).clamp(0.0, 1.0);
            if r.head(fx, fy) && ink != Ink::Belly {
                t = t.max(0.85);
            }
            mix(body, p, t)
        };
        match *self {
            Pattern::None => None,
            Pattern::Paws => (bs && r.paw(fy)).then_some(WHITE),
            Pattern::Bicolor => (bs && (fy >= r.y0 + 0.62 * r.h || r.paw(fy))).then_some(WHITE),
            Pattern::Van => (bs && !(r.head(fx, fy) || r.tail(fx, fy))).then_some(WHITE),
            Pattern::Cow => bsw.then(|| {
                if vnoise(x, y, 5.0, 7) > 0.58 {
                    body
                } else {
                    WHITE
                }
            }),
            Pattern::Patches { cols, cell, k } => bs.then(|| {
                let v = vnoise(x, y, cell, k);
                let n = cols.len() as f64;
                // `as` truncates toward zero and saturates at 0, like Python's int().
                let i = if v > 0.06 {
                    ((v * n * 1.15 - 0.07 * n) as usize).min(cols.len() - 1)
                } else {
                    0
                };
                if ink == Ink::Shade {
                    mix(cols[i], BLACK, 0.18)
                } else {
                    cols[i]
                }
            }),
            Pattern::Point(p) => bsw.then(|| point(p)),
            Pattern::Birman(_) if bs && r.paw(fy) => Some(WHITE),
            Pattern::Birman(p) => bsw.then(|| point(p)),
            Pattern::Spots(spot) => bs.then(|| {
                if (x * 3 + y * 5) % 7 == 0 || ((x + 2) * 5 + y * 3) % 11 == 0 {
                    spot
                } else {
                    body
                }
            }),
            Pattern::Rosette { spot, rim } => bs.then(|| match vnoise(x, y, 2.6, 3) {
                v if v > 0.72 => spot,
                v if v > 0.62 => rim,
                _ => body,
            }),
            Pattern::Ticked(tick) => bs.then(|| if (x + y) % 2 == 0 { tick } else { body }),
            Pattern::Stars { star, nebula } => bs.then(|| {
                if hash(x, y, 11) > 0.9 {
                    star
                } else {
                    mix(body, nebula, vnoise(x, y, 6.0, 5) * 0.8)
                }
            }),
            Pattern::Rainbow => {
                bs.then(|| BANDS[(((fx - r.x0) / r.w * 6.0) as usize).min(BANDS.len() - 1)])
            }
        }
    }
}

/// `a` toward `b` by `t`, keeping `a`'s alpha. Rounds half to even, like the prototype.
fn mix(a: [u8; 4], b: [u8; 4], t: f64) -> [u8; 4] {
    let c = |i: usize| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * t).round_ties_even() as u8;
    [c(0), c(1), c(2), a[3]]
}

/// Smooth value noise over a lattice of `cell` pixels.
fn vnoise(x: i64, y: i64, cell: f64, k: u32) -> f64 {
    let (gx, gy) = (x as f64 / cell, y as f64 / cell);
    let (x0, y0) = (gx as i64, gy as i64);
    let (fx, fy) = (gx - x0 as f64, gy - y0 as f64);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (a, b) = (hash(x0, y0, k), hash(x0 + 1, y0, k));
    let (c, d) = (hash(x0, y0 + 1, k), hash(x0 + 1, y0 + 1, k));
    (a * (1.0 - s(fx)) + b * s(fx)) * (1.0 - s(fy)) + (c * (1.0 - s(fx)) + d * s(fx)) * s(fy)
}

/// Lattice points the 32x16 sprites can reach, hashed once.
const HASH_W: i64 = 34;
const HASH_H: i64 = 18;
const HASH_K: u32 = 16;
static HASHES: LazyLock<Vec<f64>> = LazyLock::new(|| {
    (0..HASH_K)
        .flat_map(|k| (0..HASH_H).flat_map(move |y| (0..HASH_W).map(move |x| hash_md5(x, y, k))))
        .collect()
});

/// Deterministic hash of a pixel in [0, 1).
fn hash(x: i64, y: i64, k: u32) -> f64 {
    if (0..HASH_W).contains(&x) && (0..HASH_H).contains(&y) && k < HASH_K {
        HASHES[((k as i64 * HASH_H + y) * HASH_W + x) as usize]
    } else {
        hash_md5(x, y, k)
    }
}

/// The first 32 bits of MD5("x,y,k") over 2^32: the prototype's hash.
fn hash_md5(x: i64, y: i64, k: u32) -> f64 {
    let d = md5(format!("{x},{y},{k}").as_bytes());
    u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as f64 / 4_294_967_296.0
}

/// MD5 of a message shorter than 56 bytes (one block). Not for security.
fn md5(msg: &[u8]) -> [u8; 16] {
    const S: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    assert!(msg.len() < 56);
    let mut block = [0u8; 64];
    block[..msg.len()].copy_from_slice(msg);
    block[msg.len()] = 0x80;
    block[56..].copy_from_slice(&(msg.len() as u64 * 8).to_le_bytes());
    let m = |j: usize| u32::from_le_bytes(block[j * 4..j * 4 + 4].try_into().unwrap());
    let init = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    let [mut a, mut b, mut c, mut d] = init;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m(g));
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f.rotate_left(S[i / 16 * 4 + i % 4]));
    }
    let mut out = [0u8; 16];
    for (i, v) in [a, b, c, d].into_iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.wrapping_add(init[i]).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CatState, frames, palette, skid_frames, windup_frames};

    /// Every frame of every state and one-shot.
    fn all_frames() -> impl Iterator<Item = &'static Sprite> {
        [
            CatState::Sleeping,
            CatState::Sitting,
            CatState::LookingAround,
            CatState::Running,
            CatState::Sprinting,
            CatState::Happy,
            CatState::Surprised,
        ]
        .into_iter()
        .flat_map(frames)
        .chain(windup_frames())
        .chain(skid_frames())
    }

    #[test]
    fn hash_matches_the_prototype() {
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        // Values printed by the prototype's Python.
        assert_eq!(hash(0, 0, 0), 0.6190094100311399);
        assert_eq!(hash(12, 7, 13), 0.9003784360829741);
        assert_eq!(hash(12, 7, 13), hash_md5(12, 7, 13));
        assert_eq!(vnoise(13, 9, 2.6, 3), 0.3767400768742987);
    }

    #[test]
    fn mix_rounds_half_to_even() {
        // 0 + 5 * 0.5 = 2.5 rounds to 2, as Python's round() does.
        assert_eq!(mix([0, 1, 255, 9], [5, 4, 0, 0], 0.5), [2, 2, 128, 9]);
    }

    #[test]
    fn registry_is_well_formed() {
        assert_eq!(
            COATS.len(),
            47,
            "46 catalogue coats and the OpenCode default"
        );
        for (i, c) in COATS.iter().enumerate() {
            let slug: String = c
                .name
                .to_lowercase()
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("-");
            assert_eq!(c.id, slug);
            assert!(GROUPS.contains(&c.group), "{}", c.id);
            assert!(
                COATS[..i].iter().all(|o| o.id != c.id),
                "duplicate {}",
                c.id
            );
        }
        // Grouped in order, so a picker can list them group by group.
        let order: Vec<_> = COATS
            .iter()
            .map(|c| GROUPS.iter().position(|g| *g == c.group))
            .collect();
        assert!(order.is_sorted());
        assert_eq!(EYES.len(), 7);
        assert_eq!(find_eye("blue").map(|e| e.name), Some("Blue"));
        assert_eq!(find_coat("calico").map(|c| c.name), Some("Calico"));
        assert!(find_coat("nope").is_none() && find_eye("nope").is_none());
    }

    #[test]
    fn default_looks_paint_exactly_like_the_old_palettes() {
        for p in [Provider::ClaudeCode, Provider::Codex, Provider::OpenCode] {
            let (look, old) = (Look::default_for(p), palette(p));
            assert_eq!(look.palette(), old, "{p:?}");
            for s in all_frames() {
                let want: Vec<_> = (0..s.height())
                    .flat_map(|y| (0..s.width()).map(move |x| (x, y)))
                    .map(|(x, y)| old.color(s.ink(x, y)))
                    .collect();
                assert_eq!(look.paint(s), want, "{p:?}");
            }
        }
    }

    fn fnv(look: Look) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for s in all_frames() {
            for px in look.paint(s) {
                for b in px.unwrap_or([0; 4]) {
                    h = (h ^ b as u64).wrapping_mul(0x100000001b3);
                }
            }
        }
        h
    }

    #[test]
    fn patterns_match_the_prototype_on_every_frame() {
        // FNV-1a over every frame's RGBA as painted by the prototype's colour().
        let want = [
            ("tuxedo", 0x2591761c8796e901),
            ("ginger-and-white", 0x75898823f73ceff9),
            ("cow-cat", 0x8f1aa1c07db75cfb),
            ("van", 0xde551d27eb17fa43),
            ("calico", 0x209a430b44106743),
            ("torbie", 0xcb4353bfc283b8e3),
            ("seal-point-siamese", 0x6fd8aee088136b18),
            ("birman", 0xf7a3f25c769cfc3b),
            ("bengal", 0xb5f5c8fce9abc28e),
            ("egyptian-mau", 0x7c98bcfb59bb150),
            ("abyssinian", 0x8b250fc438942216),
            ("galaxy", 0x989f758494b1a1c5),
            ("ghost", 0x751c776227a96c46),
            ("rainbow", 0xb1c3bcf799dcb27f),
        ];
        for (id, sum) in want {
            let look = Look {
                coat: find_coat(id).unwrap(),
                eye: None,
            };
            assert_eq!(fnv(look), sum, "{id}");
        }
    }

    #[test]
    fn patterns_touch_only_coat_pixels_and_are_stable() {
        for coat in COATS {
            let look = Look { coat, eye: None };
            let pal = look.palette();
            for s in all_frames() {
                let px = look.paint(s);
                assert_eq!(px, look.paint(s), "{}: same frame, same pixels", coat.id);
                let mut changed = 0;
                for (i, c) in px.iter().enumerate() {
                    let ink = s.ink(i % s.width(), i / s.width());
                    match ink {
                        Ink::Body | Ink::Shade | Ink::Belly => {
                            changed += usize::from(Some(*c) != Some(pal.color(ink)));
                        }
                        _ => assert_eq!(*c, pal.color(ink), "{}", coat.id),
                    }
                }
                // Paws may be drawn as belly pixels (already white) or
                // outline only, so in some frames there is nothing to whiten.
                if !matches!(coat.pattern, Pattern::None | Pattern::Paws) {
                    assert!(changed > 0, "{}: pattern shows on every frame", coat.id);
                }
            }
        }
    }

    #[test]
    fn eye_choice_changes_only_the_eyes() {
        let coat = find_coat("calico").unwrap();
        let own = Look { coat, eye: None };
        let blue = Look {
            coat,
            eye: find_eye("blue"),
        };
        for s in all_frames() {
            for (i, (a, b)) in own.paint(s).into_iter().zip(blue.paint(s)).enumerate() {
                if s.ink(i % s.width(), i / s.width()) == Ink::Eye {
                    assert_eq!((a, b), (Some(coat.palette.eye), Some(rgb(0x3a8ae0))));
                } else {
                    assert_eq!(a, b);
                }
            }
        }
    }
}
