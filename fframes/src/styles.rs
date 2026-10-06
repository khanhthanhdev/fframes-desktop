//! Resolved design tokens for generated videos (feature `styles`).
//!
//! [`Styles`] parses the flat, alias-resolved `style/tokens.json` runtime file from caller-provided
//! text or bytes. It performs no I/O: embed the file with `include_str!`, build [`Styles`] once
//! when the video is constructed, and read typed tokens from [`Video::render_frame`] through the
//! accessors, which only look up and borrow already-validated values.
//!
//! ```text
//! {"schema":1,"tokens":{
//!   "color.accent":{"type":"color","value":"#FF8800"},
//!   "spacing.md":{"type":"dimension","value":24.0}
//! }}
//! ```
//!
//! [`Video::render_frame`]: crate::Video::render_frame

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use crate::Color;
use crate::animation::Easing;

/// The only token file schema version this runtime understands.
pub const SUPPORTED_SCHEMA: u64 = 1;

/// Why a token file could not be read or a token could not be accessed.
#[derive(Debug, Clone, PartialEq)]
pub enum StylesError {
    /// The input is not valid UTF-8 JSON or does not have the `{schema, tokens}` shape.
    Parse(String),
    /// The `schema` field is not [`SUPPORTED_SCHEMA`].
    UnsupportedSchema(String),
    /// No token with this name exists.
    Missing { token: String },
    /// The token exists but has another type than the accessor asked for.
    WrongType {
        token: String,
        expected: &'static str,
        found: &'static str,
    },
    /// The token's `type` or `value` is malformed.
    Invalid { token: String, reason: String },
}

impl fmt::Display for StylesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(reason) => write!(f, "invalid style tokens: {reason}"),
            Self::UnsupportedSchema(found) => write!(
                f,
                "unsupported style token schema {found}, expected {SUPPORTED_SCHEMA}"
            ),
            Self::Missing { token } => write!(f, "style token `{token}` is missing"),
            Self::WrongType {
                token,
                expected,
                found,
            } => write!(
                f,
                "style token `{token}` is a {found} token, expected {expected}"
            ),
            Self::Invalid { token, reason } => {
                write!(f, "style token `{token}` is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for StylesError {}

/// A typography token.
#[derive(Debug, Clone, PartialEq)]
pub struct Typography {
    pub family: String,
    /// Font size in px of video coordinates.
    pub size: f32,
    pub weight: u16,
    /// Line height as a multiple of `size`.
    pub line_height: f32,
    /// Letter spacing in px of video coordinates.
    pub letter_spacing: f32,
}

/// A drop shadow token.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub color: Color,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Color(Color),
    Typography(Typography),
    Dimension(f32),
    Duration(f32),
    Easing(Easing),
    Shadow(Shadow),
}

impl Token {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Color(_) => "color",
            Self::Typography(_) => "typography",
            Self::Dimension(_) => "dimension",
            Self::Duration(_) => "duration",
            Self::Easing(_) => "easing",
            Self::Shadow(_) => "shadow",
        }
    }
}

/// Parsed, validated style tokens. Cheap to query, immutable after construction.
#[derive(Debug, Clone, PartialEq)]
pub struct Styles {
    tokens: BTreeMap<String, Token>,
}

