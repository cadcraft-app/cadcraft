//! Object snap tracking (OTRACK, F11), polar tracking paths and the snaps that work through
//! acquired points: Extension (EXT), Parallel (PAR) and extended Apparent Intersection (APP).
//!
//! Resting the cursor on an object snap point for [`DWELL`] seconds *acquires* it (the UI marks
//! it with a small `+`); resting on it again releases it. Up to [`MAX_ACQUIRED`] points are
//! kept, the oldest dropped first. Acquired points send alignment paths: horizontal and vertical,
//! or every polar angle when POLARMODE bit 2 says so. With EXT running, an acquired line or arc
//! endpoint also sends the object's extension; with PAR, a line rested on gives a path parallel
//! to it through the base point; with APP, an object rested on gives its whole extension, so
//! where it would meet another object snaps as an Apparent Intersection.
//!
//! [`Tracker::track`] then finds, near the cursor, in this order: the intersection of two paths,
//! a path crossing an object, or the nearest point on one path. Everything here is plain data
//! and pure functions over the drawing, so scripts and tests drive it without the UI
//! ([`crate::Session::snap_cursor`]).

use cadcraft_doc::{Drawing, Prim, Space};
use cadcraft_geom::{Line, PI, Segment, TAU, Vec2, angle_in_sweep, intersect_ext, norm_angle};
use serde::Serialize;

use super::{SnapHit, mode};

/// The most points kept acquired (as AutoCAD: seven).
pub const MAX_ACQUIRED: usize = 7;
/// Seconds the cursor rests on a point before it is acquired (or released).
pub const DWELL: f64 = 0.4;
/// The most polar angles one path source sends (a tiny POLARANG is hostile input).
const MAX_ANGLES: usize = 360;
/// The most paths looked at near the cursor.
const MAX_NEAR_PATHS: usize = 64;

/// POLARMODE bits.
pub mod polarmode {
    /// Polar angles measured relative to the last segment (not modelled yet).
    pub const RELATIVE: u32 = 1;
    /// Object snap tracking uses every polar angle, not only horizontal and vertical.
    pub const TRACK_POLAR: u32 = 2;
    /// The additional polar angles (POLARADDANG) are tracked too.
    pub const ADDITIONAL: u32 = 4;
    /// Points are acquired by holding Shift, not by resting the cursor on them.
    pub const SHIFT_ACQUIRE: u32 = 8;
}

/// The shape of a tracking path.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PathShape {
    /// From `origin` along the unit direction `dir`.
    Ray { origin: Vec2, dir: Vec2 },
    /// The whole line through `origin` along `dir`.
    Line { origin: Vec2, dir: Vec2 },
    /// The circle an arc lies on, without the arc itself (`gap`: its start and end angles, the
    /// arc running counterclockwise from one to the other) when it is the arc's extension.
    Circle { center: Vec2, radius: f64, gap: Option<(f64, f64)> },
}

impl PathShape {
    /// The closest point of the path to `p`, if any part of the path is closest there.
    pub fn foot(&self, p: Vec2) -> Option<Vec2> {
        let f = match *self {
            PathShape::Ray { origin, dir } => {
                let t = (p - origin).dot(dir);
                (t > 0.0).then(|| origin + dir * t)?
            }
            PathShape::Line { origin, dir } => origin + dir * (p - origin).dot(dir),
            PathShape::Circle { center, radius, .. } => {
                let u = (p - center).normalized();
                if u == Vec2::ZERO {
                    return None;
                }
                center + u * radius
            }
        };
        (f.is_finite() && self.contains(f)).then_some(f)
    }

    /// Whether `p` (a point of the path's line or circle) is on the path.
    fn contains(&self, p: Vec2) -> bool {
        match *self {
            PathShape::Ray { origin, dir } => (p - origin).dot(dir) >= -cadcraft_geom::EPS,
            PathShape::Line { .. } => true,
            PathShape::Circle { center, gap, .. } => gap.is_none_or(|(s, e)| !angle_in_sweep(center.angle_to(p), s, e)),
        }
    }

