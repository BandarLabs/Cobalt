//! Resolution-independent icon drawing.
//!
//! # Why this replaces a bitmap grid
//!
//! Icons used to be sixteen-by-sixteen bitmaps scaled by whole cells. Whole
//! cells because a resampled bitmap blurs, and blur is the one thing a panel
//! with sixteen grey levels cannot render convincingly. That was the right
//! trade for a first version and it produced exactly what you would expect: at
//! three pixels per cell a book is a fat rounded blob and a diagonal is a
//! staircase. Photographs of the panel showed it plainly.
//!
//! The scale is the problem, not the artwork. A 300 pixel-per-inch panel draws
//! a 48 pixel icon; the source had sixteen. So the source is now geometry, in a
//! 1000 by 1000 box, rasterised at whatever size the layout asks for.
//!
//! # Why coverage, when thin antialiased strokes are supposed to ghost
//!
//! Because the alternative is worse, and the panel already does this for text.
//! The renderer picks its waveform from the pixels themselves, so an icon with
//! grey edges is driven by the same sixteen-level update that draws the
//! sentence beside it. What ghosts on E Ink is a *two-level* waveform crushing
//! antialiased edges, that is a waveform choice, not an antialiasing one.
//!
//! # Why applications cannot supply paths
//!
//! Nothing here is reachable over the protocol. An application names a
//! [`Glyph`] and the runtime draws it. Arbitrary path data from
//! an application is an untrusted input to a scanline rasteriser and a way for
//! one application to draw something indistinguishable from a system control.
//! A curated set is the same choice the rest of this UI layer makes everywhere.

use crate::{Glyph, Percent, Signal};

mod tabler;

/// The side of the box every icon is designed in.
///
/// A round number well above the pixel size of any panel, so an icon is
/// authored in proportions rather than in pixels and the same definition is
/// crisp on a 212 and a 300 pixel-per-inch screen.
pub const UNITS: i32 = 1000;

/// The stroke weight every icon is drawn at.
///
/// One weight for the whole set: a row of icons at mixed weights looks like a
/// row from different sets.
///
/// One and a half source units in the source's twenty-four unit box, which is
/// the lighter of the two weights the artwork is published at, rather than the
/// two units it is drawn at by default. Measured against the type: at the size
/// a list row draws a glyph, two units puts about twice as much ink in the
/// stroke as there is in a text stem beside it, which is what makes an icon
/// read as clip art next to a sentence. It is not the whole of that fault, and
/// the rest of it cannot be fixed here: the stroke scales with the box, so an
/// icon drawn far larger than the title it leads is heavy however thinly it is
/// drawn. Making the icon the right size is a layout question.
///
/// Not thinner than this, tempting as the arithmetic is. The artwork was
/// designed against a weight, and a corner radius authored for two units looks
/// slack at one.
const WEIGHT: i32 = 62;

/// Sub-pixel precision for edge intersections.
const FIXED: i64 = 256;
/// Sub-scanlines per pixel row. Four is the point where more stops being
/// visible at icon sizes on a sixteen-level panel.
const SUB: i64 = 4;
/// Straight segments per quadratic curve. Twelve keeps the error below a
/// quarter of a pixel at every size this draws at.
const CURVE_STEPS: i32 = 12;

/// One step of an outline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Cmd {
    Move(i32, i32),
    Line(i32, i32),
    /// One control point and an end point.
    Quad(i32, i32, i32, i32),
    /// Two control points and an end point.
    ///
    /// Here because imported artwork is authored in cubics. Nothing in this
    /// file writes one by hand: a shape that needs a curve is easier to reason
    /// about with a single control point.
    Cubic(i32, i32, i32, i32, i32, i32),
    Close,
}

/// An outline in icon units.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Path {
    commands: Vec<Cmd>,
}

impl Path {
    #[must_use]
    fn new() -> Self {
        Self::default()
    }

