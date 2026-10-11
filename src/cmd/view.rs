//! View menu: ZOOM, PAN, REGEN, REDRAW, layout switching.

use cadcraft_doc::{Space, entity_bounds};
use cadcraft_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::machines::{SelectRun, number};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step, View};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("zoom", "Zoom", run_zoom)
            .menu(&["View", "Zoom", "Realtime"])
            .alias(&["z"])
            .params("{mode: extents|all|window|previous|in|out|center|scale|object, p1?, p2?, center?, height?, factor?, xp?: bool (scale relative to paper space: in a viewport, `factor` paper units per model unit), limits?: bool (scale relative to the limits)} (center: `height`, or `factor` with `xp`/`limits`)")
            .noundo()
            .transparent()
            .interactive(|_| Ok(Box::new(ZoomM::default()))),
        CommandSpec::new("zoom.previous", "Zoom Previous", |s, _| zoom(s, &json!({"mode":"previous"})))
            .menu(&["View", "Zoom", "Previous"])
            .noundo()
            .transparent(),
        CommandSpec::new("zoom.window", "Zoom Window", run_zoom)
            .menu(&["View", "Zoom", "Window"])
            .params("{p1, p2}")
            .noundo()
            .transparent()
            .interactive(|_| Ok(Box::new(ZoomM { window: true, ..Default::default() }))),
        CommandSpec::new("zoom.in", "Zoom In", |s, _| zoom(s, &json!({"mode":"in"}))).menu(&["View", "Zoom", "In"]).noundo().transparent(),
        CommandSpec::new("zoom.out", "Zoom Out", |s, _| zoom(s, &json!({"mode":"out"}))).menu(&["View", "Zoom", "Out"]).noundo().transparent(),
        CommandSpec::new("zoom.all", "Zoom All", |s, _| zoom(s, &json!({"mode":"all"}))).menu(&["View", "Zoom", "All"]).noundo().transparent(),
        CommandSpec::new("zoom.extents", "Zoom Extents", |s, _| zoom(s, &json!({"mode":"extents"})))
            .menu(&["View", "Zoom", "Extents"])
            .noundo()
            .transparent(),
        CommandSpec::new("zoom.object", "Zoom Object", |s, p| {
            zoom(s, &json!({"mode":"object", "handles": p.get("handles").cloned().unwrap_or(Value::Null)}))
        })
        .menu(&["View", "Zoom", "Object"])
        .params("{handles?}")
        .noundo()
        .transparent()
        .interactive(|_| Ok(Box::new(SelectRun::new("zoom.object", "ZOOM")))),
        CommandSpec::new("pan", "Pan", run_pan)
            .menu(&["View", "Pan", "Realtime"])
            .alias(&["p"])
            .params("{delta: [dx,dy]} | {from, to} | {center}")
            .noundo()
            .transparent()
            .interactive(|_| Ok(Box::new(PanM::default()))),
        CommandSpec::new("pan.left", "Pan Left", |s, _| pan_frac(s, Vec2::new(-0.25, 0.0))).menu(&["View", "Pan", "Left"]).noundo().transparent(),
        CommandSpec::new("pan.right", "Pan Right", |s, _| pan_frac(s, Vec2::new(0.25, 0.0))).menu(&["View", "Pan", "Right"]).noundo().transparent(),
        CommandSpec::new("pan.up", "Pan Up", |s, _| pan_frac(s, Vec2::new(0.0, 0.25))).menu(&["View", "Pan", "Up"]).noundo().transparent(),
        CommandSpec::new("pan.down", "Pan Down", |s, _| pan_frac(s, Vec2::new(0.0, -0.25))).menu(&["View", "Pan", "Down"]).noundo().transparent(),
        CommandSpec::new("regen", "Regen", |s, _| {
            s.touch();
            Ok(json!({"message": "Regenerating model."}))
        })
        .menu(&["View", "Regen"])
        .alias(&["re"])
        .noundo(),
        CommandSpec::new("regenall", "Regen All", |s, _| {
            s.touch();
            Ok(json!({"message": "Regenerating all viewports."}))
        })
        .menu(&["View", "Regen All"])
        .alias(&["rea"])
        .noundo(),
        CommandSpec::new("redraw", "Redraw", |s, _| {
            s.touch();
            ok()
        })
        .menu(&["View", "Redraw"])
        .alias(&["r"])
        .noundo()
        .transparent(),
        CommandSpec::new("view.set", "Set View", run_view_set).params("{center: [x,y], height}").noundo(),
        CommandSpec::new("view.get", "Get View", |s, _| {
            let v = s.state()?.view();
            Ok(json!({"center": [v.center.x, v.center.y], "height": v.height, "viewportPx": [s.viewport_px.0, s.viewport_px.1]}))
        })
        .noundo(),
        CommandSpec::new("layout.set", "Switch Layout", run_layout_set).params("{name: \"Model\" | layout name}").noundo(),
    ]
}