    /// The path's line or circle as a segment to intersect (`intersect_ext` extends it).
    fn segment(&self) -> Segment {
        match *self {
            PathShape::Ray { origin, dir } | PathShape::Line { origin, dir } => Segment::Line(Line::new(origin, origin + dir)),
            PathShape::Circle { center, radius, .. } => Segment::Arc { arc: cadcraft_geom::Arc::new(center, radius, 0.0, PI), ccw: true },
        }
    }

    fn is_finite(&self) -> bool {
        match *self {
            PathShape::Ray { origin, dir } | PathShape::Line { origin, dir } => origin.is_finite() && dir.is_finite() && dir != Vec2::ZERO,
            PathShape::Circle { center, radius, .. } => center.is_finite() && radius.is_finite() && radius > cadcraft_geom::EPS,
        }
    }
}

/// What sent a tracking path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PathKind {
    /// Polar tracking from the base point.
    Polar,
    /// Object snap tracking from an acquired point.
    Alignment,
    /// The extension of a line or arc from its acquired endpoint (EXT).
    Extension,
    /// Parallel to an acquired line, through the base point (PAR).
    Parallel,
    /// The whole extension of an acquired object (APP): only its crossings snap.
    Apparent,
}

/// One tracking path.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Path {
    pub shape: PathShape,
    pub kind: PathKind,
    /// The point it is measured from (tooltips, direct distance entry): the acquired point, or
    /// the base point.
    pub from: Vec2,
    /// The tooltip name: the acquired point's snap ("Endpoint" …), "Polar", "Extension",
    /// "Parallel" or "Apparent Intersection".
    pub name: &'static str,
}

impl Path {
    /// Whether the cursor slides along this path (an Apparent Intersection path only snaps
    /// where it crosses something).
    fn slides(&self) -> bool {
        self.kind != PathKind::Apparent
    }
}

/// A point acquired for tracking.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Acquired {
    /// An object snap point; `ext` is the extension path when it is a line or arc endpoint.
    Point {
        at: Vec2,
        name: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        ext: Option<PathShape>,
    },
    /// A line rested on with Parallel running: `at` where, `dir` its unit direction.
    Parallel { at: Vec2, dir: Vec2 },
    /// An object rested on with Apparent Intersection running, and its whole extension.
    Apparent { at: Vec2, shape: PathShape },
}

impl Acquired {
    /// Where the acquisition marker goes.
    pub fn at(&self) -> Vec2 {
        match *self {
            Acquired::Point { at, .. } | Acquired::Parallel { at, .. } | Acquired::Apparent { at, .. } => at,
        }
    }

    /// The same point or object (so resting on it again releases it).
    fn same(&self, o: &Acquired) -> bool {
        let near = |a: Vec2, b: Vec2| a.dist(b) <= 1e-9 * (1.0 + a.len().max(b.len()));
        match (self, o) {
            (Acquired::Point { at: a, .. }, Acquired::Point { at: b, .. }) => near(*a, *b),
            (Acquired::Parallel { dir: a, at: p }, Acquired::Parallel { dir: b, at: q }) => {
                a.cross(*b).abs() < 1e-9 && (*p - *q).cross(*a).abs() <= 1e-9 * (1.0 + p.len())
            }
            (Acquired::Apparent { shape: a, .. }, Acquired::Apparent { shape: b, .. }) => same_shape(a, b),
            _ => false,
        }
    }
}