    #[must_use]
    fn move_to(mut self, x: i32, y: i32) -> Self {
        self.commands.push(Cmd::Move(x, y));
        self
    }

    #[must_use]
    fn line_to(mut self, x: i32, y: i32) -> Self {
        self.commands.push(Cmd::Line(x, y));
        self
    }

    #[must_use]
    fn quad_to(mut self, cx: i32, cy: i32, x: i32, y: i32) -> Self {
        self.commands.push(Cmd::Quad(cx, cy, x, y));
        self
    }

    #[must_use]
    fn close(mut self) -> Self {
        self.commands.push(Cmd::Close);
        self
    }

    /// An outline that was authored somewhere else.
    ///
    /// The generated artwork in [`tabler`] is a flat `static` rather than a
    /// run of builder calls, because a hundred icons written as method chains
    /// is a megabyte of source for no gain: none of it is read by a person and
    /// none of it is edited by hand.
    #[must_use]
    fn from_commands(commands: &[Cmd]) -> Self {
        Self {
            commands: commands.to_vec(),
        }
    }

    /// A circle, as eight quadratic arcs.
    ///
    /// Eight rather than the usual four: four leaves an error of about three
    /// percent of the radius, which at icon sizes is a visibly flat-sided
    /// circle. Eight puts the error below a fifth of a pixel.
    #[must_use]
    fn circle(cx: i32, cy: i32, r: i32) -> Self {
        // The control point of each 45 degree arc sits where the tangents
        // meet, at radius / cos(22.5 degrees).
        let mut path = Self::new();
        let step = std::f64::consts::FRAC_PI_4;
        let control = f64::from(r) / (step / 2.0).cos();
        let at = |angle: f64, radius: f64| {
            (
                f64::from(cx) + radius * angle.cos(),
                f64::from(cy) + radius * angle.sin(),
            )
        };
        let (sx, sy) = at(0.0, f64::from(r));
        path = path.move_to(round(sx), round(sy));
        for index in 0..8 {
            let mid = step.mul_add(f64::from(index), step / 2.0);
            let end = step * f64::from(index + 1);
            let (cx, cy) = at(mid, control);
            let (ex, ey) = at(end, f64::from(r));
            path = path.quad_to(round(cx), round(cy), round(ex), round(ey));
        }
        path.close()
    }

    /// A rectangle with equal corner radii.
    #[must_use]
    fn rounded(x: i32, y: i32, width: i32, height: i32, radius: i32) -> Self {
        let (right, bottom) = (x + width, y + height);
        let r = radius.min(width / 2).min(height / 2).max(0);
        Self::new()
            .move_to(x + r, y)
            .line_to(right - r, y)
            .quad_to(right, y, right, y + r)
            .line_to(right, bottom - r)
            .quad_to(right, bottom, right - r, bottom)
            .line_to(x + r, bottom)
            .quad_to(x, bottom, x, bottom - r)
            .line_to(x, y + r)
            .quad_to(x, y, x + r, y)
            .close()
    }

    #[must_use]
    fn line(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self::new().move_to(x0, y0).line_to(x1, y1)
    }
}

fn round(value: f64) -> i32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        value.round() as i32
    }
}

/// What to do with an outline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Shape {
    Fill(Path),
    /// Centred on the outline, with round caps and joins. Round because an
    /// icon set with mitred corners needs every corner angle checked by eye,
    /// and round never produces a spike.
    Stroke {
        path: Path,
        width: i32,
    },
}

/// A rasterised icon: one alpha value per pixel of a square.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Coverage {
    pub size: i32,
    /// Row-major, `size * size` values, 0 for untouched and 255 for solid.
    pub alpha: Vec<u8>,
}

impl Coverage {
    #[must_use]
    pub fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.size || y >= self.size {
            return 0;
        }
        usize::try_from(y * self.size + x)
            .ok()
            .and_then(|index| self.alpha.get(index).copied())
            .unwrap_or(0)
    }
}