fn ext_bounds(s: &Session) -> Result<Bounds2> {
    let space = s.space();
    Ok(s.doc()?.extents(&space))
}

fn fit(s: &mut Session, b: Bounds2) -> Result<()> {
    if b.is_empty() {
        return Ok(());
    }
    let (w, h) = s.viewport_px;
    let st = s.state_mut()?;
    let aspect = st.zoom_aspect(w / h.max(1.0));
    let height = b.height().max(b.width() / aspect.max(1e-6)).max(1e-9);
    st.set_zoom_frame(View { center: b.center(), height });
    Ok(())
}

pub(crate) fn zoom(s: &mut Session, p: &Value) -> Result<Value> {
    let mode = str_param(p, "mode").unwrap_or("extents").to_ascii_lowercase();
    match mode.as_str() {
        "extents" | "e" => s.zoom_extents()?,
        "all" | "a" => {
            let b = ext_bounds(s)?.union(&s.zoom_limits()?);
            fit(s, b.expand(b.height().max(b.width()) * 0.02))?;
        }
        "window" | "w" => {
            let a = point_req("zoom", p, "p1")?;
            let b = point_req("zoom", p, "p2")?;
            fit(s, Bounds2::new(a, b))?;
        }
        "previous" | "p" => {
            let st = s.state_mut()?;
            if let Some(v) = st.view_history.pop() {
                st.set_view_quiet(v);
            } else {
                return Ok(json!({"message": "No previous view saved."}));
            }
        }
        "in" | "out" => {
            let f = if mode == "in" { 2.0 } else { 0.5 };
            let st = s.state_mut()?;
            let v = st.zoom_frame();
            st.set_zoom_frame(View { center: v.center, height: v.height / f });
        }
        "center" | "c" => {
            let c = point_req("zoom", p, "center")?;
            let cur = s.state()?.zoom_frame().height;
            let h = if p.get("factor").is_some() { factor_height(s, p, cur)? } else { f64_or(p, "height", cur) };
            s.state_mut()?.set_zoom_frame(View { center: c, height: h.max(1e-9) });
        }
        "scale" | "s" => {
            let v = s.state()?.zoom_frame();
            let height = factor_height(s, p, v.height)?;
            s.state_mut()?.set_zoom_frame(View { center: v.center, height });
        }
        "object" | "o" => {
            let hs = targets(s, p)?;
            let d = s.doc()?;
            let b = hs.iter().filter_map(|h| d.entity(*h)).fold(Bounds2::EMPTY, |a, e| a.union(&entity_bounds(d, e, 0)));
            fit(s, b.expand(b.height().max(b.width()) * 0.05))?;
        }
        _ => return Err(bad("zoom", "unknown mode")),
    }
    s.touch();
    Ok(Value::Null)
}

/// The view height for a zoom `factor`: relative to the current height `cur` (nX), to paper space
/// with `xp` (nXP; in a viewport `factor` paper units per model unit, elsewhere like nX), or to
/// the limits with `limits` (a plain n: 1 fits the limits).
fn factor_height(s: &Session, p: &Value, cur: f64) -> Result<f64> {
    let f = f64_req("zoom", p, "factor")?;
    if f <= 0.0 {
        return Err(bad("zoom", "factor must be positive"));
    }
    let st = s.state()?;
    if bool_or(p, "xp", false)
        && let Some(vp) = st.zoom_viewport()
    {
        return Ok(vp.height / f);
    }
    if bool_or(p, "limits", false) {
        let lim = s.zoom_limits()?;
        let (w, h) = s.viewport_px;
        let aspect = st.zoom_aspect(w / h.max(1.0)).max(1e-6);
        return Ok(lim.height().max(lim.width() / aspect).max(1e-9) / f);
    }
    Ok(cur / f)
}

