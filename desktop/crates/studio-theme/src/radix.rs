//! The raw Radix steps, once per appearance. `Theme` still copies them into its public
//! `gray_*`/`blue_*`/... fields until their callers move to roles; [`Colors`] and
//! [`Syntax`] are built from here so a role and the step it replaces cannot drift.
//!
//! [`Colors`]: crate::Colors
//! [`Syntax`]: crate::Syntax

use gpui::Rgba;

use crate::palette::hex;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Radix {
    pub gray_1: Rgba,
    pub gray_2: Rgba,
    pub gray_3: Rgba,
    pub gray_4: Rgba,
    pub gray_5: Rgba,
    pub gray_6: Rgba,
    pub gray_7: Rgba,
    pub gray_8: Rgba,
    pub gray_9: Rgba,
    pub gray_10: Rgba,
    pub gray_11: Rgba,
    pub gray_12: Rgba,

    pub blue_3: Rgba,
    pub blue_4: Rgba,
    pub blue_5: Rgba,
    pub blue_6: Rgba,
    pub blue_8: Rgba,
    pub blue_9: Rgba,
    pub blue_10: Rgba,
    pub blue_11: Rgba,
    pub blue_12: Rgba,

    pub red_2: Rgba,
    pub red_3: Rgba,
    pub red_4: Rgba,
    pub red_9: Rgba,
    pub red_10: Rgba,
    pub red_11: Rgba,

    pub amber_3: Rgba,
    pub amber_6: Rgba,
    pub amber_11: Rgba,

    /// Stock Radix green; Cap's palette carries no green.
    pub green_3: Rgba,
    pub green_11: Rgba,
}

pub(crate) const LIGHT: Radix = Radix {
    gray_1: hex(0xfcfcfc),
    gray_2: hex(0xf9f9f9),
    gray_3: hex(0xf0f0f0),
    gray_4: hex(0xe8e8e8),
    gray_5: hex(0xe0e0e0),
    gray_6: hex(0xd9d9d9),
    gray_7: hex(0xcecece),
    gray_8: hex(0xbbbbbb),
    gray_9: hex(0x8d8d8d),
    gray_10: hex(0x838383),
    gray_11: hex(0x646464),
    gray_12: hex(0x202020),

    blue_3: hex(0xe6f4fe),
    blue_4: hex(0xd5efff),
    blue_5: hex(0xc2e5ff),
    blue_6: hex(0xacd8fc),
    blue_8: hex(0x5eb1ef),
    blue_9: hex(0x0090ff),
    blue_10: hex(0x0588f0),
    blue_11: hex(0x0d74ce),
    blue_12: hex(0x113264),

    red_2: hex(0xfff7f7),
    red_3: hex(0xfeebec),
    red_4: hex(0xffdbdc),
    red_9: hex(0xe5484d),
    red_10: hex(0xdc3e42),
    red_11: hex(0xce2c31),

    amber_3: hex(0xfff7c2),
    amber_6: hex(0xf3d673),
    amber_11: hex(0xab6400),

    green_3: hex(0xe6f6eb),
    green_11: hex(0x218358),
};

/// Gray 1-6 and 11 are Cap's `theme.css` overrides, not stock Radix.
pub(crate) const DARK: Radix = Radix {
    gray_1: hex(0x111111),
    gray_2: hex(0x1c1c1c),
    gray_3: hex(0x282828),
    gray_4: hex(0x323232),
    gray_5: hex(0x3a3a3a),
    gray_6: hex(0x444444),
    gray_7: hex(0x484848),
    gray_8: hex(0x606060),
    gray_9: hex(0x6e6e6e),
    gray_10: hex(0x7b7b7b),
    gray_11: hex(0xa1a1a1),
    gray_12: hex(0xeeeeee),

    blue_3: hex(0x0d2847),
    blue_4: hex(0x003362),
    blue_5: hex(0x004074),
    blue_6: hex(0x104d87),
    blue_8: hex(0x2870bd),
    blue_9: hex(0x0090ff),
    blue_10: hex(0x3b9eff),
    blue_11: hex(0x70b8ff),
    blue_12: hex(0xc2e6ff),

    red_2: hex(0x201314),
    red_3: hex(0x3b1219),
    red_4: hex(0x500f1c),
    red_9: hex(0xe5484d),
    red_10: hex(0xec5d5e),
    red_11: hex(0xff9592),

    amber_3: hex(0x302008),
    amber_6: hex(0x5c3d05),
    amber_11: hex(0xffca16),

    green_3: hex(0x132d21),
    green_11: hex(0x3dd68c),
};