macro_rules! accessor {
    ($(#[$doc:meta])* $name:ident, $variant:ident, $ty:ty, $expected:literal, |$v:ident| $get:expr) => {
        $(#[$doc])*
        pub fn $name(&self, token: &str) -> Result<$ty, StylesError> {
            match self.token(token)? {
                Token::$variant($v) => Ok($get),
                other => Err(wrong_type(token, $expected, other)),
            }
        }
    };
}

impl Styles {
    /// Parses token JSON text.
    pub fn from_json_str(json: &str) -> Result<Self, StylesError> {
        let root: Value =
            serde_json::from_str(json).map_err(|e| StylesError::Parse(e.to_string()))?;
        Self::from_value(&root)
    }

    /// Parses token JSON bytes (UTF-8).
    pub fn from_json_slice(json: &[u8]) -> Result<Self, StylesError> {
        let root: Value =
            serde_json::from_slice(json).map_err(|e| StylesError::Parse(e.to_string()))?;
        Self::from_value(&root)
    }

    fn from_value(root: &Value) -> Result<Self, StylesError> {
        let root = root
            .as_object()
            .ok_or_else(|| StylesError::Parse("root must be an object".into()))?;
        match root.get("schema") {
            Some(Value::Number(n)) if n.as_u64() == Some(SUPPORTED_SCHEMA) => {}
            Some(other) => return Err(StylesError::UnsupportedSchema(other.to_string())),
            None => return Err(StylesError::Parse("missing `schema`".into())),
        }
        let tokens = root
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(|| StylesError::Parse("`tokens` must be an object".into()))?;
        let tokens = tokens
            .iter()
            .map(|(name, value)| Ok((name.clone(), parse_token(name, value)?)))
            .collect::<Result<_, StylesError>>()?;
        Ok(Self { tokens })
    }

    /// Whether a token with this name exists, of any type.
    pub fn contains(&self, token: &str) -> bool {
        self.tokens.contains_key(token)
    }

    /// Token names in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tokens.keys().map(String::as_str)
    }

    fn token(&self, token: &str) -> Result<&Token, StylesError> {
        self.tokens.get(token).ok_or_else(|| StylesError::Missing {
            token: token.to_owned(),
        })
    }

    accessor!(
        /// A `color` token; `#RRGGBBAA` keeps its alpha.
        color, Color, Color, "color", |v| *v
    );
    accessor!(
        /// A `dimension` token (spacing, radius, stroke width) in px of video coordinates.
        dimension, Dimension, f32, "dimension", |v| *v
    );
    accessor!(
        /// A `duration` token in seconds.
        duration, Duration, f32, "duration", |v| *v
    );
    accessor!(
        /// An `easing` token.
        easing, Easing, Easing, "easing", |v| *v
    );
    accessor!(
        /// A `shadow` token.
        shadow, Shadow, Shadow, "shadow", |v| *v
    );

    /// A `typography` token.
    pub fn typography(&self, token: &str) -> Result<&Typography, StylesError> {
        match self.token(token)? {
            Token::Typography(value) => Ok(value),
            other => Err(wrong_type(token, "typography", other)),
        }
    }
}

fn wrong_type(token: &str, expected: &'static str, found: &Token) -> StylesError {
    StylesError::WrongType {
        token: token.to_owned(),
        expected,
        found: found.type_name(),
    }
}

fn parse_token(name: &str, value: &Value) -> Result<Token, StylesError> {
    let invalid = |reason: String| StylesError::Invalid {
        token: name.to_owned(),
        reason,
    };
    let object = value
        .as_object()
        .ok_or_else(|| invalid("token must be an object".into()))?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("missing string `type`".into()))?;
    let value = object
        .get("value")
        .ok_or_else(|| invalid("missing `value`".into()))?;
    let token = match kind {
        "color" => Token::Color(parse_color(value).map_err(invalid)?),
        "typography" => Token::Typography(parse_typography(value).map_err(invalid)?),
        "dimension" => Token::Dimension(number(value, "value").map_err(invalid)?),
        "duration" => {
            let seconds = number(value, "value").map_err(invalid)?;
            if seconds < 0.0 {
                return Err(invalid("duration must not be negative".into()));
            }
            Token::Duration(seconds)
        }
        "easing" => Token::Easing(parse_easing(value).map_err(invalid)?),
        "shadow" => Token::Shadow(parse_shadow(value).map_err(invalid)?),
        other => return Err(invalid(format!("unknown token type `{other}`"))),
    };
    Ok(token)
}

