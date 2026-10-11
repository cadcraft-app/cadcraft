//! Hit testing and window/crossing selection.

use cadcraft_doc::{Drawing, Entity, EntityKind, Handle, Prim, Space, entity_bounds};
use cadcraft_geom::{Bounds2, Line, Vec2};

use crate::spatial::{self, SpatialIndex};

/// Polylines approximating an entity for hit testing, tessellated at `tol`.
pub fn hit_polylines(d: &Drawing, e: &Entity, tol: f64) -> Vec<Vec<Vec2>> {
    match &e.kind {
        EntityKind::Text(_)
        | EntityKind::MText(_)
        | EntityKind::Insert(_)
        | EntityKind::Dimension(_)
        | EntityKind::AttDef(_)
        | EntityKind::Table(_)
        | EntityKind::MLeader(_)
        | EntityKind::Image(_) => {
            // Text-like objects are hit inside their box.
            let b = entity_bounds(d, e, 0);
            if b.is_empty() {
                return Vec::new();
            }
            let c = b.corners();
            let mut v = vec![vec![c[0], c[1], c[2], c[3], c[0]]];
            if matches!(e.kind, EntityKind::Dimension(_) | EntityKind::Insert(_) | EntityKind::MLeader(_)) {
                let list = cadcraft_render::build_entities(
                    d,
                    std::iter::once(e),
                    &cadcraft_render::Options { tolerance: tol, text: false, ..Default::default() },
                );
                v = list.prims.iter().map(|p| list.points(p).to_vec()).collect();
                v.push(vec![c[0], c[1], c[2], c[3], c[0]]);
            }
            v
        }
        kind => {
            let mut out = Vec::new();
            for p in kind.prims() {
                let mut pts = Vec::new();
                match p {
                    Prim::Seg(s) => s.tessellate(tol, &mut pts),
                    Prim::Circle(c) => c.tessellate(tol, &mut pts),
                    Prim::Ellipse(el) => el.tessellate(tol, &mut pts),
                    Prim::Spline(s) => pts = s.tessellate(tol),
                    Prim::Point(p) => pts = vec![p, p],
                    Prim::Infinite { base, dir, ray } => {
                        let far = 1e9;
                        pts = vec![if ray { base } else { base - dir * far }, base + dir * far];
                    }
                    Prim::Fill(f) => {
                        pts = f.clone();
                        if let Some(first) = f.first() {
                            pts.push(*first);
                        }
                    }
                }
                out.push(pts);
            }
            out
        }
    }
}

fn dist_to_polyline(pts: &[Vec2], p: Vec2) -> f64 {
    if pts.len() == 1 {
        return pts.first().map(|q| q.dist(p)).unwrap_or(f64::INFINITY);
    }
    pts.windows(2).filter_map(|w| Some(Line::new(*w.first()?, *w.get(1)?).dist(p))).fold(f64::INFINITY, f64::min)
}

/// Distance from `p` to the entity (0 inside text boxes and solid fills).
pub fn entity_distance(d: &Drawing, e: &Entity, p: Vec2, tol: f64) -> f64 {
    let polys = hit_polylines(d, e, tol);
    let mut best = polys.iter().map(|pl| dist_to_polyline(pl, p)).fold(f64::INFINITY, f64::min);
    let filled = matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_) | EntityKind::AttDef(_) | EntityKind::Solid(_) | EntityKind::Table(_))
        || matches!(&e.kind, EntityKind::Hatch(h) if h.solid);
    if filled && entity_bounds(d, e, 0).contains(p) {
        best = 0.0;
    }
    best
}

fn selectable(d: &Drawing, e: &Entity) -> bool {
    d.is_visible(e)
}

/// Entities to test, in the order to test them, each with its bounds when already known.
pub(crate) type Cands<'a> = Box<dyn Iterator<Item = (&'a Entity, Option<Bounds2>)> + 'a>;

pub(crate) fn bounds_of(d: &Drawing, e: &Entity, known: Option<Bounds2>) -> Bounds2 {
    known.unwrap_or_else(|| entity_bounds(d, e, 0))
}

pub(crate) fn is_infinite(e: &Entity) -> bool {
    matches!(e.kind, EntityKind::XLine(_) | EntityKind::Ray(_))
}

/// Candidates from the spatial index touching `query` (draw order, reversed when `rev`), or every
/// entity of the space when it is too small to index (or the query box isn't finite).
pub(crate) fn candidates<'a>(
    d: &'a Drawing,
    space: &Space,
    ix: &'a Option<std::sync::Arc<SpatialIndex>>,
    query: &Bounds2,
    infinite: bool,
    rev: bool,
) -> Option<Cands<'a>> {
    if let Some(ix) = ix
        && let Some(c) = ix.query(query, infinite)
    {
        let it = move |i: u32| ix.entity(i).map(|e| (e.as_ref(), Some(ix.bounds(i))));
        return Some(if rev { Box::new(c.into_iter().rev().filter_map(it)) } else { Box::new(c.into_iter().filter_map(it)) });
    }
    let store = d.space(space)?;
    if rev {
        let ents: Vec<&Entity> = store.iter().map(|e| e.as_ref()).collect();
        Some(Box::new(ents.into_iter().rev().map(|e| (e, None))))
    } else {
        Some(Box::new(store.iter().map(|e| (e.as_ref(), None))))
    }
}