fn same_shape(a: &PathShape, b: &PathShape) -> bool {
    match (*a, *b) {
        (PathShape::Line { origin: o1, dir: d1 }, PathShape::Line { origin: o2, dir: d2 }) => {
            d1.cross(d2).abs() < 1e-9 && (o2 - o1).cross(d1).abs() <= 1e-9 * (1.0 + o1.len())
        }
        (PathShape::Circle { center: c1, radius: r1, .. }, PathShape::Circle { center: c2, radius: r2, .. }) => {
            c1.dist(c2) <= 1e-9 * (1.0 + c1.len()) && (r1 - r2).abs() <= 1e-9 * (1.0 + r1)
        }
        _ => false,
    }
}

/// The settings tracking reads, resolved once per cursor update.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackConfig {
    /// OTRACK (F11): acquired points send alignment paths.
    pub otrack: bool,
    /// Running object snaps (OSMODE, with the F3 off bit).
    pub osmode: u32,
    /// Directions (radians) polar tracking follows from the base point; empty when polar is off
    /// or ortho is on.
    pub polar_angles: Vec<f64>,
    /// Directions (radians) alignment paths follow from acquired points.
    pub track_angles: Vec<f64>,
    /// POLARMODE bit 8: acquire with Shift instead of resting the cursor.
    pub shift_acquire: bool,
}

impl TrackConfig {
    /// From the session settings and ANGBASE (radians), from which polar angles are measured.
    pub fn new(s: &crate::Settings, angbase: f64) -> Self {
        let polar = polar_angles(s.polarang, if s.polar_flags & polarmode::ADDITIONAL != 0 { &s.polaraddang } else { &[] }, angbase);
        let track_angles = if s.polar_flags & polarmode::TRACK_POLAR != 0 { polar.clone() } else { orthogonal() };
        TrackConfig {
            otrack: s.otrack,
            osmode: s.osmode,
            polar_angles: if s.polarmode && !s.orthomode { polar } else { Vec::new() },
            track_angles,
            shift_acquire: s.polar_flags & polarmode::SHIFT_ACQUIRE != 0,
        }
    }

    fn running(&self, bit: u32) -> bool {
        self.osmode & mode::OFF == 0 && self.osmode & bit != 0
    }
}

/// Horizontal and vertical, both ways.
fn orthogonal() -> Vec<f64> {
    vec![0.0, PI / 2.0, PI, 1.5 * PI]
}

/// Every multiple of the increment `inc` (POLARANG) around the circle, plus the `additional`
/// angles (POLARADDANG), all measured from `angbase`; radians, normalized, without repeats and
/// at most [`MAX_ANGLES`] of them.
pub fn polar_angles(inc: f64, additional: &[f64], angbase: f64) -> Vec<f64> {
    let angbase = if angbase.is_finite() { angbase } else { 0.0 };
    let mut v: Vec<f64> = Vec::new();
    let mut push = |a: f64| {
        let a = norm_angle(a + angbase);
        if v.len() < MAX_ANGLES && !v.iter().any(|b| (norm_angle(a - b + PI) - PI).abs() < 1e-9) {
            v.push(a);
        }
    };
    if inc.is_finite() && inc > 1e-6 {
        let n = ((TAU / inc) - 1e-9).ceil().clamp(1.0, MAX_ANGLES as f64) as usize;
        for k in 0..n {
            push(k as f64 * inc);
        }
    } else {
        push(0.0);
    }
    for a in additional.iter().filter(|a| a.is_finite()) {
        push(*a);
    }
    v
}

/// The tracking result at the cursor.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub point: Vec2,
    /// The paths the point lies on (one, or two where they cross).
    pub paths: Vec<Path>,
    /// Set where a path crosses an object: "Intersection", or "Apparent Intersection" for an
    /// object's extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<&'static str>,
}

impl Track {
    /// The point and unit direction a typed distance is measured from and along: the one
    /// straight path the cursor slides on (`None` at crossings and on arc extensions).
    pub fn along(&self) -> Option<(Vec2, Vec2)> {
        let [p] = self.paths.as_slice() else { return None };
        if self.object.is_some() || matches!(p.shape, PathShape::Circle { .. }) {
            return None;
        }
        let dir = (self.point - p.from).normalized();
        (dir != Vec2::ZERO && dir.is_finite()).then_some((p.from, dir))
    }
}