/// A typed zoom factor: `n` (relative to the limits), `nX` (to the current view) or `nXP` (to
/// paper space), as `zoom` scale parameters.
fn typed_factor(t: &str) -> Option<Value> {
    let tl = t.trim().to_ascii_lowercase();
    let (num, rel) = if let Some(n) = tl.strip_suffix("xp") {
        (n, "xp")
    } else if let Some(n) = tl.strip_suffix('x') {
        (n, "x")
    } else {
        (tl.as_str(), "limits")
    };
    let f = number(num.trim()).filter(|f| *f > 0.0)?;
    Some(json!({ "factor": f, "xp": rel == "xp", "limits": rel == "limits" }))
}

fn with_mode(mut p: Value, mode: &str, center: Option<Vec2>) -> Value {
    if let Some(o) = p.as_object_mut() {
        o.insert("mode".into(), json!(mode));
        if let Some(c) = center {
            o.insert("center".into(), json!([c.x, c.y]));
        }
    }
    p
}

fn run_zoom(s: &mut Session, p: &Value) -> Result<Value> {
    zoom(s, if p.is_null() { &Value::Null } else { p })
}

fn pan_frac(s: &mut Session, f: Vec2) -> Result<Value> {
    let (w, h) = s.viewport_px;
    let st = s.state_mut()?;
    let v = st.view();
    let ww = v.height * w / h.max(1.0);
    st.set_view(View { center: v.center + Vec2::new(f.x * ww, f.y * v.height), height: v.height });
    ok()
}

fn run_pan(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.state_mut()?;
    let v = st.view();
    let c = if let Some(c) = point_param(p, "center") {
        c
    } else if let Some(d) = point_param(p, "delta") {
        v.center - d
    } else {
        let a = point_req("pan", p, "from")?;
        let b = point_req("pan", p, "to")?;
        v.center - (b - a)
    };
    st.set_view(View { center: c, height: v.height });
    ok()
}

fn run_view_set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = point_req("view.set", p, "center")?;
    let h = f64_req("view.set", p, "height")?;
    if h <= 0.0 {
        return Err(bad("view.set", "height must be positive"));
    }
    s.state_mut()?.set_view(View { center: c, height: h });
    ok()
}

fn run_layout_set(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").unwrap_or("Model");
    let space = if name.eq_ignore_ascii_case("model") {
        Space::Model
    } else {
        let l = s.doc()?.layout(name).ok_or_else(|| bad("layout.set", format!("no layout `{name}`")))?;
        Space::Paper(l.name.clone())
    };
    let st = s.state_mut()?;
    st.space = space.clone();
    st.mspace = None;
    st.selection.clear();
    if let Space::Paper(n) = &space
        && !st.views.iter().any(|(sp, _)| sp == &space)
    {
        // Default paper view: the sheet.
        let page = st.doc.layout(n).map(|l| l.page.clone()).unwrap_or_default();
        let (w, h) = if page.landscape { (page.height_mm, page.width_mm) } else { (page.width_mm, page.height_mm) };
        let unit = if st.doc.header.i64("MEASUREMENT", 0) == 1 { 1.0 } else { 1.0 / 25.4 };
        st.set_view_quiet(View { center: Vec2::new(w * unit / 2.0, h * unit / 2.0), height: h * unit * 1.15 });
    }
    s.touch();
    ok()
}

#[derive(Default)]
struct ZoomM {
    window: bool,
    first: Option<Vec2>,
    stage: ZoomStage,
}

/// The Center and Scale options' own prompts.
#[derive(Clone, Copy, Default)]
enum ZoomStage {
    #[default]
    Main,
    Center,
    CenterHeight(Vec2),
    Scale,
}

