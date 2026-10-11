//! Utilities: CAL (expression calculator), FIND (find and replace text), MASSPROP, MEASUREGEOM.

use cadcraft_doc::{EntityKind, Handle};
use cadcraft_geom::{Polyline, Vec2};
use serde_json::{Value, json};

use super::*;
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("cal", "QuickCalc", run_cal)
            .menu(&["Tools", "Inquiry", "Quick"])
            .alias(&["quickcalc", "qc"])
            .params("{expr: \"(3+4)*2^2\", vars?: {x: 1}}")
            .enabled(always)
            .noundo()
            .transparent(),
        CommandSpec::new("find", "Find...", run_find).menu(&["Edit", "Find..."]).params("{find, replace?, matchCase?: bool, wholeWord?: bool}"),
        CommandSpec::new("massprop", "Region/Mass Properties", run_massprop)
            .menu(&["Tools", "Inquiry", "Region/Mass Properties"])
            .params("{handles?}")
            .noundo(),
        CommandSpec::new("measuregeom", "Measure Geometry", run_measuregeom).params("{mode: distance|radius|angle|area, ...}").noundo(),
        CommandSpec::new("radius.measure", "Radius", run_radius).menu(&["Tools", "Inquiry", "Radius"]).params("{handle}").noundo(),
        CommandSpec::new("angle.measure", "Angle", run_angle).menu(&["Tools", "Inquiry", "Angle"]).params("{vertex, p1, p2} | {h1, h2}").noundo(),
    ]
}

// ---------------- CAL ----------------

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    vars: &'a serde_json::Map<String, Value>,
    depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn expr(&mut self) -> std::result::Result<f64, String> {
        self.depth += 1;
        if self.depth > 200 {
            return Err("expression too deeply nested".into());
        }
        let mut v = self.term()?;
        while let Some(c) = self.peek() {
            match c {
                b'+' => {
                    self.i += 1;
                    v += self.term()?;
                }
                b'-' => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => break,
            }
        }
        self.depth -= 1;
        Ok(v)
    }
    fn term(&mut self) -> std::result::Result<f64, String> {
        let mut v = self.unary()?;
        while let Some(c) = self.peek() {
            match c {
                b'*' => {
                    self.i += 1;
                    v *= self.unary()?;
                }
                b'/' => {
                    self.i += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return Err("division by zero".into());
                    }
                    v /= d;
                }
                _ => break,
            }
        }
        Ok(v)
    }
    fn power(&mut self) -> std::result::Result<f64, String> {
        let b = self.atom()?;
        if self.peek() == Some(b'^') {
            self.i += 1;
            let e = self.unary()?;
            return Ok(b.powf(e));
        }
        Ok(b)
    }
    fn unary(&mut self) -> std::result::Result<f64, String> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }
    fn atom(&mut self) -> std::result::Result<f64, String> {
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(b')') {
                    return Err("missing )".into());
                }
                self.i += 1;
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => {
                let st = self.i;
                while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.' || *c == b'e' || *c == b'E') {
                    self.i += 1;
                }
                let t = std::str::from_utf8(self.s.get(st..self.i).unwrap_or(&[])).unwrap_or("");
                t.parse::<f64>().map_err(|_| format!("bad number `{t}`"))
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let st = self.i;
                while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                    self.i += 1;
                }
                let name = std::str::from_utf8(self.s.get(st..self.i).unwrap_or(&[])).unwrap_or("").to_ascii_lowercase();
                if self.peek() == Some(b'(') {
                    self.i += 1;
                    let a = self.expr()?;
                    let b = if self.peek() == Some(b',') {
                        self.i += 1;
                        Some(self.expr()?)
                    } else {
                        None
                    };
                    if self.peek() != Some(b')') {
                        return Err("missing )".into());
                    }
                    self.i += 1;
                    return Ok(match (name.as_str(), b) {
                        ("sin", _) => a.to_radians().sin(),
                        ("cos", _) => a.to_radians().cos(),
                        ("tan", _) => a.to_radians().tan(),
                        ("asin", _) => a.asin().to_degrees(),
                        ("acos", _) => a.acos().to_degrees(),
                        ("atan", None) => a.atan().to_degrees(),
                        ("atan", Some(x)) => a.atan2(x).to_degrees(),
                        ("sqrt", _) => a.sqrt(),
                        ("sqr", _) => a * a,
                        ("abs", _) => a.abs(),
                        ("ln", _) => a.ln(),
                        ("log", _) => a.log10(),
                        ("exp", _) => a.exp(),
                        ("round", _) => a.round(),
                        ("trunc", _) => a.trunc(),
                        ("r2d", _) => a.to_degrees(),
                        ("d2r", _) => a.to_radians(),
                        ("min", Some(x)) => a.min(x),
                        ("max", Some(x)) => a.max(x),
                        ("pow", Some(x)) => a.powf(x),
                        _ => return Err(format!("unknown function `{name}`")),
                    });
                }
                match name.as_str() {
                    "pi" => Ok(std::f64::consts::PI),
                    "e" => Ok(std::f64::consts::E),
                    _ => self.vars.get(&name).and_then(Value::as_f64).ok_or_else(|| format!("unknown name `{name}`")),
                }
            }
            _ => Err("expected a number".into()),
        }
    }
}

