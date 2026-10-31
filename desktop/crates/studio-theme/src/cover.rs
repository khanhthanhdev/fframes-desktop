//! The book covers of the start screen's project shelf, after Oleafly's `Book`: a soft cover
//! colour, a darker spine and bottom edge, and dark ink. The covers are the same in both
//! appearances, as Oleafly's are; a missing project gets a muted cover per appearance.

use gpui::{Rgba, rgb};

use crate::{Appearance, rgba_alpha};

/// The colours one book cover is painted with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BookCover {
    /// The front cover.
    pub face: Rgba,
    /// The spine strip at the left edge and the cover's thickness peeking below it.
    pub spine: Rgba,
    /// The project name on the cover.
    pub ink: Rgba,
    /// The small kind label above the name: the ink at [`KIND_INK`].
    pub ink_muted: Rgba,
    /// The cover's outline (Oleafly's `border-black/10`).
    pub outline: Rgba,
}

/// Oleafly's `BOOK_PALETTE`: ten soft colours, cream to spring green.
const FACES: [u32; 10] = [
    0xfbf8cc, 0xfde4cf, 0xffcfd2, 0xf1c0e8, 0xcfbaf0, 0xa3c4f3, 0x90dbf4, 0x8eecf5, 0x98f5e1,
    0xb9fbc0,
];

/// How many covers [`book_cover`] chooses from.
pub const COVER_COUNT: usize = FACES.len();

/// Oleafly's ink on a light cover (`#1f2937`).
const INK: u32 = 0x1f2937;

/// The kind label's ink opacity: 65 %, above Oleafly's 55 % (`opacity-55`), which falls
/// under 3:1 on the lilac cover.
const KIND_INK: f32 = 0.65;

/// Oleafly's `shade(color, -42)`: every channel 42 steps darker.
fn shade(color: Rgba) -> Rgba {
    let step = 42. / 255.;
    Rgba {
        r: (color.r - step).max(0.),
        g: (color.g - step).max(0.),
        b: (color.b - step).max(0.),
        a: color.a,
    }
}

fn cover(face: Rgba, ink: Rgba) -> BookCover {
    BookCover {
        face,
        spine: shade(face),
        ink,
        ink_muted: rgba_alpha(ink, KIND_INK),
        outline: rgba_alpha(rgb(0x000000), 0.1),
    }
}

/// Cover `index` of the palette (wrapping), for a project that can be opened.
pub fn book_cover(index: usize) -> BookCover {
    cover(rgb(FACES[index % COVER_COUNT]), rgb(INK))
}

/// The muted cover of a project whose folder is missing: a neutral grey that sits back on
/// the start screen's surface in `appearance`.
pub fn missing_cover(appearance: Appearance) -> BookCover {
    match appearance {
        Appearance::Light => cover(rgb(0xe5e5e5), rgb(0x404040)),
        Appearance::Dark => cover(rgb(0x333333), rgb(0xd4d4d4)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contrast::{composite, ratio};

    #[test]
    fn cover_ink_is_legible_on_every_cover() {
        let covers = (0..COVER_COUNT)
            .map(book_cover)
            .chain([Appearance::Light, Appearance::Dark].map(missing_cover));
        for cover in covers {
            assert!(ratio(cover.ink, cover.face) >= 4.5, "{cover:?}");
            let muted = composite(cover.ink_muted, cover.face);
            let muted = ratio(muted, cover.face);
            assert!(muted >= 3., "kind label {muted}: {cover:?}");
            let spine = ratio(cover.spine, cover.face);
            assert!(spine > 1.2, "spine {spine}: {cover:?}");
        }
    }

    #[test]
    fn the_palette_wraps() {
        assert_eq!(book_cover(COVER_COUNT + 2), book_cover(2));
        assert_ne!(book_cover(0), book_cover(1));
    }
}