/// Acquired points, the acquisition in progress and the last tracking result.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tracker {
    pub acquired: Vec<Acquired>,
    /// The candidate under the cursor and when the cursor came to it.
    #[serde(skip)]
    pending: Option<(Acquired, f64)>,
    /// The candidate last acquired or released: the cursor must leave it before it toggles again.
    #[serde(skip)]
    toggled: Option<Acquired>,
    /// The result of the latest [`Tracker::track`] (direct distance entry follows its path).
    pub last: Option<Track>,
}

impl Tracker {
    /// Forget every acquired point (a command ends, the prompt no longer wants a point).
    pub fn clear(&mut self) {
        *self = Tracker::default();
    }

    /// Acquire or release `cand` by hand, as resting the cursor on it does.
    pub fn toggle(&mut self, cand: Acquired) {
        if let Some(i) = self.acquired.iter().position(|a| a.same(&cand)) {
            self.acquired.remove(i);
        } else {
            self.acquired.push(cand);
            if self.acquired.len() > MAX_ACQUIRED {
                self.acquired.remove(0);
            }
        }
    }

    /// The cursor is over `cand` (or nothing) at `now` seconds: acquires or releases it once it
    /// has rested there for [`DWELL`] seconds, or at once when `shift` is held in Shift-to-acquire
    /// mode. Returns true while an acquisition is pending (the UI should redraw again soon).
    pub fn hover(&mut self, cand: Option<Acquired>, now: f64, shift_acquire: bool, shift: bool) -> bool {
        let Some(c) = cand else {
            self.pending = None;
            self.toggled = None;
            return false;
        };
        if self.toggled.is_some_and(|t| t.same(&c)) {
            return false;
        }
        self.toggled = None;
        if shift_acquire {
            self.pending = None;
            if shift {
                self.toggle(c);
                self.toggled = Some(c);
            }
            return false;
        }
        match self.pending {
            Some((p, t0)) if p.same(&c) && now.is_finite() && t0.is_finite() => {
                if now - t0 >= DWELL {
                    self.pending = None;
                    self.toggle(c);
                    self.toggled = Some(c);
                    false
                } else {
                    true
                }
            }
            _ => {
                self.pending = Some((c, now));
                true
            }
        }
    }

    /// The paths the acquired points (and the base point) send.
    pub fn paths(&self, cfg: &TrackConfig, base: Option<Vec2>) -> Vec<Path> {
        let mut out = Vec::new();
        let ray = |origin: Vec2, a: f64| PathShape::Ray { origin, dir: Vec2::from_angle(a) };
        if let Some(b) = base.filter(|b| b.is_finite()) {
            out.extend(cfg.polar_angles.iter().map(|a| Path { shape: ray(b, *a), kind: PathKind::Polar, from: b, name: "Polar" }));
        }
        for acq in &self.acquired {
            match *acq {
                Acquired::Point { at, name, ext } => {
                    if cfg.otrack {
                        out.extend(cfg.track_angles.iter().map(|a| Path { shape: ray(at, *a), kind: PathKind::Alignment, from: at, name }));
                    }
                    if let Some(shape) = ext.filter(|_| cfg.running(mode::EXT)) {
                        out.push(Path { shape, kind: PathKind::Extension, from: at, name: "Extension" });
                    }
                }
                Acquired::Parallel { dir, .. } => {
                    if let Some(b) = base.filter(|b| b.is_finite())
                        && cfg.running(mode::PAR)
                    {
                        for d in [dir, -dir] {
                            out.push(Path { shape: PathShape::Ray { origin: b, dir: d }, kind: PathKind::Parallel, from: b, name: "Parallel" });
                        }
                    }
                }
                Acquired::Apparent { at, shape } => {
                    if cfg.running(mode::APP) {
                        out.push(Path { shape, kind: PathKind::Apparent, from: at, name: "Apparent Intersection" });
                    }
                }
            }
        }
        out.retain(|p| p.shape.is_finite() && p.from.is_finite());
        out
    }