/// Evaluate an arithmetic expression (degrees for trig, `^` power, functions, pi).
pub fn eval(expr: &str, vars: &serde_json::Map<String, Value>) -> std::result::Result<f64, String> {
    if expr.len() > 10_000 {
        return Err("expression too long".into());
    }
    let mut p = Parser { s: expr.as_bytes(), i: 0, vars, depth: 0 };
    let v = p.expr()?;
    if p.peek().is_some() {
        return Err(format!("unexpected `{}`", expr.get(p.i..).unwrap_or("")));
    }
    if !v.is_finite() {
        return Err("result is not a finite number".into());
    }
    Ok(v)
}

fn run_cal(_s: &mut Session, p: &Value) -> Result<Value> {
    let e = str_param(p, "expr").ok_or_else(|| bad("cal", "`expr` is required"))?;
    let vars = p.get("vars").and_then(Value::as_object).cloned().unwrap_or_default();
    let v = eval(e, &vars).map_err(|m| bad("cal", m))?;
    Ok(json!({ "value": v, "message": format!("{v}") }))
}

// ---------------- FIND ----------------

fn replace_in(text: &str, find: &str, rep: &str, case: bool, whole: bool) -> (String, usize) {
    if find.is_empty() {
        return (text.to_string(), 0);
    }
    let hay = if case { text.to_string() } else { text.to_lowercase() };
    let needle = if case { find.to_string() } else { find.to_lowercase() };
    let mut out = String::new();
    let mut n = 0;
    let mut last = 0;
    let mut start = 0;
    while let Some(pos) = hay.get(start..).and_then(|h| h.find(&needle)).map(|p| p + start) {
        let end = pos + needle.len();
        let boundary = |i: usize| hay.get(..i).and_then(|s| s.chars().last()).is_none_or(|c| !c.is_alphanumeric());
        let after = hay.get(end..).and_then(|s| s.chars().next()).is_none_or(|c| !c.is_alphanumeric());
        if !whole || (boundary(pos) && after) {
            out.push_str(text.get(last..pos).unwrap_or(""));
            out.push_str(rep);
            last = end;
            n += 1;
        }
        start = end.max(pos + 1);
        if start >= hay.len() {
            break;
        }
    }
    out.push_str(text.get(last..).unwrap_or(""));
    (out, n)
}

