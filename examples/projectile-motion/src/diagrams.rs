//! Analytic trajectories: the diagram and the numeric readouts use the same SI units.
use std::sync::LazyLock;

use fframes::{
    Color, Svgr,
    animation::{Easing, KeyFramesAnimation},
    svgr,
};
use fframes_kit::{props::SuggestedStop, ui::Ui};

use crate::{flight_time, position};

fn text<'a>(
    ui: &Ui<'a, '_, '_>,
    at: (f32, f32),
    size: usize,
    fill: Color,
    value: impl Into<String>,
) -> Svgr<'a> {
    let value = value.into();
    svgr!(<text x={at.0} y={at.1} font-family={ui.theme.typography.body.family}
        font-size={size} font-weight="500" fill={fill}>{value}</text>)
}

fn formula<'a>(ui: &Ui<'a, '_, '_>, at: (f32, f32), value: &str) -> Svgr<'a> {
    let value = value.to_owned();
    let fill = ui.theme.colors.title;
    svgr!(<text x={at.0} y={at.1} font-family="JetBrains Mono"
        font-size="36" font-weight="400" fill={fill}>{value}</text>)
}

fn heading<'a>(ui: &Ui<'a, '_, '_>, number: &str, title: &str) -> Svgr<'a> {
    let c = ui.theme.colors;
    svgr!(<g>
        {text(ui, (144.0, 105.0), 28, c.accent, format!("{number} / 06     THE SHAPE OF A THROW"))}
        {text(ui, (144.0, 220.0), 76, c.title, title)}
        <line x1="144" y1="268" x2="1776" y2="268" stroke={c.surface} stroke-width="2" />
    </g>)
}

/// An exact quadratic Bezier prefix, not a screen-space approximation of gravity.
fn arc(angle: f32, fraction: f32, origin: (f32, f32), scale: f32) -> String {
    let duration = flight_time(angle);
    let (range, _) = position(duration, angle);
    let (_, height) = position(duration * 0.5, angle);
    let (x, y) = position(duration * fraction, angle);
    format!(
        "M {} {} Q {} {} {} {}",
        origin.0,
        origin.1,
        origin.0 + range * fraction * scale * 0.5,
        origin.1 - 2.0 * height * fraction * scale,
        origin.0 + x * scale,
        origin.1 - y * scale
    )
}

fn axes<'a>(ui: &Ui<'a, '_, '_>, origin: (f32, f32), scale: f32) -> Svgr<'a> {
    let c = ui.theme.colors;
    let vertical = (0..=4).map(|i| {
        let x = origin.0 + i as f32 * 10.0 * scale;
        svgr!(<g>
            <line x1={x} y1={origin.1 - 17.0 * scale} x2={x} y2={origin.1} stroke={c.muted} opacity="0.14" />
            {text(ui, (x - 10.0, origin.1 + 45.0), 26, c.muted, (i * 10).to_string())}
        </g>)
    }).collect::<Vec<_>>();
    let horizontal = (1..=3).map(|i| {
        let y = origin.1 - i as f32 * 5.0 * scale;
        svgr!(<g>
            <line x1={origin.0} y1={y} x2={origin.0 + 44.0 * scale} y2={y} stroke={c.muted} opacity="0.14" />
            {text(ui, (origin.0 - 55.0, y + 9.0), 26, c.muted, (i * 5).to_string())}
        </g>)
    }).collect::<Vec<_>>();
    svgr!(<g>
        {vertical}{horizontal}
        <path d={format!("M {} {} V {} H {}", origin.0, origin.1 - 17.0 * scale, origin.1, origin.0 + 44.0 * scale)}
            fill="none" stroke={c.muted} stroke-width="2" />
        {text(ui, (origin.0, origin.1 - 17.0 * scale - 22.0), 28, c.muted, "height / m")}
        {text(ui, (origin.0 + 35.0 * scale, origin.1 + 90.0), 28, c.muted, "distance / m")}
    </g>)
}

pub fn launch<'a>(ui: &mut Ui<'a, '_, '_>) -> Svgr<'a> {
    static FLIGHT: LazyLock<KeyFramesAnimation<f32>> =
        LazyLock::new(|| fframes::timeline!(at 0.8 => 4.8, animate 0.0_f32 => 1.0, Easing::Linear));
    let c = ui.theme.colors;
    let angle = std::f32::consts::FRAC_PI_4;
    let p = ui.frame.animate(&*FLIGHT);
    let origin = (220.0, 820.0);
    let scale = 35.0;
    let (x, y) = position(flight_time(angle) * p, angle);
    ui.suggest_stops(|| vec![SuggestedStop {
        seconds: 4.8, label: Some("A throw traces a parabola".into()),
        notes: Some("Ask why the ball curves even though it keeps moving forward. Ignore drag and use a level landing surface.".into()),
    }]);
    svgr!(<g>
        {text(ui, (144.0, 116.0), 28, c.accent, "PHYSICS × MATH × RUST      /      01")}
        {text(ui, (144.0, 265.0), 112, c.title, "The shape of a throw")}
        {text(ui, (148.0, 345.0), 42, c.muted, "One moving ball. Two independent motions.")}
        <line x1="220" y1="820" x2="1730" y2="820" stroke={c.muted} stroke-width="2" />
        <path d={arc(angle, 1.0, origin, scale)} fill="none" stroke={c.accent} stroke-width="3" opacity="0.16" stroke-dasharray="8 14" />
        <path d={arc(angle, p, origin, scale)} fill="none" stroke={c.accent} stroke-width="6" stroke-linecap="round" />
        <circle cx={origin.0 + x * scale} cy={origin.1 - y * scale} r="14" fill={c.title} />
        <circle cx="220" cy="820" r="6" fill={c.accent} />
        {text(ui, (220.0, 884.0), 28, c.accent, "20 m/s · 45°")}
        {text(ui, (1250.0, 884.0), 28, c.muted, "Same launch & landing height")}
        {text(ui, (144.0, 1000.0), 28, c.muted, "An idealised model, not a ballistics simulator.")}
    </g>)
}

