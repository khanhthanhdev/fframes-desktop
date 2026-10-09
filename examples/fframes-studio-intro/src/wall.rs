//! A wall of squares, each one a frame, all rendering at once; when the last one
//! lands it resolves into the QR code of `short/qr.rs`. Shared by the 9:16 and the
//! 16:9 cuts, which differ in where they put it.

use fframes::Svgr;

use crate::beat::*;
use crate::short::qr::{MODULES, dark, qr};

const N: usize = 33;
/// Beats a square spends rendering.
const WORK: f32 = 0.55;

const DIM_C: [f32; 3] = [43.0, 41.0, 38.0];
const BONE_C: [f32; 3] = [236.0, 232.0, 225.0];
const ACCENT_C: [f32; 3] = [47.0, 128.0, 255.0];
const OFF_C: [f32; 3] = [20.0, 42.0, 82.0];
const QR_C: [f32; 3] = [33.0, 70.0, 183.0];
const WHITE_C: [f32; 3] = [255.0, 255.0, 255.0];
const SLATE_C: [f32; 3] = [15.0, 14.0, 13.0];

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> String {
    let t = clamp01(t);
    let c = |i: usize| lerp(a[i], b[i], t).round() as u8;
    format!("#{:02x}{:02x}{:02x}", c(0), c(1), c(2))
}

/// Beat a square starts rendering on: scattered, with a drift down the diagonal so
/// the wall still reads as filling in.
fn start_of(col: usize, row: usize) -> f32 {
    let idx = (row * N + col) as f32;
    let diag = (col + row) as f32 / (2 * N - 2) as f32;
    0.5 + 3.3 * (0.62 * hash(idx * 1.37) + 0.38 * diag)
}

/// The wall centered on (`cx`, `cy`), `size` pixels wide (the QR code itself, without
/// its tile), `tile_pad` the white margin of the tile. `lb` is the beat the squares
/// start on; `resolve` turns them into the QR code and `to_qr` swaps them for the
/// vector code. Returns the drawing and how many squares are done.
pub fn wall(
    lb: f32,
    (cx, cy): (f32, f32),
    size: f32,
    tile_pad: f32,
    resolve: f32,
    to_qr: f32,
) -> (Svgr<'static>, u32) {
    let k = size / MODULES;
    let x0 = cx - size / 2.0;
    let y0 = cy - size / 2.0;
    let half = size / 2.0 + tile_pad;

    let mut done = 0u32;
    let mut cells: Vec<Svgr> = Vec::with_capacity(N * N);
    if to_qr < 1.0 {
        for row in 0..N {
            for col in 0..N {
                let start = start_of(col, row);
                let p = prog(lb, start, start + WORK);
                if p >= 1.0 {
                    done += 1;
                }
                let on = dark(col, row);
                let settled = if on { ACCENT_C } else { OFF_C };
                // hot while it renders, then it cools into its place
                let (base, scale) = if p <= 0.0 {
                    (DIM_C, 0.55)
                } else if p < 1.0 {
                    (lerp3(ACCENT_C, BONE_C, p), 0.55 + 0.45 * p)
                } else {
                    let fresh = prog(lb, start + WORK, start + WORK + 0.6);
                    (lerp3(BONE_C, settled, fresh), 0.92)
                };
                // the resolve: dark modules turn the QR blue, the rest white
                let target = if on { QR_C } else { WHITE_C };
                let color = mix(base, target, resolve);
                let scale = scale + (1.0 - scale) * resolve;
                let w = k * scale;
                let inset = (k - w) / 2.0;
                cells.push(fframes::svgr!(
                    <rect x={x0 + col as f32 * k + inset} y={y0 + row as f32 * k + inset} width={w} height={w} fill={color} />
                ));
            }
        }
    } else {
        done = (N * N) as u32;
    }

    let tile_fill = mix(SLATE_C, WHITE_C, resolve);
    let drawing = fframes::svgr!(
        <g>
            <rect x={cx - half} y={cy - half} width={half * 2.0} height={half * 2.0} rx={26.0 * size / 500.0} fill={tile_fill} stroke="#3d3a35" stroke-width="2" stroke-opacity={1.0 - to_qr} />
            {cells}
            <g opacity={to_qr}>
                <g transform={format!("translate({x0} {y0}) scale({k})")}>{qr()}</g>
            </g>
        </g>
    );
    (drawing, done)
}