fn run_find(s: &mut Session, p: &Value) -> Result<Value> {
    let find = str_param(p, "find").ok_or_else(|| bad("find", "`find` is required"))?.to_string();
    let rep = str_param(p, "replace").map(str::to_string);
    let case = bool_or(p, "matchCase", false);
    let whole = bool_or(p, "wholeWord", false);
    let space = s.space();
    let d = s.doc()?;
    let hs: Vec<Handle> = d.space(&space).map(|st| st.handles()).unwrap_or_default();
    let mut found: Vec<Value> = Vec::new();
    let mut total = 0;
    let mut edits: Vec<(Handle, EntityKind)> = Vec::new();
    for h in hs {
        let Some(e) = d.entity(h) else { continue };
        let mut k = e.kind.clone();
        let mut hits = 0;
        let r = rep.as_deref().unwrap_or(&find);
        let mut apply = |t: &mut String| {
            let (nt, n) = replace_in(t, &find, r, case, whole);
            if n > 0 {
                hits += n;
                if rep.is_some() {
                    *t = nt;
                }
            }
        };
        match &mut k {
            EntityKind::Text(t) => apply(&mut t.value),
            EntityKind::MText(t) => apply(&mut t.contents),
            EntityKind::Insert(i) => i.attribs.iter_mut().for_each(|a| apply(&mut a.text.value)),
            EntityKind::Dimension(dm) => apply(&mut dm.text),
            EntityKind::MLeader(m) => {
                if let Some(t) = &mut m.text {
                    apply(&mut t.contents)
                }
            }
            EntityKind::Table(t) => t.cells.iter_mut().flatten().for_each(|c| apply(&mut c.text)),
            _ => {}
        }
        if hits > 0 {
            total += hits;
            found.push(json!({ "handle": h.hex(), "type": e.kind.type_name(), "count": hits }));
            if rep.is_some() {
                edits.push((h, k));
            }
        }
    }
    let n = edits.len();
    if rep.is_some() {
        let doc = s.doc_mut()?;
        for (h, k) in edits {
            doc.modify_entity(h, |e| {
                e.kind = k;
                if let EntityKind::Dimension(dm) = &mut e.kind {
                    dm.block = None;
                }
            })?;
        }
    } else {
        let sel: Vec<Handle> = found.iter().filter_map(|v| v["handle"].as_str().and_then(Handle::parse_hex)).collect();
        s.set_selection(sel);
    }
    let msg =
        if rep.is_some() { format!("{total} replacement(s) in {n} object(s)") } else { format!("{total} match(es) in {} object(s)", found.len()) };
    Ok(json!({ "matches": found, "total": total, "message": msg }))
}

// ---------------- MASSPROP / MEASUREGEOM ----------------

/// Area, perimeter, centroid and second moments of a closed boundary.
fn region_props(pts: &[Vec2]) -> Option<(f64, f64, Vec2, f64, f64)> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let (mut a, mut cx, mut cy, mut ixx, mut iyy, mut per) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (*pts.get(i)?, *pts.get((i + 1) % n)?);
        let c = p.cross(q);
        a += c;
        cx += (p.x + q.x) * c;
        cy += (p.y + q.y) * c;
        ixx += (p.y * p.y + p.y * q.y + q.y * q.y) * c;
        iyy += (p.x * p.x + p.x * q.x + q.x * q.x) * c;
        per += p.dist(q);
    }
    a /= 2.0;
    if a.abs() < 1e-15 {
        return None;
    }
    let c = Vec2::new(cx / (6.0 * a), cy / (6.0 * a));
    Some((a.abs(), per, c, (ixx / 12.0).abs(), (iyy / 12.0).abs()))
}

fn closed_points(k: &EntityKind) -> Option<Vec<Vec2>> {
    match k {
        EntityKind::Circle(c) => {
            let mut v = Vec::new();
            cadcraft_geom::Circle::new(c.center.xy(), c.radius).tessellate(c.radius * 1e-5, &mut v);
            v.pop();
            Some(v)
        }
        EntityKind::LwPolyline(p) => {
            let mut v = Polyline { vertices: p.vertices.clone(), closed: true }.tessellate(1e-5);
            if v.len() > 1 && v.first().zip(v.last()).is_some_and(|(a, b)| a.near(*b, 1e-12)) {
                v.pop();
            }
            Some(v)
        }
        EntityKind::Hatch(h) => h.loops.first().map(|l| Polyline { vertices: l.vertices.clone(), closed: true }.tessellate(1e-5)),
        _ => None,
    }
}

fn run_massprop(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let d = s.doc()?;
    let mut out = Vec::new();
    let mut lines = Vec::new();
    for h in hs {
        let Some(e) = d.entity(h) else { continue };
        let Some((a, per, c, ixx, iyy)) = closed_points(&e.kind).and_then(|v| region_props(&v)) else { continue };
        lines.push(format!("----------------   REGIONS   ----------------\nArea:                    {a:.4}\nPerimeter:               {per:.4}\nCentroid:                X: {:.4}  Y: {:.4}\nMoments of inertia:      X: {ixx:.4}  Y: {iyy:.4}", c.x, c.y));
        out.push(json!({ "handle": h.hex(), "area": a, "perimeter": per, "centroid": [c.x, c.y], "ixx": ixx, "iyy": iyy }));
    }
    if out.is_empty() {
        return Err(EngineError::Other("Select closed objects (regions, closed polylines, circles).".into()));
    }
    Ok(json!({ "objects": out, "message": lines.join("\n") }))
}