/// The topmost entity within `aperture` of `p`.
pub fn pick(d: &Drawing, space: &Space, p: Vec2, aperture: f64) -> Option<Handle> {
    let ix = spatial::index(d, space);
    pick_with(d, space, &ix, p, aperture)
}

pub(crate) fn pick_with(d: &Drawing, space: &Space, ix: &Option<std::sync::Arc<SpatialIndex>>, p: Vec2, aperture: f64) -> Option<Handle> {
    let probe = Bounds2::new(p, p).expand(aperture);
    let mut best: Option<(f64, Handle)> = None;
    for (e, known) in candidates(d, space, ix, &probe.expand(aperture), true, true)? {
        if !selectable(d, e) {
            continue;
        }
        let infinite = is_infinite(e);
        if !infinite && !bounds_of(d, e, known).expand(aperture).intersects(&probe) {
            continue;
        }
        let dist = entity_distance(d, e, p, aperture / 4.0);
        if dist <= aperture && best.is_none_or(|(bd, _)| dist < bd - 1e-12) {
            best = Some((dist, e.handle));
            if dist == 0.0 {
                break;
            }
        }
    }
    best.map(|(_, h)| h)
}

fn seg_hits_box(a: Vec2, b: Vec2, bx: &Bounds2) -> bool {
    if bx.contains(a) || bx.contains(b) {
        return true;
    }
    let c = bx.corners();
    (0..4).any(|i| cadcraft_geom::line_line(&Line::new(a, b), &Line::new(c[i], c[(i + 1) % 4])).is_some())
}

/// Window selection (entirely inside) or crossing selection (inside or touching).
pub fn select_window(d: &Drawing, space: &Space, bx: Bounds2, crossing: bool) -> Vec<Handle> {
    let ix = spatial::index(d, space);
    select_window_with(d, space, &ix, bx, crossing)
}

pub(crate) fn select_window_with(d: &Drawing, space: &Space, ix: &Option<std::sync::Arc<SpatialIndex>>, bx: Bounds2, crossing: bool) -> Vec<Handle> {
    let Some(cands) = candidates(d, space, ix, &bx, crossing, false) else { return Vec::new() };
    let tol = window_tol(&bx);
    cands.filter(|(e, known)| selectable(d, e) && in_window(d, e, *known, &bx, crossing, tol)).map(|(e, _)| e.handle).collect()
}

/// The tessellation tolerance window and crossing tests use for a box.
pub(crate) fn window_tol(bx: &Bounds2) -> f64 {
    (bx.width() + bx.height()).max(1e-9) / 2000.0
}

/// Whether a window box (`crossing` false: entirely inside) or a crossing box (inside or
/// touching its geometry, not just its bounding box) selects `e`.
pub(crate) fn in_window(d: &Drawing, e: &Entity, known: Option<Bounds2>, bx: &Bounds2, crossing: bool, tol: f64) -> bool {
    let infinite = is_infinite(e);
    let eb = bounds_of(d, e, known);
    if !infinite && bx.contains_box(&eb) {
        return true;
    }
    if !crossing || (!infinite && !eb.intersects(bx)) {
        return false;
    }
    let polys = hit_polylines(d, e, tol);
    let hit = polys.iter().any(|pl| {
        if pl.len() == 1 {
            pl.first().is_some_and(|p| bx.contains(*p))
        } else {
            pl.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| seg_hits_box(*a, *b, bx)))
        }
    });
    // A crossing box entirely inside a text/block bounding rectangle also selects it.
    if hit || (eb.contains_box(bx) && matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_) | EntityKind::Insert(_))) {
        return true;
    }
    // SOLID/TRACE are filled polygons. A crossing window wholly within the
    // fill must select them even when it does not touch their outline. Check
    // the polygon, not just its bounding rectangle (which can cover empty
    // space around triangular solids).
    if matches!(e.kind, EntityKind::Solid(_) | EntityKind::Trace(_)) && eb.contains_box(bx) {
        return e.kind.prims().iter().any(|prim| match prim {
            Prim::Fill(poly) => bx.corners().iter().all(|p| cadcraft_geom::point_in_polygon(poly, *p)),
            _ => false,
        });
    }
    false
}