/// An edge in fixed-point pixel space.
#[derive(Clone, Copy)]
struct Edge {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

/// Rasterises `shapes` into a `size` by `size` coverage map.
///
/// Returns an empty map for a non-positive size rather than panicking: a
/// layout can legitimately produce a zero-height row, and an icon is not the
/// place to discover it.
#[must_use]
pub fn render(shapes: &[Shape], size: i32) -> Coverage {
    if size <= 0 {
        return Coverage {
            size: 0,
            alpha: Vec::new(),
        };
    }
    let scale = |value: i32| i64::from(value) * i64::from(size) * FIXED / i64::from(UNITS);
    let mut edges = Vec::new();
    for shape in shapes {
        match shape {
            Shape::Fill(path) => {
                for contour in flatten(path) {
                    push_contour(&mut edges, &contour, &scale);
                }
            }
            Shape::Stroke { path, width } => {
                for contour in flatten(path) {
                    for polygon in outline(&contour, *width) {
                        push_contour(&mut edges, &polygon, &scale);
                    }
                }
            }
        }
    }
    fill(&edges, size)
}

/// Splits a path into contours of straight points, in icon units.
fn flatten(path: &Path) -> Vec<Vec<(i32, i32)>> {
    let mut contours = Vec::new();
    let mut current: Vec<(i32, i32)> = Vec::new();
    let mut cursor = (0, 0);
    for command in &path.commands {
        match *command {
            Cmd::Move(x, y) => {
                if current.len() > 1 {
                    contours.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                cursor = (x, y);
                current.push(cursor);
            }
            Cmd::Line(x, y) => {
                cursor = (x, y);
                current.push(cursor);
            }
            Cmd::Quad(cx, cy, x, y) => {
                let (x0, y0) = cursor;
                for step in 1..=CURVE_STEPS {
                    let t = f64::from(step) / f64::from(CURVE_STEPS);
                    let inverse = 1.0 - t;
                    let bx = inverse.mul_add(
                        inverse.mul_add(f64::from(x0), 2.0 * t * f64::from(cx)),
                        t * t * f64::from(x),
                    );
                    let by = inverse.mul_add(
                        inverse.mul_add(f64::from(y0), 2.0 * t * f64::from(cy)),
                        t * t * f64::from(y),
                    );
                    current.push((round(bx), round(by)));
                }
                cursor = (x, y);
            }
            Cmd::Cubic(ax, ay, bx, by, x, y) => {
                let (x0, y0) = cursor;
                for step in 1..=CURVE_STEPS {
                    let t = f64::from(step) / f64::from(CURVE_STEPS);
                    let inverse = 1.0 - t;
                    let (w0, w1, w2, w3) = (
                        inverse * inverse * inverse,
                        3.0 * inverse * inverse * t,
                        3.0 * inverse * t * t,
                        t * t * t,
                    );
                    let at = |p0: i32, p1: i32, p2: i32, p3: i32| {
                        w3.mul_add(
                            f64::from(p3),
                            w2.mul_add(
                                f64::from(p2),
                                w1.mul_add(f64::from(p1), w0 * f64::from(p0)),
                            ),
                        )
                    };
                    current.push((round(at(x0, ax, bx, x)), round(at(y0, ay, by, y))));
                }
                cursor = (x, y);
            }
            Cmd::Close => {
                if let Some(&first) = current.first() {
                    current.push(first);
                    cursor = first;
                }
                if current.len() > 1 {
                    contours.push(std::mem::take(&mut current));
                    current.push(cursor);
                }
            }
        }
    }
    if current.len() > 1 {
        contours.push(current);
    }
    contours
}

/// Turns one open contour into the polygons that cover its stroke.
///
/// A quadrilateral per segment plus a disc at every joint and both ends. The
/// pieces overlap, which is exactly why the fill rule is non-zero winding: any
/// number of overlapping same-direction polygons is still simply inside.
fn outline(points: &[(i32, i32)], width: i32) -> Vec<Vec<(i32, i32)>> {
    let radius = (width / 2).max(1);
    let mut polygons = Vec::new();
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        let (dx, dy) = (f64::from(x1 - x0), f64::from(y1 - y0));
        let length = dx.hypot(dy);
        if length < 1.0 {
            continue;
        }
        let (nx, ny) = (
            -dy / length * f64::from(radius),
            dx / length * f64::from(radius),
        );
        polygons.push(vec![
            (x0 + round(nx), y0 + round(ny)),
            (x1 + round(nx), y1 + round(ny)),
            (x1 - round(nx), y1 - round(ny)),
            (x0 - round(nx), y0 - round(ny)),
        ]);
    }
    for &(x, y) in points {
        polygons.push(disc(x, y, radius));
    }
    polygons
}

/// A twelve-sided approximation of a disc, used for caps and joins.
fn disc(cx: i32, cy: i32, radius: i32) -> Vec<(i32, i32)> {
    (0..12)
        .map(|index| {
            let angle = std::f64::consts::TAU * f64::from(index) / 12.0;
            (
                cx + round(f64::from(radius) * angle.cos()),
                cy + round(f64::from(radius) * angle.sin()),
            )
        })
        .collect()
}

/// Adds one closed contour's edges, in a consistent winding direction.
///
/// The direction is normalised because the stroke pieces are generated from
/// segment directions and discs independently: two overlapping polygons wound
/// oppositely would cancel under the non-zero rule and punch a hole in the
/// stroke exactly at a corner.
fn push_contour(edges: &mut Vec<Edge>, points: &[(i32, i32)], scale: &impl Fn(i32) -> i64) {
    if points.len() < 2 {
        return;
    }
    let mut points = points.to_vec();
    if points.first() != points.last() {
        if let Some(&first) = points.first() {
            points.push(first);
        }
    }
    let area: i64 = points
        .windows(2)
        .map(|pair| {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            i64::from(x0) * i64::from(y1) - i64::from(x1) * i64::from(y0)
        })
        .sum();
    if area < 0 {
        points.reverse();
    }
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if y0 == y1 {
            continue;
        }
        edges.push(Edge {
            x0: scale(x0),
            y0: scale(y0),
            x1: scale(x1),
            y1: scale(y1),
        });
    }
}

