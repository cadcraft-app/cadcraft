//! MTEXT's options before the text (Height, Justify, Line spacing, Rotation, Style, Width,
//! Columns) for the prompt machine in `draw.rs`.

use cadcraft_doc::MText;
use cadcraft_geom::{Vec2, Vec3};

use super::curves;
use super::machines::number;
use crate::{Accept, EngineError, Input, Prompt, Result, Session};

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

/// The justification keywords, in attachment order 1..=9.
pub(super) const JUSTIFY: [&str; 9] = ["TL", "TC", "TR", "ML", "MC", "MR", "BL", "BC", "BR"];

/// Attachment code (1..=9) for a justification keyword.
pub(super) fn attach_code(k: &str) -> Option<u8> {
    JUSTIFY.iter().position(|j| j.eq_ignore_ascii_case(k.trim())).map(|i| i as u8 + 1)
}

/// What the options set before the text is typed.
pub(super) struct MTextOpts {
    pub height: f64,
    pub attach: u8,
    /// Line spacing factor (1 = 5/3 of the text height between baselines).
    pub spacing: f64,
    pub exact: bool,
    pub rotation: f64,
    pub style: String,
    /// The Width option replaces the opposite corner.
    pub width: Option<f64>,
}

impl Default for MTextOpts {
    fn default() -> Self {
        MTextOpts { height: 0.2, attach: 1, spacing: 1.0, exact: false, rotation: 0.0, style: "Standard".into(), width: None }
    }
}

impl MTextOpts {
    pub(super) fn new(s: &Session) -> Self {
        let (height, style) =
            s.doc().map(|d| (d.header.f64("TEXTSIZE", 0.2), d.header.str("TEXTSTYLE", "Standard"))).unwrap_or((0.2, "Standard".into()));
        MTextOpts { height, attach: 1, spacing: 1.0, exact: false, rotation: 0.0, style, width: None }
    }

    /// The text box from the first corner and either the opposite corner or the Width answer:
    /// the insertion point is the box's attachment point, the box turned by the rotation about
    /// the first corner.
    pub(super) fn mtext(&self, first: Vec2, second: Option<Vec2>, contents: String) -> MText {
        let (insert, width) = match (second, self.width) {
            (Some(b), _) => {
                // The opposite corner in the box's own (rotated) frame.
                let l = (b - first).rotate(-self.rotation);
                let (x0, x1) = (l.x.min(0.0), l.x.max(0.0));
                let (y0, y1) = (l.y.min(0.0), l.y.max(0.0));
                let i = usize::from(self.attach.clamp(1, 9) - 1);
                let x = [x0, (x0 + x1) / 2.0, x1][i % 3];
                let y = [y1, (y0 + y1) / 2.0, y0][i / 3];
                (first + Vec2::new(x, y).rotate(self.rotation), x1 - x0)
            }
            (None, w) => (first, w.unwrap_or(0.0)),
        };
        MText {
            insert: Vec3::new(insert.x, insert.y, 0.0),
            height: self.height,
            width,
            attach: self.attach.clamp(1, 9),
            rotation: self.rotation,
            style: self.style.clone(),
            contents,
            line_spacing: self.spacing,
            line_spacing_exact: self.exact,
        }
    }
}

/// A question an option asks before MTEXT goes back to "Specify opposite corner".
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MTextAsk {
    Height,
    Justify,
    SpacingType,
    SpacingFactor,
    Rotation,
    Style,
    Width,
    Columns,
}

pub(super) enum Outcome {
    /// Ask this next (the same question again after an invalid answer).
    Ask(MTextAsk),
    /// Back to the opposite-corner prompt.
    Back,
    /// The Width answer replaces the opposite corner: go on to the text.
    Text,
}

impl MTextAsk {
    pub(super) fn from_keyword(k: &str) -> Option<MTextAsk> {
        Some(match k {
            "Height" => MTextAsk::Height,
            "Justify" => MTextAsk::Justify,
            "Line spacing" => MTextAsk::SpacingType,
            "Rotation" => MTextAsk::Rotation,
            "Style" => MTextAsk::Style,
            "Width" => MTextAsk::Width,
            "Columns" => MTextAsk::Columns,
            _ => return None,
        })
    }

    pub(super) fn prompt(self, o: &MTextOpts, first: Vec2) -> Prompt {
        match self {
            MTextAsk::Height => Prompt::new("Specify height", Accept::POINT_OR_NUMBER).default(format!("{:.4}", o.height)).base(first),
            MTextAsk::Justify => Prompt::new("Enter justification", curves::KW)
                .kw(&JUSTIFY)
                .default(JUSTIFY.get(usize::from(o.attach.clamp(1, 9) - 1)).copied().unwrap_or("TL")),
            MTextAsk::SpacingType => {
                Prompt::new("Enter line spacing type", curves::KW).kw(&["At least", "Exactly"]).default(if o.exact { "Exactly" } else { "At least" })
            }
            MTextAsk::SpacingFactor => Prompt::new("Enter line spacing factor or distance", Accept::NUMBER).default(format!("{}x", trim(o.spacing))),
            MTextAsk::Rotation => Prompt::new("Specify rotation angle", Accept::POINT_OR_NUMBER).default(trim(o.rotation.to_degrees())).base(first),
            MTextAsk::Style => Prompt::new("Enter style name", curves::KW).kw(&["?"]).default(o.style.clone()),
            MTextAsk::Width => Prompt::new("Specify width", Accept::POINT_OR_NUMBER).base(first),
            MTextAsk::Columns => Prompt::new("Enter column type", curves::KW).kw(&["Dynamic", "Static", "No"]).default("No"),
        }
    }