    /// The tracking point near `cursor`: the crossing of two paths, then a path crossing an
    /// object of the drawing, then the nearest point of one path. `tol` is how close (world
    /// units) the cursor must come to a path to slide on it, `aperture` how close a crossing
    /// must be. Stores the result in [`Tracker::last`].
    #[allow(clippy::too_many_arguments)]
    pub fn track(
        &mut self,
        d: &Drawing,
        space: &Space,
        cursor: Vec2,
        aperture: f64,
        tol: f64,
        cfg: &TrackConfig,
        base: Option<Vec2>,
    ) -> Option<Track> {
        let t = self.find(d, space, cursor, aperture, tol, cfg, base);
        self.last = t.clone();
        t
    }

    #[allow(clippy::too_many_arguments)]
    fn find(&self, d: &Drawing, space: &Space, cursor: Vec2, aperture: f64, tol: f64, cfg: &TrackConfig, base: Option<Vec2>) -> Option<Track> {
        if !(cursor.is_finite() && aperture.is_finite() && tol.is_finite() && aperture > 0.0 && tol > 0.0) {
            return None;
        }
        let reach = aperture.max(tol);
        let near: Vec<(Path, Vec2, f64)> = self
            .paths(cfg, base)
            .into_iter()
            .filter_map(|p| {
                let f = p.shape.foot(cursor)?;
                let dd = f.dist(cursor);
                // Not right at the point the path comes from: every path passes there.
                (dd <= reach && f.dist(p.from) > tol).then_some((p, f, dd))
            })
            .take(MAX_NEAR_PATHS)
            .collect();
        if near.is_empty() {
            return None;
        }
        // 1. Two paths crossing.
        let mut best: Option<(f64, Track)> = None;
        let offer = |dd: f64, t: Track, best: &mut Option<(f64, Track)>| {
            if t.point.is_finite() && best.as_ref().is_none_or(|(bd, _)| dd < *bd) {
                *best = Some((dd, t));
            }
        };
        for (i, (a, _, _)) in near.iter().enumerate() {
            for (b, _, _) in near.iter().skip(i + 1) {
                if a.from.dist(b.from) <= cadcraft_geom::EPS {
                    continue;
                }
                for x in intersect_ext(&a.shape.segment(), &b.shape.segment(), true) {
                    let dd = x.dist(cursor);
                    if dd <= aperture && a.shape.contains(x) && b.shape.contains(x) {
                        let object = (a.kind == PathKind::Apparent && b.kind == PathKind::Apparent).then_some("Apparent Intersection");
                        offer(dd, Track { point: x, paths: vec![*a, *b], object }, &mut best);
                    }
                }
            }
        }
        if let Some((_, t)) = best {
            return Some(t);
        }
        // 2. A path crossing an object.
        if cfg.running(mode::INT | mode::APP) {
            let ix = crate::spatial::index(d, space);
            let prims = super::gather(d, space, &ix, cursor, aperture, mode::NEA).map(|(_, p)| p).unwrap_or_default();
            for prim in prims.iter().take(200) {
                let extend = cfg.running(mode::APP) && super::extendable(prim);
                for seg in super::prim_segments(prim) {
                    let seg = super::fix_circle(&seg);
                    for (p, _, _) in &near {
                        for x in intersect_ext(&p.shape.segment(), &seg, true) {
                            let on_object = seg.closest(x).dist(x) <= 1e-9 * (1.0 + x.len());
                            // Off the object, or along an object's extension: apparent.
                            let apparent = !on_object || p.kind == PathKind::Apparent;
                            let dd = x.dist(cursor);
                            if dd > aperture
                                || !p.shape.contains(x)
                                || x.dist(p.from) <= tol
                                || !(on_object || extend)
                                || !cfg.running(if apparent { mode::APP } else { mode::INT })
                            {
                                continue;
                            }
                            let object = if apparent { "Apparent Intersection" } else { "Intersection" };
                            offer(dd, Track { point: x, paths: vec![*p], object: Some(object) }, &mut best);
                        }
                    }
                }
            }
        }
        if let Some((_, t)) = best {
            return Some(t);
        }
        // 3. Along the nearest path.
        near.iter().filter(|(p, _, dd)| p.slides() && *dd <= tol).min_by(|a, b| a.2.total_cmp(&b.2)).map(|(p, f, _)| Track {
            point: *f,
            paths: vec![*p],
            object: None,
        })
    }
}