/// Scanline fill with non-zero winding and vertical supersampling.
fn fill(edges: &[Edge], size: i32) -> Coverage {
    let width = usize::try_from(size).unwrap_or(0);
    let mut alpha = vec![0_u8; width.saturating_mul(width)];
    // A whole pixel's worth of coverage, so the accumulator can be divided
    // once at the end rather than rounded on every span.
    let full = FIXED * SUB;
    let mut accumulator = vec![0_i64; width];
    let mut crossings: Vec<(i64, i32)> = Vec::new();
    for row in 0..size {
        accumulator.fill(0);
        for sub in 0..SUB {
            let y = i64::from(row) * FIXED + (sub * FIXED * 2 + FIXED) / (SUB * 2);
            crossings.clear();
            for edge in edges {
                let (top, bottom) = (edge.y0.min(edge.y1), edge.y0.max(edge.y1));
                if y < top || y >= bottom {
                    continue;
                }
                let x = edge.x0 + (edge.x1 - edge.x0) * (y - edge.y0) / (edge.y1 - edge.y0);
                crossings.push((x, if edge.y1 > edge.y0 { 1 } else { -1 }));
            }
            crossings.sort_unstable();
            let mut winding = 0;
            let mut span_start = 0_i64;
            for &(x, direction) in &crossings {
                if winding == 0 {
                    span_start = x;
                }
                winding += direction;
                if winding == 0 {
                    add_span(&mut accumulator, span_start, x);
                }
            }
        }
        for (column, value) in accumulator.iter().enumerate() {
            let coverage = (value * 255 / full).clamp(0, 255);
            let index = usize::try_from(row).unwrap_or(0) * width + column;
            if let Some(pixel) = alpha.get_mut(index) {
                *pixel = u8::try_from(coverage).unwrap_or(255);
            }
        }
    }
    Coverage { size, alpha }
}