fn number(value: &Value, what: &str) -> Result<f32, String> {
    let n = value
        .as_f64()
        .ok_or_else(|| format!("`{what}` must be a number"))?;
    if n.is_finite() && n.abs() <= f64::from(f32::MAX) {
        Ok(n as f32)
    } else {
        Err(format!("`{what}` is out of range"))
    }
}

fn field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Value, String> {
    object.get(key).ok_or_else(|| format!("missing `{key}`"))
}

fn num_field(object: &Map<String, Value>, key: &str) -> Result<f32, String> {
    number(field(object, key)?, key)
}

fn as_object(value: &Value) -> Result<&Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| "value must be an object".to_owned())
}

fn parse_color(value: &Value) -> Result<Color, String> {
    let text = value
        .as_str()
        .ok_or_else(|| "color must be a `#RRGGBB` or `#RRGGBBAA` string".to_owned())?;
    let digits = text
        .strip_prefix('#')
        .filter(|d| (d.len() == 6 || d.len() == 8) && d.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| format!("`{text}` is not `#RRGGBB` or `#RRGGBBAA`"))?;
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).expect("hex digits checked");
    Ok(Color::rgba(
        byte(0),
        byte(2),
        byte(4),
        if digits.len() == 8 { byte(6) } else { 255 },
    ))
}

fn parse_typography(value: &Value) -> Result<Typography, String> {
    let object = as_object(value)?;
    let family = field(object, "family")?
        .as_str()
        .filter(|f| !f.is_empty())
        .ok_or("`family` must be a non-empty string")?
        .to_owned();
    let weight = field(object, "weight")?
        .as_u64()
        .and_then(|w| u16::try_from(w).ok())
        .filter(|w| (1..=1000).contains(w))
        .ok_or("`weight` must be an integer in 1..=1000")?;
    let size = num_field(object, "size")?;
    if size <= 0.0 {
        return Err("`size` must be positive".into());
    }
    Ok(Typography {
        family,
        size,
        weight,
        line_height: num_field(object, "line_height")?,
        letter_spacing: num_field(object, "letter_spacing")?,
    })
}

fn parse_easing(value: &Value) -> Result<Easing, String> {
    let object = as_object(value)?;
    let kind = field(object, "kind")?
        .as_str()
        .ok_or("`kind` must be a string")?;
    Ok(match kind {
        "linear" => Easing::Linear,
        "ease_in" => Easing::EaseIn,
        "ease_out" => Easing::EaseOut,
        "ease_in_out" => Easing::EaseInOut,
        "cubic_bezier" => {
            let (x1, y1) = (num_field(object, "x1")?, num_field(object, "y1")?);
            let (x2, y2) = (num_field(object, "x2")?, num_field(object, "y2")?);
            if !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) {
                return Err("cubic_bezier x1 and x2 must be within 0..=1".into());
            }
            Easing::CubicBezier(x1, y1, x2, y2)
        }
        "spring" => {
            let spring = Easing::Spring {
                mass: num_field(object, "mass")?,
                stiffness: num_field(object, "stiffness")?,
                damping: num_field(object, "damping")?,
            };
            if let Easing::Spring {
                mass,
                stiffness,
                damping,
            } = spring
                && (mass <= 0.0 || stiffness <= 0.0 || damping < 0.0)
            {
                return Err(
                    "spring mass and stiffness must be positive, damping non-negative".into(),
                );
            }
            spring
        }
        other => return Err(format!("unknown easing kind `{other}`")),
    })
}

fn parse_shadow(value: &Value) -> Result<Shadow, String> {
    let object = as_object(value)?;
    let blur = num_field(object, "blur")?;
    if blur < 0.0 {
        return Err("`blur` must not be negative".into());
    }
    Ok(Shadow {
        dx: num_field(object, "dx")?,
        dy: num_field(object, "dy")?,
        blur,
        color: parse_color(field(object, "color")?)?,
    })
}