/// What resting the cursor at `cursor` would acquire: the object snap `hit` there (with its
/// extension when it is a line or arc endpoint and EXT runs), else with EXT a line or arc
/// endpoint, else with PAR a line (there is a base point), else with APP a line, arc or circle.
pub fn candidate(
    d: &Drawing,
    space: &Space,
    cursor: Vec2,
    hit: Option<&SnapHit>,
    aperture: f64,
    cfg: &TrackConfig,
    base: Option<Vec2>,
) -> Option<Acquired> {
    let nothing_to_acquire = !cfg.otrack && !cfg.running(mode::EXT | mode::PAR | mode::APP);
    if nothing_to_acquire || !(cursor.is_finite() && aperture.is_finite() && aperture > 0.0) {
        return None;
    }
    let ix = crate::spatial::index(d, space);
    let prims = super::gather(d, space, &ix, cursor, aperture, mode::NEA).map(|(_, p)| p).unwrap_or_default();
    let hit = hit.filter(|h| h.deferred.is_none() && h.mode != mode::NEA && h.point.is_finite());
    let ext_end = if cfg.running(mode::EXT) { extension_near(&prims, hit.map_or(cursor, |h| h.point), aperture) } else { None };
    if let Some((at, ext)) = ext_end
        && hit.is_none_or(|h| h.point.dist(at) <= cadcraft_geom::EPS * (1.0 + at.len()))
    {
        return Some(Acquired::Point { at, name: hit.map_or("Endpoint", |h| h.name), ext: Some(ext) });
    }
    if let Some(h) = hit {
        return cfg.otrack.then_some(Acquired::Point { at: h.point, name: h.name, ext: None });
    }
    let (prim, at) = prims
        .iter()
        .filter(|p| super::extendable(p))
        .filter_map(|p| {
            let at = super::prim_segments(p)
                .iter()
                .map(|s| super::fix_circle(s).closest(cursor))
                .min_by(|a, b| a.dist(cursor).total_cmp(&b.dist(cursor)))?;
            (at.is_finite() && at.dist(cursor) <= aperture).then_some((p, at))
        })
        .min_by(|a, b| a.1.dist(cursor).total_cmp(&b.1.dist(cursor)))?;
    let line = match prim {
        Prim::Seg(Segment::Line(l)) => Some((l.b - l.a).normalized()),
        Prim::Infinite { dir, .. } => Some(dir.normalized()),
        _ => None,
    }
    .filter(|d| *d != Vec2::ZERO && d.is_finite());
    if cfg.running(mode::PAR)
        && base.is_some()
        && let Some(dir) = line
    {
        return Some(Acquired::Parallel { at, dir });
    }
    if cfg.running(mode::APP) {
        let shape = match prim {
            Prim::Seg(Segment::Arc { arc, .. }) => PathShape::Circle { center: arc.center, radius: arc.radius, gap: None },
            Prim::Circle(c) => PathShape::Circle { center: c.center, radius: c.radius, gap: None },
            _ => PathShape::Line { origin: at, dir: line? },
        };
        return shape.is_finite().then_some(Acquired::Apparent { at, shape });
    }
    None
}