/// Adds one inside span, in fixed-point x, to a row of accumulators.
fn add_span(accumulator: &mut [i64], from: i64, to: i64) {
    if to <= from {
        return;
    }
    let last = i64::try_from(accumulator.len()).unwrap_or(0);
    let first_pixel = (from / FIXED).max(0);
    let last_pixel = ((to - 1) / FIXED).min(last - 1);
    let mut pixel = first_pixel;
    while pixel <= last_pixel {
        let left = (pixel * FIXED).max(from);
        let right = ((pixel + 1) * FIXED).min(to);
        if right > left {
            if let Some(value) = usize::try_from(pixel)
                .ok()
                .and_then(|index| accumulator.get_mut(index))
            {
                *value += right - left;
            }
        }
        pixel += 1;
    }
}

/// The geometry of one glyph.
///
/// Line art at a single stroke weight rather than filled silhouettes, because
/// a page of solid black icons on a reflective panel reads as heavier than the
/// text it sits beside, and the icon is subordinate to the label next to it.
///
/// The artwork is Tabler Icons, imported into [`tabler`] rather than drawn
/// here. Forty hand-authored icons were forty separate judgements about how
/// round a corner runs and how long a tail is, and a set drawn that way looks
/// like a set only from across the room. A published set is one designer's
/// judgement applied five thousand times, and it costs nothing to adopt: the
/// source is stroked outlines at a single weight with round caps and joins in
/// a square box, which is exactly what this rasteriser already draws.
#[must_use]
pub fn shapes(glyph: Glyph) -> Vec<Shape> {
    tabler::outline(glyph)
        .iter()
        .map(|commands| Shape::Stroke {
            path: Path::from_commands(commands),
            width: WEIGHT,
        })
        .collect()
}

/// The back control, drawn rather than typed.
///
/// The built-in typeface has no arrow, and an application must never be able
/// to substitute this control by writing a character that looks like one. It
/// lives here, in the same geometry as the icons, because the version drawn
/// from rectangles was the fattest thing on the screen.
///
/// The geometry is Tabler's own `chevron-left`, transcribed rather than
/// imported because it is not a [`Glyph`] and an application must not be able
/// to ask for it. It used to be taller than that and drawn half again as
/// heavy, which made the one control guaranteed to be on every screen the odd
/// one out in its own bar: the back mark read as different chrome from the
/// refresh mark sitting a centimetre away from it.
#[must_use]
pub fn back_arrow() -> Vec<Shape> {
    vec![Shape::Stroke {
        path: Path::new()
            .move_to(625, 250)
            .line_to(375, 500)
            .line_to(625, 750),
        width: WEIGHT,
    }]
}