pub fn components<'a>(ui: &mut Ui<'a, '_, '_>) -> Svgr<'a> {
    static FLIGHT: LazyLock<KeyFramesAnimation<f32>> =
        LazyLock::new(|| fframes::timeline!(at 1.0 => 5.8, animate 0.0_f32 => 1.0, Easing::Linear));
    let c = ui.theme.colors;
    let angle = std::f32::consts::FRAC_PI_4;
    let p = ui.frame.animate(&*FLIGHT);
    let t = p * flight_time(angle);
    let (x, y) = position(t, angle);
    let origin = (220.0, 830.0);
    let scale = 21.0;
    let (sx, sy) = (origin.0 + x * scale, origin.1 - y * scale);
    ui.suggest_stops(|| vec![
        SuggestedStop { seconds: 3.4, label: Some("At the apex: vertical velocity is zero".into()), notes: Some("Only vy is zero. Horizontal velocity and the downward acceleration remain nonzero.".into()) },
        SuggestedStop { seconds: 5.8, label: Some("Same clock, two equations".into()), notes: Some("The ball returns to ground level after about 2.88 seconds and travels about 40.8 metres.".into()) },
    ]);
    svgr!(<g>
        {heading(ui, "03", "One throw. Two equations.")}
        {axes(ui, origin, scale)}
        <path d={arc(angle, 1.0, origin, scale)} fill="none" stroke={c.accent} stroke-width="3" opacity="0.2" />
        <path d={arc(angle, p, origin, scale)} fill="none" stroke={c.accent} stroke-width="5" />
        <path d={format!("M {sx} {} V {sy} H {}", origin.1, origin.0)} fill="none" stroke={c.highlight} stroke-width="2" stroke-dasharray="8 8" />
        <circle cx={sx} cy={sy} r="11" fill={c.title} />
        {text(ui, (1250.0, 400.0), 30, c.highlight, "HORIZONTAL · CONSTANT VELOCITY")}
        {formula(ui, (1250.0, 464.0), "x(t) = v₀ cos(θ) t")}
        {text(ui, (1250.0, 580.0), 30, c.accent, "VERTICAL · CONSTANT ACCELERATION")}
        {formula(ui, (1250.0, 644.0), "y(t) = v₀ sin(θ) t − ½gt²")}
        {text(ui, (1250.0, 790.0), 36, c.highlight, format!("t = {t:.2} s"))}
        {text(ui, (1250.0, 848.0), 36, c.text, format!("x = {x:.1} m     y = {:.1} m", y.max(0.0)))}
        {text(ui, (144.0, 1010.0), 30, c.muted, "Launch: 20 m/s at 45°     Gravity: 9.81 m/s²     ·     Flight slowed down for the diagram")}
    </g>)
}

pub fn range<'a>(ui: &mut Ui<'a, '_, '_>) -> Svgr<'a> {
    static DRAW: LazyLock<KeyFramesAnimation<f32>> = LazyLock::new(
        || fframes::timeline!(at 0.8 => 4.0, animate 0.0_f32 => 1.0, Easing::EaseInOut),
    );
    let c = ui.theme.colors;
    let p = ui.frame.animate(&*DRAW);
    let origin = (220.0, 855.0);
    let scale = 30.0;
    let curves =
        [(30.0_f32, c.muted), (60.0, c.highlight), (45.0, c.accent)].map(|(degrees, color)| {
            svgr!(<path d={arc(degrees.to_radians(), p, origin, scale)} fill="none" stroke={color}
            stroke-width={if degrees == 45.0 { 6 } else { 3 }} stroke-linecap="round" />)
        });
    ui.suggest_stops(|| vec![SuggestedStop {
        seconds: 4.0, label: Some("45° gives the longest ideal range".into()),
        notes: Some("30 and 60 degrees land together in distance, not in time. Equal launch and landing heights and no drag are essential to the 45-degree optimum.".into()),
    }]);
    svgr!(<g>
        {heading(ui, "04", "Why does 45° go farthest?")}
        {axes(ui, origin, scale)}
        {curves.into_iter().collect::<Vec<_>>()}
        {formula(ui, (1290.0, 385.0), "R = v₀² sin(2θ) / g")}
        {text(ui, (1510.0, 530.0), 32, c.muted, "30°   35.3 m")}
        {text(ui, (1510.0, 595.0), 32, c.accent, "45°   40.8 m")}
        {text(ui, (1510.0, 660.0), 32, c.highlight, "60°   35.3 m")}
        {text(ui, (144.0, 1010.0), 28, c.muted, "Equal launch / landing height · no drag · same speed · curves draw by fraction of each flight, not a shared clock")}
    </g>)
}