/// The line or arc endpoint nearest `p` within `aperture` and the extension path from it.
fn extension_near(prims: &[Prim], p: Vec2, aperture: f64) -> Option<(Vec2, PathShape)> {
    let mut best: Option<(f64, Vec2, PathShape)> = None;
    for prim in prims {
        let Prim::Seg(s) = prim else { continue };
        for (end, other) in [(s.start(), s.end()), (s.end(), s.start())] {
            let dd = end.dist(p);
            if !dd.is_finite() || dd > aperture || best.as_ref().is_some_and(|(bd, ..)| *bd <= dd) {
                continue;
            }
            let shape = match s {
                Segment::Line(_) => PathShape::Ray { origin: end, dir: (end - other).normalized() },
                Segment::Arc { arc, .. } => PathShape::Circle { center: arc.center, radius: arc.radius, gap: Some((arc.start, arc.end)) },
            };
            if shape.is_finite() {
                best = Some((dd, end, shape));
            }
        }
    }
    best.map(|(_, at, shape)| (at, shape))
}

/// What one cursor update gives: the point to use and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorSnap {
    pub point: Vec2,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snap: Option<SnapHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<Track>,
    /// An acquisition is pending: update again after [`DWELL`] even if the cursor rests.
    pub wait: bool,
}

/// One cursor update's inputs besides the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorQuery {
    /// The base point (rubber-band origin), if the prompt has one.
    pub base: Option<Vec2>,
    /// The prompt resolves deferred tangent/perpendicular snaps.
    pub deferred: bool,
    /// The object snap aperture in world units (APERTURE pixels at the view scale).
    pub aperture: f64,
    /// How close, in world units, the cursor slides onto a tracking path.
    pub tol: f64,
    /// Seconds on any steady clock (acquisition timing).
    pub now: f64,
    /// Shift is held.
    pub shift: bool,
}

impl crate::Session {
    /// The cursor at a point prompt, snapped: running object snaps first (resting on a snap point
    /// acquires it for tracking), then grid snap, then tracking paths (polar, object snap
    /// tracking, Extension, Parallel, Apparent Intersection), then ortho.
    pub fn snap_cursor(&mut self, raw: Vec2, q: &CursorQuery) -> CursorSnap {
        let s = self.settings.clone();
        let angbase = self.angle_settings().angbase;
        let Ok(st) = self.state() else { return CursorSnap { point: raw, ..Default::default() } };
        let (doc, space) = (st.doc.clone(), st.edit_space());
        if !raw.is_finite() {
            return CursorSnap { point: raw, ..Default::default() };
        }
        let cfg = TrackConfig::new(&s, angbase);
        let osmode = s.osmode | if s.osnaphatch { mode::HATCH } else { 0 };
        let hit = super::osnap(&doc, &space, raw, q.aperture, osmode, q.base, q.deferred);
        let cand = candidate(&doc, &space, raw, hit.as_ref(), q.aperture, &cfg, q.base);
        let wait = self.tracking.hover(cand, q.now, cfg.shift_acquire, q.shift);
        if let Some(h) = hit {
            self.tracking.last = None;
            return CursorSnap { point: h.point, snap: Some(h), track: None, wait };
        }
        let mut p = raw;
        if s.snapmode {
            let (origin, angle) = super::grid_frame(&doc);
            p = super::grid_snap(p, s.snapunit, origin, angle);
        }
        if let Some(t) = self.tracking.track(&doc, &space, p, q.aperture, q.tol, &cfg, q.base) {
            return CursorSnap { point: t.point, snap: None, track: Some(t), wait };
        }
        if let Some(b) = q.base
            && s.orthomode
        {
            p = super::ortho(b, p);
        }
        CursorSnap { point: p, snap: None, track: None, wait }
    }
}