/// A battery showing how much is left, rather than a battery.
///
/// The icon in [`shapes`] is a symbol: it means "battery" and says nothing
/// about charge. A status band that showed it would tell a reader the device
/// has a battery, which they know. The fill is proportional and the whole mark
/// is drawn wider than tall so it reads at status-band size, where the square
/// design box would leave it about three millimetres across.
///
/// `percent` is clamped by [`Percent`]. `charging` replaces the fill with a
/// bolt, because a charging battery's level is the least interesting thing
/// about it and an animated fill is not something this panel can afford.
#[must_use]
pub fn battery(percent: Percent, charging: bool) -> Vec<Shape> {
    const W: i32 = 60;
    // The shell, leaving room on the right for the terminal nub.
    let (x, y, width, height) = (60, 300, 780, 400);
    let mut shapes = vec![
        Shape::Stroke {
            path: Path::rounded(x, y, width, height, 60),
            width: W,
        },
        Shape::Fill(Path::rounded(x + width + 40, 430, 60, 140, 30)),
    ];
    if charging {
        // A bolt across the shell. Drawn as a fill so it survives being
        // rasterised at eight pixels tall, where a stroked one closes up.
        shapes.push(Shape::Fill(
            Path::new()
                .move_to(520, 340)
                .line_to(300, 530)
                .line_to(430, 530)
                .line_to(380, 660)
                .line_to(600, 470)
                .line_to(470, 470)
                .close(),
        ));
        return shapes;
    }
    // Inset by the stroke and one gap, so the fill never touches the shell:
    // at this size a fill that meets the outline reads as a solid black brick.
    let inset = W + 40;
    let room = width - 2 * inset;
    let filled = room * i32::from(percent.get()) / 100;
    if filled > 0 {
        shapes.push(Shape::Fill(Path::rounded(
            x + inset,
            y + inset,
            filled,
            height - 2 * inset,
            20,
        )));
    }
    shapes
}

/// The radio, drawn at the strength it is actually running at.
///
/// Arcs are added from the bottom up, so a weak signal is a dot and one arc
/// and a strong one is a dot and three. An unlit arc is left out rather than
/// drawn faintly: this panel has no colour to spare and a ghosted arc at eight
/// pixels is indistinguishable from a lit one.
///
/// The geometry is placed so that the *full* three-arc mark is centred in the
/// design box, which is what puts it on the same line as the Bluetooth mark
/// beside it. Centring each strength on its own would be worse: the dot would
/// then jump up the strip every time the signal dropped an arc, and a status
/// icon that moves when the news changes is harder to read than one that is
/// slightly light at the top.
#[must_use]
pub fn wifi(strength: Signal) -> Vec<Shape> {
    const W: i32 = 70;
    let stroke = |path: Path| Shape::Stroke { path, width: W };
    if strength == Signal::Off {
        // The mark for "no radio" has to be different in shape, not just in
        // quantity, or it reads as a weak signal. A struck-through dot is
        // unambiguous and stays legible when it is eight pixels tall.
        return vec![
            Shape::Fill(Path::circle(500, 760, 90)),
            stroke(Path::line(180, 180, 820, 820)),
        ];
    }
    let mut shapes = vec![Shape::Fill(Path::circle(500, 780, 60))];
    let arcs = [
        Path::new().move_to(400, 660).quad_to(500, 580, 600, 660),
        Path::new().move_to(270, 510).quad_to(500, 330, 730, 510),
        Path::new().move_to(120, 350).quad_to(500, 50, 880, 350),
    ];
    let lit = match strength {
        Signal::Off => 0,
        Signal::Weak => 1,
        Signal::Fair => 2,
        Signal::Strong => 3,
    };
    for arc in arcs.into_iter().take(lit) {
        shapes.push(stroke(arc));
    }
    shapes
}

/// The Bluetooth rune, drawn only when something is actually connected.
///
/// One continuous stroke: a vertical spine with two crossing arms that meet it
/// at the quarter points. Drawn as a single path rather than a spine plus four
/// arms so that the joins stay closed when it is rasterised at eight pixels,
/// where four separate strokes come apart into a smudge.
///
/// There is no "on but not connected" mark. The reason to look at this strip
/// is to know where the sound is about to come out, and a controller that is
/// powered with nothing paired answers that question the same way as one that
/// is switched off.
#[must_use]
pub fn bluetooth() -> Vec<Shape> {
    const W: i32 = 70;
    vec![Shape::Stroke {
        path: Path::new()
            .move_to(330, 330)
            .line_to(670, 670)
            .line_to(500, 840)
            .line_to(500, 160)
            .line_to(670, 330)
            .line_to(330, 670),
        width: W,
    }]
}