/// Fence selection: entities crossed by the fence polyline.
pub fn select_fence(d: &Drawing, space: &Space, fence: &[Vec2]) -> Vec<Handle> {
    let ix = spatial::index(d, space);
    select_fence_with(d, space, &ix, fence)
}

pub(crate) fn select_fence_with(d: &Drawing, space: &Space, ix: &Option<std::sync::Arc<SpatialIndex>>, fence: &[Vec2]) -> Vec<Handle> {
    let fb = Bounds2::from_points(fence.iter().copied());
    let tol = (fb.width() + fb.height()).max(1e-9) / 2000.0;
    let probe = fb.expand(tol);
    let Some(cands) = candidates(d, space, ix, &probe, true, false) else { return Vec::new() };
    let mut out = Vec::new();
    for (e, known) in cands {
        // Infinite objects have no useful bounds; like window selection, test their geometry directly.
        if !selectable(d, e) || (!is_infinite(e) && !bounds_of(d, e, known).intersects(&probe)) {
            continue;
        }
        let polys = hit_polylines(d, e, tol);
        let hit = fence.windows(2).any(|f| {
            let (Some(a), Some(b)) = (f.first(), f.get(1)) else { return false };
            let fl = Line::new(*a, *b);
            polys.iter().any(|pl| {
                pl.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(p, q)| cadcraft_geom::line_line(&fl, &Line::new(*p, *q)).is_some()))
            })
        });
        if hit {
            out.push(e.handle);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_doc::{Common, RayLine};
    use cadcraft_geom::Vec3;

    fn fence_hits(kind: EntityKind, fence: [(f64, f64); 2]) -> usize {
        let mut d = Drawing::new_metric();
        d.add(&Space::Model, Common::default(), kind).unwrap();
        let f = [Vec2::new(fence[0].0, fence[0].1), Vec2::new(fence[1].0, fence[1].1)];
        select_fence(&d, &Space::Model, &f).len()
    }

    #[test]
    fn fence_selects_crossed_xline_and_ray() {
        let line = RayLine { base: Vec3::new(0.0, 0.0, 0.0), dir: Vec3::new(1.0, 0.0, 0.0) };
        for x in [-5.0, 0.0, 5.0] {
            assert_eq!(fence_hits(EntityKind::XLine(line.clone()), [(x, -5.0), (x, 5.0)]), 1, "xline at x={x}");
        }
        assert_eq!(fence_hits(EntityKind::Ray(line.clone()), [(5.0, -5.0), (5.0, 5.0)]), 1);
        assert_eq!(fence_hits(EntityKind::Ray(line.clone()), [(0.0, -5.0), (0.0, 5.0)]), 1);
        assert_eq!(fence_hits(EntityKind::Ray(line.clone()), [(-5.0, -5.0), (-5.0, 5.0)]), 0);
        assert_eq!(fence_hits(EntityKind::XLine(line.clone()), [(0.0, 1.0), (0.0, 5.0)]), 0);
        assert_eq!(fence_hits(EntityKind::Ray(line), [(5.0, 1.0), (5.0, 5.0)]), 0);
    }
}

#[cfg(test)]
mod solid_interior_tests {
    use super::*;

    fn face(kind: bool, triangular: bool) -> Entity {
        let a = Vec2::new(0.0, 0.0).to3(0.0);
        let b = Vec2::new(10.0, 0.0).to3(0.0);
        let c = Vec2::new(0.0, 10.0).to3(0.0);
        let d = if triangular { c } else { Vec2::new(10.0, 10.0).to3(0.0) };
        let solid = cadcraft_doc::Solid { corners: [a, b, c, d] };
        Entity::new(Handle(256), if kind { EntityKind::Solid(solid) } else { EntityKind::Trace(solid) })
    }

    #[test]
    fn crossing_window_inside_solid_and_trace_selects_fill() {
        let inside = Bounds2::new(Vec2::new(4.0, 4.0), Vec2::new(6.0, 6.0));
        for solid in [true, false] {
            let mut d = Drawing::default();
            let e = face(solid, false);
            assert!(in_window(&d, &e, None, &inside, true, 0.01));
            assert!(!in_window(&d, &e, None, &inside, false, 0.01));
            d.model.push(e);
            assert_eq!(select_window(&d, &Space::Model, inside, true), vec![Handle(256)]);
            assert!(select_window(&d, &Space::Model, inside, false).is_empty());
        }
    }

    #[test]
    fn crossing_window_in_triangular_bbox_but_outside_fill_does_not_select() {
        let d = Drawing::default();
        let e = face(true, true);
        let empty = Bounds2::new(Vec2::new(7.0, 7.0), Vec2::new(8.0, 8.0));
        assert!(!in_window(&d, &e, None, &empty, true, 0.01));
        let inside = Bounds2::new(Vec2::new(3.0, 3.0), Vec2::new(4.0, 4.0));
        assert!(in_window(&d, &e, None, &inside, true, 0.01));
    }
}