fn run_radius(s: &mut Session, p: &Value) -> Result<Value> {
    let h = targets(s, p)?.first().copied().ok_or_else(|| bad("radius.measure", "`handle` is required"))?;
    let r = match s.doc()?.entity(h).map(|e| e.kind.clone()) {
        Some(EntityKind::Circle(c)) => c.radius,
        Some(EntityKind::Arc(a)) => a.radius,
        _ => return Err(bad("radius.measure", "select an arc or circle")),
    };
    Ok(json!({ "radius": r, "diameter": 2.0 * r, "message": format!("Radius = {r:.4}\nDiameter = {:.4}", 2.0 * r) }))
}

fn run_angle(s: &mut Session, p: &Value) -> Result<Value> {
    let (v, a, b) = if let (Some(v), Some(a), Some(b)) = (point_param(p, "vertex"), point_param(p, "p1"), point_param(p, "p2")) {
        (v, a, b)
    } else {
        let hp = |k: &str| p.get(k).and_then(Value::as_str).and_then(Handle::parse_hex);
        let (Some(h1), Some(h2)) = (hp("h1"), hp("h2")) else { return Err(bad("angle.measure", "give {vertex,p1,p2} or {h1,h2}")) };
        let d = s.doc()?;
        let line = |h: Handle| match d.entity(h).map(|e| e.kind.clone()) {
            Some(EntityKind::Line(l)) => Some((l.a.xy(), l.b.xy())),
            _ => None,
        };
        let ((a1, a2), (b1, b2)) =
            (line(h1).ok_or_else(|| bad("angle.measure", "h1 is not a line"))?, line(h2).ok_or_else(|| bad("angle.measure", "h2 is not a line"))?);
        let (x, _, _) = cadcraft_geom::line_line_infinite(a1, a2, b1, b2).ok_or_else(|| bad("angle.measure", "lines are parallel"))?;
        let far = |p: Vec2, q: Vec2| if p.dist(x) > q.dist(x) { p } else { q };
        (x, far(a1, a2), far(b1, b2))
    };
    let mut ang = (cadcraft_geom::norm_angle(v.angle_to(b) - v.angle_to(a))).to_degrees();
    if ang > 180.0 {
        ang = 360.0 - ang;
    }
    Ok(json!({ "angle": ang, "message": format!("Angle = {ang:.4}°") }))
}

fn run_measuregeom(s: &mut Session, p: &Value) -> Result<Value> {
    match str_param(p, "mode").unwrap_or("distance").to_ascii_lowercase().as_str() {
        "distance" | "d" => s.execute("dist", p),
        "radius" | "r" => run_radius(s, p),
        "angle" | "a" => run_angle(s, p),
        "area" | "ar" => s.execute("area", p),
        _ => Err(bad("measuregeom", "mode must be distance, radius, angle or area")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculator() {
        let v = serde_json::Map::new();
        assert_eq!(eval("(3+4)*2^2", &v), Ok(28.0));
        assert!((eval("sin(30)", &v).unwrap() - 0.5).abs() < 1e-12);
        assert!((eval("2*pi", &v).unwrap() - std::f64::consts::TAU).abs() < 1e-12);
        assert_eq!(eval("-2^2", &v), Ok(-4.0));
        assert!(eval("1/0", &v).is_err());
        assert!(eval("((((1", &v).is_err());
        assert!(eval("foo(1)", &v).is_err());
        assert!(eval(&"(".repeat(5000), &v).is_err());
        let mut vars = serde_json::Map::new();
        vars.insert("x".into(), json!(3));
        assert_eq!(eval("x*x", &vars), Ok(9.0));
    }

    #[test]
    fn find_and_replace() {
        assert_eq!(replace_in("Bolt bolt BOLTS", "bolt", "Pin", false, true), ("Pin Pin BOLTS".to_string(), 2));
        assert_eq!(replace_in("aaa", "a", "b", true, false), ("bbb".to_string(), 3));
        assert_eq!(replace_in("x", "", "y", true, false).1, 0);
    }

    #[test]
    fn rectangle_mass_properties() {
        let (a, per, c, _, _) = region_props(&[Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(4.0, 2.0), Vec2::new(0.0, 2.0)]).unwrap();
        assert!((a - 8.0).abs() < 1e-12);
        assert!((per - 12.0).abs() < 1e-12);
        assert!(c.near(Vec2::new(2.0, 1.0), 1e-12));
    }
}