#[cfg(test)]
mod tests {
    use super::{back_arrow, render, shapes, Path, Shape, UNITS};
    use crate::{Glyph, Signal};

    const EVERY: [Glyph; Glyph::ALL.len()] = Glyph::ALL;

    fn inked(glyph: Glyph, size: i32) -> usize {
        render(&shapes(glyph), size)
            .alpha
            .iter()
            .filter(|&&value| value > 0)
            .count()
    }

    /// The vertical extent of the ink, in design units.
    fn ink_band(shapes: &[Shape]) -> (i32, i32) {
        const SIZE: i32 = 200;
        let coverage = render(shapes, SIZE);
        let (mut top, mut bottom) = (i32::MAX, i32::MIN);
        for row in 0..SIZE {
            for column in 0..SIZE {
                if coverage.at(column, row) > 0 {
                    top = top.min(row);
                    bottom = bottom.max(row);
                }
            }
        }
        let to_units = |value: i32| value * UNITS / SIZE;
        (to_units(top), to_units(bottom))
    }

    #[test]
    fn the_status_marks_sit_on_one_line() {
        // These are drawn side by side in a strip eight pixels tall, where a
        // mark hung fifty units low out of a thousand is plainly crooked to
        // the eye but invisible to a test that only asks whether the glyph
        // drew anything. The wifi mark was exactly that for a long time: its
        // dot was pinned to the bottom of the box and the arcs grew upward
        // from there, so the whole mark sat low beside the Bluetooth rune.
        let marks = [
            ("bluetooth", super::bluetooth()),
            ("wifi", super::wifi(Signal::Strong)),
            ("wifi off", super::wifi(Signal::Off)),
        ];
        for (name, shapes) in marks {
            let (top, bottom) = ink_band(&shapes);
            let centre = (top + bottom) / 2;
            let drift = (centre - UNITS / 2).abs();
            assert!(
                drift <= 20,
                "the {name} mark centres at {centre} rather than {}, which reads as crooked \
                 beside the marks either side of it",
                UNITS / 2
            );
        }
    }

    #[test]
    fn every_glyph_draws_something_at_every_size_a_panel_asks_for() {
        // The sizes the row and tile layouts produce across the supported
        // panels. An icon that vanishes at one of them is a blank space beside
        // a label, which is exactly the failure a bitmap grid used to hide by
        // always drawing *something*.
        for glyph in EVERY {
            for size in [24, 32, 40, 48, 56, 64, 96] {
                assert!(inked(glyph, size) > 0, "{glyph:?} vanished at {size}");
            }
        }
    }

    #[test]
    fn nothing_is_drawn_outside_the_box() {
        // Coverage is indexed by the caller against the rect it asked for, so
        // an icon whose geometry left the unit box would silently overdraw the
        // text beside it.
        for glyph in EVERY {
            let coverage = render(&shapes(glyph), 64);
            assert_eq!(coverage.alpha.len(), 64 * 64, "{glyph:?}");
            for edge in 0..64 {
                assert_eq!(coverage.at(-1, edge), 0);
                assert_eq!(coverage.at(64, edge), 0);
            }
        }
    }

    #[test]
    fn an_icon_grows_with_the_box_rather_than_staying_the_same_size() {
        // The whole point. A bitmap grid scaled by whole cells drew the same
        // number of inked cells at 33 pixels as at 48.
        for glyph in EVERY {
            let small = inked(glyph, 32);
            let large = inked(glyph, 64);
            assert!(large > small * 2, "{glyph:?}: {small} then {large}");
        }
    }

    #[test]
    fn edges_are_grey_rather_than_stepped() {
        // If every value is 0 or 255 the rasteriser is not antialiasing at
        // all, and diagonals will stair-step exactly as the bitmaps did.
        let coverage = render(&shapes(Glyph::Search), 64);
        let partial = coverage
            .alpha
            .iter()
            .filter(|&&value| value > 0 && value < 255)
            .count();
        assert!(partial > 40, "only {partial} grey pixels");
    }