    pub(super) fn input(self, s: &mut Session, i: Input, o: &mut MTextOpts, first: Vec2) -> Result<Outcome> {
        let word = |i: &Input| match i {
            Input::Keyword(k) | Input::Text(k) => Some(k.trim().to_string()),
            _ => None,
        };
        match (self, &i) {
            (_, Input::Enter) => Ok(if self == MTextAsk::SpacingType { Outcome::Ask(MTextAsk::SpacingFactor) } else { Outcome::Back }),
            (MTextAsk::Height, Input::Point(p)) => set_height(s, o, first.dist(*p)),
            (MTextAsk::Height, Input::Text(t)) => set_height(s, o, number(t).unwrap_or(f64::NAN)),
            (MTextAsk::Justify, _) => {
                let k = word(&i).unwrap_or_default();
                o.attach = attach_code(&k).ok_or_else(|| other("Invalid option keyword."))?;
                Ok(Outcome::Back)
            }
            (MTextAsk::SpacingType, _) => {
                let k = word(&i).unwrap_or_default().to_ascii_lowercase();
                o.exact = match k.as_str() {
                    "at least" | "a" | "at" => false,
                    "exactly" | "e" => true,
                    _ => return Err(other("Invalid option keyword.")),
                };
                Ok(Outcome::Ask(MTextAsk::SpacingFactor))
            }
            (MTextAsk::SpacingFactor, Input::Text(t)) => {
                // "1.5x" is a factor; a plain number is the distance between lines (1x = 5/3 of the height).
                let t = t.trim();
                let f = match t.strip_suffix(['x', 'X']) {
                    Some(f) => number(f),
                    None => number(t).map(|d| d / (o.height * 5.0 / 3.0)),
                };
                o.spacing = f
                    .filter(|f| (0.25 - 1e-9..=4.0 + 1e-9).contains(f))
                    .ok_or_else(|| other("Requires a factor between 0.25x and 4x, or a distance."))?;
                Ok(Outcome::Back)
            }
            (MTextAsk::Rotation, Input::Point(p)) => {
                o.rotation = first.angle_to(*p);
                Ok(Outcome::Back)
            }
            (MTextAsk::Rotation, Input::Text(t)) => {
                o.rotation = s.angle_settings().direction(t).filter(|a| a.is_finite()).ok_or_else(|| other("Requires an angle."))?;
                Ok(Outcome::Back)
            }
            (MTextAsk::Style, _) => {
                let name = word(&i).unwrap_or_default();
                let d = s.doc()?;
                if name == "?" {
                    let names: Vec<String> = d.text_styles.iter().map(|t| t.name.clone()).collect();
                    s.echo(format!("Text styles: {}", names.join(", ")));
                    return Ok(Outcome::Ask(MTextAsk::Style));
                }
                o.style = d.text_style(&name).map(|t| t.name.clone()).ok_or_else(|| other(format!("Cannot find text style \"{name}\".")))?;
                Ok(Outcome::Back)
            }
            (MTextAsk::Width, Input::Point(p)) => {
                // The distance along the text direction from the first corner.
                o.width = Some((*p - first).rotate(-o.rotation).x.abs());
                Ok(Outcome::Text)
            }
            (MTextAsk::Width, Input::Text(t)) => {
                o.width = Some(number(t).filter(|w| w.is_finite() && *w >= 0.0).ok_or_else(|| other("Requires a non-negative width."))?);
                Ok(Outcome::Text)
            }
            (MTextAsk::Columns, _) => match word(&i).unwrap_or_default().to_ascii_lowercase().as_str() {
                "no" | "n" => Ok(Outcome::Back),
                "dynamic" | "d" | "static" | "s" => {
                    s.echo("Text columns are not available yet; the text stays in one column.");
                    Ok(Outcome::Back)
                }
                _ => Err(other("Invalid option keyword.")),
            },
            _ => Ok(Outcome::Ask(self)),
        }
    }
}

fn set_height(s: &mut Session, o: &mut MTextOpts, h: f64) -> Result<Outcome> {
    if !h.is_finite() || h <= 0.0 {
        return Err(other("Requires a positive height."));
    }
    o.height = h;
    s.doc_mut()?.header.set_f64("TEXTSIZE", h);
    Ok(Outcome::Back)
}

/// A number without trailing zeros (`1.5`, `2`).
fn trim(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