impl Interactive for ZoomM {
    fn name(&self) -> &'static str {
        "ZOOM"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.stage {
            ZoomStage::Main => {}
            ZoomStage::Center => return Prompt::new("Specify center point", Accept::POINT),
            ZoomStage::CenterHeight(_) => {
                let h = s.state().map(|st| st.zoom_frame().height).unwrap_or(1.0);
                return Prompt::new("Enter magnification or height", Accept::NUMBER).default(format!("{h:.4}"));
            }
            ZoomStage::Scale => return Prompt::new("Enter a scale factor (nX or nXP)", Accept::NUMBER),
        }
        match (self.window, self.first) {
            (false, None) => Prompt::new("Specify corner of window, enter a scale factor (nX or nXP)", Accept::POINT_OR_NUMBER)
                .kw(&["All", "Center", "Dynamic", "Extents", "Previous", "Scale", "Window", "Object"])
                .default("real time"),
            (true, None) => Prompt::new("Specify first corner", Accept::POINT),
            (_, Some(a)) => Prompt::new("Specify opposite corner", Accept::POINT).base(a),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let bad_factor = || crate::EngineError::Other("Requires a scale factor (n, nX or nXP).".into());
        match (self.stage, &i) {
            (_, Input::Cancel) | (ZoomStage::Main, _) => {}
            (ZoomStage::Center, Input::Point(c)) => {
                self.stage = ZoomStage::CenterHeight(*c);
                return Ok(Step::Continue);
            }
            (ZoomStage::Center, Input::Enter) => return Ok(Step::Done),
            (ZoomStage::Center, _) => return Err(crate::EngineError::Other("Requires a point.".into())),
            (ZoomStage::CenterHeight(c), Input::Enter) => {
                zoom(s, &json!({ "mode": "center", "center": [c.x, c.y] }))?;
                return Ok(Step::Done);
            }
            (ZoomStage::CenterHeight(c), Input::Text(t)) => {
                let tl = t.trim().to_ascii_lowercase();
                // A plain number is the new view height; nX / nXP a magnification.
                let p = if tl.ends_with('x') || tl.ends_with("xp") {
                    typed_factor(&tl).ok_or_else(bad_factor)?
                } else {
                    json!({ "height": number(&tl).filter(|h| *h > 0.0).ok_or_else(bad_factor)? })
                };
                zoom(s, &with_mode(p, "center", Some(c)))?;
                return Ok(Step::Done);
            }
            (ZoomStage::Scale, Input::Text(t)) => {
                zoom(s, &with_mode(typed_factor(t).ok_or_else(bad_factor)?, "scale", None))?;
                return Ok(Step::Done);
            }
            (ZoomStage::Scale, Input::Enter) => return Ok(Step::Done),
            (_, _) => return Err(bad_factor()),
        }
        match i {
            Input::Keyword(k) => {
                let mode = match k.as_str() {
                    "All" => "all",
                    "Extents" => "extents",
                    "Previous" => "previous",
                    "Dynamic" => {
                        // Dynamic zoom drags a view box; without one it asks for a window.
                        s.echo("Dynamic zoom: specify the new view as a window.");
                        self.window = true;
                        return Ok(Step::Continue);
                    }
                    "Window" => {
                        self.window = true;
                        return Ok(Step::Continue);
                    }
                    "Center" => {
                        self.stage = ZoomStage::Center;
                        return Ok(Step::Continue);
                    }
                    "Scale" => {
                        self.stage = ZoomStage::Scale;
                        return Ok(Step::Continue);
                    }
                    "Object" => "object",
                    _ => return Err(crate::EngineError::Other("Invalid option keyword.".into())),
                };
                zoom(s, &json!({ "mode": mode }))?;
                Ok(Step::Done)
            }
            Input::Point(p) => match self.first {
                None => {
                    self.first = Some(p);
                    Ok(Step::Continue)
                }
                Some(a) => {
                    fit(s, Bounds2::new(a, p))?;
                    Ok(Step::Done)
                }
            },
            Input::Text(t) => {
                // nX: relative to the current view; nXP: to paper space; n: to the limits.
                let p = typed_factor(&t).ok_or_else(|| crate::EngineError::Other("Requires a point, scale factor or option keyword.".into()))?;
                zoom(s, &with_mode(p, "scale", None))?;
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<cadcraft_doc::EntityKind> {
        self.first.map(|a| vec![super::helpers::lwpoly(super::helpers::rect_vertices(a, c), true)]).unwrap_or_default()
    }
}

#[derive(Default)]
struct PanM {
    first: Option<Vec2>,
}

impl Interactive for PanM {
    fn name(&self) -> &'static str {
        "PAN"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.first {
            None => Prompt::new("Specify base point or displacement (drag with the middle mouse button for realtime pan)", Accept::POINT),
            Some(a) => Prompt::new("Specify second point", Accept::POINT).base(a),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.first, i) {
            (None, Input::Point(p)) => {
                self.first = Some(p);
                Ok(Step::Continue)
            }
            (Some(a), Input::Point(b)) => {
                run_pan(s, &json!({ "from": [a.x, a.y], "to": [b.x, b.y] }))?;
                Ok(Step::Done)
            }
            (_, Input::Enter) => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}