    #[test]
    fn a_stroke_corner_is_solid_rather_than_holed() {
        // The failure this covers: the quadrilateral for a segment and the
        // disc at its joint are generated independently, and if they are wound
        // oppositely the non-zero rule cancels them and punches a hole at every
        // corner.
        let path = Path::new()
            .move_to(200, 200)
            .line_to(800, 200)
            .line_to(800, 800);
        let coverage = render(&[Shape::Stroke { path, width: 120 }], 100);
        assert_eq!(coverage.at(80, 20), 255, "the corner has a hole in it");
    }

    #[test]
    fn the_back_arrow_is_cut_like_every_other_mark() {
        // First it was drawn from rectangles and was the heaviest thing on the
        // panel. Then it was a hand-cut stroke half again the weight the icon
        // set uses, which made the one control guaranteed to be on every
        // screen the odd one out in its own bar.
        let shapes = back_arrow();
        let Some(Shape::Stroke { width, .. }) = shapes.first() else {
            panic!("the back mark is a stroke");
        };
        assert_eq!(
            *width,
            super::WEIGHT,
            "the back mark is cut to its own weight"
        );
        let coverage = render(&shapes, 60);
        let inked = coverage.alpha.iter().filter(|&&value| value > 0).count();
        assert!(inked * 3 < 60 * 60, "the arrow still fills {inked} pixels");
        assert!(inked > 100, "the arrow is too faint at {inked} pixels");
    }

    #[test]
    fn a_zero_sized_box_is_empty_rather_than_a_panic() {
        assert!(render(&shapes(Glyph::App), 0).alpha.is_empty());
        assert!(render(&shapes(Glyph::App), -5).alpha.is_empty());
    }

    #[test]
    fn the_design_box_is_the_one_the_geometry_uses() {
        // A guard on the constant: every icon above is authored against it.
        assert_eq!(UNITS, 1000);
    }
}

#[cfg(test)]
mod sheet {
    use crate::Glyph;

    /// Draws every glyph onto one sheet, for looking at.
    ///
    /// Not an assertion. Icon artwork is judged by eye and nothing else, and
    /// the alternative to this is deploying to a reader and photographing it,
    /// which is a slow way to find out that a corner radius is wrong. Run it
    /// with `cargo test -p kobo-ui contact_sheet -- --ignored` and open the
    /// file it names.
    #[test]
    #[ignore = "writes a sheet to look at rather than asserting anything"]
    fn contact_sheet() {
        const CELL: i32 = 96;
        const COLUMNS: usize = 8;
        let rows = Glyph::ALL.len().div_ceil(COLUMNS);
        let width = CELL * i32::try_from(COLUMNS).unwrap();
        let height = CELL * i32::try_from(rows).unwrap();
        let mut pixels = vec![255u8; usize::try_from(width * height).unwrap()];
        for (index, glyph) in Glyph::ALL.iter().enumerate() {
            let coverage = super::render(&super::shapes(*glyph), CELL - 16);
            let column = i32::try_from(index % COLUMNS).unwrap();
            let row = i32::try_from(index / COLUMNS).unwrap();
            for y in 0..coverage.size {
                for x in 0..coverage.size {
                    let at = (row * CELL + 8 + y) * width + column * CELL + 8 + x;
                    let at = usize::try_from(at).unwrap();
                    pixels[at] = pixels[at].saturating_sub(coverage.at(x, y));
                }
            }
        }
        let path = std::env::temp_dir().join("cobalt-glyph-sheet.pgm");
        let mut out = format!("P5\n{width} {height}\n255\n").into_bytes();
        out.extend_from_slice(&pixels);
        std::fs::write(&path, out).unwrap();
        println!("{} glyphs written to {}", Glyph::ALL.len(), path.display());
    }
}
