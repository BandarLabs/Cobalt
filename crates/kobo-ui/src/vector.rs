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

/// The side of the box every icon is designed in.
///
/// A round number well above the pixel size of any panel, so an icon is
/// authored in proportions rather than in pixels and the same definition is
/// crisp on a 212 and a 300 pixel-per-inch screen.
pub const UNITS: i32 = 1000;

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
/// Designed as line art at a single stroke weight rather than as filled
/// silhouettes, because a page of solid black icons on a reflective panel
/// reads as heavier than the text it sits beside, and the icon is subordinate
/// to the label next to it.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn shapes(glyph: Glyph) -> Vec<Shape> {
    /// One weight for every icon. Anything else and a row of them looks like a
    /// row from different sets.
    const W: i32 = 70;
    let stroke = |path: Path| Shape::Stroke { path, width: W };
    match glyph {
        Glyph::App => vec![
            stroke(Path::rounded(150, 150, 300, 300, 60)),
            stroke(Path::rounded(550, 150, 300, 300, 60)),
            stroke(Path::rounded(150, 550, 300, 300, 60)),
            stroke(Path::rounded(550, 550, 300, 300, 60)),
        ],
        // A book seen from above, open: two leaves meeting at a spine.
        Glyph::Book => vec![
            stroke(
                Path::new()
                    .move_to(500, 260)
                    .quad_to(340, 160, 130, 190)
                    .line_to(130, 780)
                    .quad_to(340, 750, 500, 850),
            ),
            stroke(
                Path::new()
                    .move_to(500, 260)
                    .quad_to(660, 160, 870, 190)
                    .line_to(870, 780)
                    .quad_to(660, 750, 500, 850),
            ),
            stroke(Path::line(500, 260, 500, 850)),
        ],
        // A page with a turned corner.
        Glyph::Note => vec![
            stroke(
                Path::new()
                    .move_to(600, 130)
                    .line_to(230, 130)
                    .line_to(230, 870)
                    .line_to(770, 870)
                    .line_to(770, 300)
                    .close(),
            ),
            stroke(
                Path::new()
                    .move_to(600, 130)
                    .line_to(600, 300)
                    .line_to(770, 300),
            ),
        ],
        Glyph::Clock => vec![
            stroke(Path::circle(500, 500, 350)),
            stroke(
                Path::new()
                    .move_to(500, 290)
                    .line_to(500, 510)
                    .line_to(680, 590),
            ),
        ],
        // A dial: a ring with the marks around it, rather than a gear, whose
        // teeth disappear at small sizes.
        Glyph::Settings => {
            let mut shapes = vec![stroke(Path::circle(500, 500, 220))];
            for index in 0..8 {
                let angle = std::f64::consts::TAU * f64::from(index) / 8.0;
                let point = |radius: f64| {
                    (
                        500 + round(radius * angle.cos()),
                        500 + round(radius * angle.sin()),
                    )
                };
                let (x0, y0) = point(310.0);
                let (x1, y1) = point(410.0);
                shapes.push(stroke(Path::line(x0, y0, x1, y1)));
            }
            shapes
        }
        Glyph::Folder => vec![stroke(
            Path::new()
                .move_to(120, 300)
                .line_to(120, 800)
                .line_to(880, 800)
                .line_to(880, 380)
                .line_to(480, 380)
                .line_to(390, 250)
                .line_to(120, 250)
                .close(),
        )],
        Glyph::Chart => vec![
            stroke(
                Path::new()
                    .move_to(150, 130)
                    .line_to(150, 850)
                    .line_to(870, 850),
            ),
            stroke(Path::line(320, 850, 320, 600)),
            stroke(Path::line(520, 850, 520, 400)),
            stroke(Path::line(720, 850, 720, 230)),
        ],
        Glyph::Search => vec![
            stroke(Path::circle(430, 430, 260)),
            stroke(Path::line(620, 620, 850, 850)),
        ],
        // Three arcs and a dot.
        Glyph::Wifi => vec![
            stroke(Path::new().move_to(120, 400).quad_to(500, 100, 880, 400)),
            stroke(Path::new().move_to(270, 560).quad_to(500, 380, 730, 560)),
            stroke(Path::new().move_to(400, 710).quad_to(500, 630, 600, 710)),
            Shape::Fill(Path::circle(500, 830, 60)),
        ],
        Glyph::Battery => vec![
            stroke(Path::rounded(120, 330, 680, 340, 60)),
            Shape::Fill(Path::rounded(830, 430, 70, 140, 30)),
        ],
        // A page of text: what "the reader" means, without being a book again.
        Glyph::Reader => vec![
            stroke(Path::rounded(180, 140, 640, 720, 60)),
            stroke(Path::line(320, 330, 680, 330)),
            stroke(Path::line(320, 500, 680, 500)),
            stroke(Path::line(320, 670, 540, 670)),
        ],
        Glyph::Power => vec![
            stroke(
                Path::new()
                    .move_to(300, 300)
                    .quad_to(120, 500, 260, 720)
                    .quad_to(500, 980, 740, 720)
                    .quad_to(880, 500, 700, 300),
            ),
            stroke(Path::line(500, 130, 500, 480)),
        ],
        // Three by three, with a nought and a cross in it.
        Glyph::Grid => vec![
            stroke(Path::line(370, 120, 370, 880)),
            stroke(Path::line(630, 120, 630, 880)),
            stroke(Path::line(120, 370, 880, 370)),
            stroke(Path::line(120, 630, 880, 630)),
            stroke(Path::circle(245, 245, 80)),
            stroke(Path::line(700, 700, 810, 810)),
            stroke(Path::line(810, 700, 700, 810)),
        ],
        // An empty ring. Deliberately not a square box: a box at this stroke
        // weight is hard to tell from the panel's own rules at a glance.
        Glyph::Circle => vec![stroke(Path::circle(500, 500, 300))],
        // The same ring with a tick in it, so a finished row differs from an
        // unfinished one in shape as well as in weight. Shape survives being
        // read at arm's length under a reading light; tone does not.
        Glyph::Check => vec![
            stroke(Path::circle(500, 500, 300)),
            stroke(Path::line(350, 510, 460, 620)),
            stroke(Path::line(460, 620, 670, 390)),
        ],
        // A prompt, not a screen: the chevron and the underscore are what a
        // terminal looks like to anyone who has seen one, and a rectangle at
        // this weight is already the launcher's own tile border.
        Glyph::Terminal => vec![
            stroke(
                Path::new()
                    .move_to(210, 300)
                    .line_to(420, 500)
                    .line_to(210, 700),
            ),
            stroke(Path::line(520, 700, 830, 700)),
        ],
        // A folded newspaper: a masthead rule, a column and a headline block.
        // Distinct in silhouette from the bubble beside it in the launcher,
        // which is the only property that matters at 24 pixels.
        Glyph::News => vec![
            stroke(Path::rounded(120, 220, 760, 560, 50)),
            stroke(Path::line(220, 380, 620, 380)),
            stroke(Path::line(220, 520, 460, 520)),
            stroke(Path::line(220, 640, 460, 640)),
            Shape::Fill(Path::rounded(560, 500, 220, 160, 30)),
        ],
        // A speech bubble with a tail: a conversation, a comment thread, a
        // reply. Two lines inside rather than three, because at 24 pixels the
        // third closes up into a smudge.
        Glyph::Chat => vec![
            stroke(Path::rounded(130, 180, 740, 500, 90)),
            stroke(
                Path::new()
                    .move_to(300, 680)
                    .line_to(300, 870)
                    .line_to(470, 680),
            ),
            stroke(Path::line(280, 340, 720, 340)),
            stroke(Path::line(280, 500, 580, 500)),
        ],
        // The feed mark: a dot at the corner with two arcs radiating from it.
        // The one icon here that people read as a specific meaning rather than
        // a category, so it is drawn as the mark itself and not as a metaphor
        // for it.
        //
        // Each quarter is two quadratics rather than one, for the reason
        // `circle` gives: a single quadratic per quarter sits about six percent
        // proud of the radius at its midpoint, which at 24 pixels stops reading
        // as a curve and starts reading as a corner.
        // A disc with eight rays. The rays stop well short of the box so the
        // whole mark still reads as round at 24 pixels: drawn to the edge, the
        // gaps between them close up and it becomes a blob with a bite out of
        // it. Four square and four diagonal, because seven or nine of them
        // cannot be spaced evenly on a pixel grid this coarse.
        // Two strokes through the middle, kept well inside the box: a cross
        // that reaches the corners reads as a large X on the panel rather than
        // as a small control.
        Glyph::Close => vec![
            stroke(Path::line(250, 250, 750, 750)),
            stroke(Path::line(750, 250, 250, 750)),
        ],
        Glyph::Light => {
            let mut shapes = vec![Shape::Fill(Path::circle(500, 500, 210))];
            for (from, to) in [
                ((500, 120), (500, 250)),
                ((500, 750), (500, 880)),
                ((120, 500), (250, 500)),
                ((750, 500), (880, 500)),
                ((232, 232), (324, 324)),
                ((676, 676), (768, 768)),
                ((232, 768), (324, 676)),
                ((676, 324), (768, 232)),
            ] {
                shapes.push(stroke(Path::line(from.0, from.1, to.0, to.1)));
            }
            shapes
        }
        // An arrow down into a tray. The tray is open at the top rather than a
        // closed box, so the arrow reads as going into something rather than
        // as sitting on top of a rectangle.
        Glyph::Download => vec![
            stroke(Path::line(500, 130, 500, 590)),
            stroke(
                Path::new()
                    .move_to(300, 410)
                    .line_to(500, 610)
                    .line_to(700, 410),
            ),
            stroke(
                Path::new()
                    .move_to(180, 640)
                    .line_to(180, 850)
                    .line_to(820, 850)
                    .line_to(820, 640),
            ),
        ],
        // A ribbon with a notch cut from its foot. Drawn as an outline rather
        // than filled: a solid bookmark at this weight is the heaviest mark in
        // the set and pulls the eye off the title it belongs to.
        Glyph::Bookmark => vec![stroke(
            Path::new()
                .move_to(250, 130)
                .line_to(750, 130)
                .line_to(750, 870)
                .line_to(500, 640)
                .line_to(250, 870)
                .close(),
        )],
        // A funnel. The stem is short and centred, because a long stem at this
        // size reads as a wine glass.
        Glyph::Filter => vec![stroke(
            Path::new()
                .move_to(150, 200)
                .line_to(850, 200)
                .line_to(580, 520)
                .line_to(580, 830)
                .line_to(420, 750)
                .line_to(420, 520)
                .close(),
        )],
        // A head and shoulders. The shoulders are an arc that stops at the box
        // edge rather than a closed shape, so the mark does not turn into a
        // filled semicircle when the stroke is rasterised small.
        Glyph::Person => vec![
            stroke(Path::circle(500, 330, 175)),
            stroke(
                Path::new()
                    .move_to(180, 870)
                    .quad_to(180, 600, 500, 600)
                    .quad_to(820, 600, 820, 870),
            ),
        ],
        // A label with a punched hole, leaning the way a luggage tag hangs.
        Glyph::Tag => vec![
            stroke(
                Path::new()
                    .move_to(520, 130)
                    .line_to(870, 480)
                    .line_to(480, 870)
                    .line_to(130, 520)
                    .line_to(130, 130)
                    .close(),
            ),
            Shape::Fill(Path::circle(310, 310, 75)),
        ],
        // A sphere with one meridian and one parallel. Two lines rather than a
        // graticule: at 24 pixels a third line closes the gaps and the mark
        // fills in solid.
        Glyph::Globe => vec![
            stroke(Path::circle(500, 500, 370)),
            stroke(Path::line(130, 500, 870, 500)),
            stroke(
                Path::new()
                    .move_to(500, 130)
                    .quad_to(280, 500, 500, 870)
                    .quad_to(720, 500, 500, 130),
            ),
        ],
        // Two arrows chasing each other round a circle. Each arm is three
        // quarters of a turn with a head on it, and the two gaps sit opposite
        // so the mark reads as rotation rather than as a broken ring.
        Glyph::Refresh => vec![
            stroke(
                Path::new()
                    .move_to(820, 500)
                    .quad_to(820, 180, 500, 180)
                    .quad_to(250, 180, 200, 380),
            ),
            stroke(
                Path::new()
                    .move_to(180, 130)
                    .line_to(200, 400)
                    .line_to(460, 350),
            ),
            stroke(
                Path::new()
                    .move_to(180, 500)
                    .quad_to(180, 820, 500, 820)
                    .quad_to(750, 820, 800, 620),
            ),
            stroke(
                Path::new()
                    .move_to(820, 870)
                    .line_to(800, 600)
                    .line_to(540, 650),
            ),
        ],
        // Three dots. Filled rather than stroked rings, because a ring this
        // small rasterises to a grey smudge and three grey smudges are not a
        // control anybody will press.
        Glyph::More => vec![
            Shape::Fill(Path::circle(210, 500, 95)),
            Shape::Fill(Path::circle(500, 500, 95)),
            Shape::Fill(Path::circle(790, 500, 95)),
        ],
        // The same mark as the status strip draws, so the row in Settings and
        // the indicator in the bar are recognisably one thing.
        Glyph::Bluetooth => bluetooth(),
        // Bow to the left, blade to the right, two teeth on the underside.
        // The bow is a ring rather than a disc so it stays a key and does not
        // close up into a lollipop when it is rasterised small.
        Glyph::Key => vec![
            Shape::Stroke {
                path: Path::circle(320, 500, 170),
                width: 90,
            },
            stroke(Path::line(490, 500, 880, 500)),
            stroke(Path::line(790, 500, 790, 660)),
            stroke(Path::line(880, 500, 880, 620)),
        ],
        // A horseshoe, poles down, with the tips flared into solid blocks.
        // The flare is what makes it a magnet rather than an arch: a plain
        // stroked U at this weight reads as a horseshoe only if you already
        // knew that is what it was.
        Glyph::Magnet => vec![
            stroke(
                Path::new()
                    .move_to(255, 690)
                    .line_to(255, 470)
                    .quad_to(255, 160, 500, 160)
                    .quad_to(745, 160, 745, 470)
                    .line_to(745, 690),
            ),
            Shape::Fill(Path::rounded(170, 750, 170, 130, 20)),
            Shape::Fill(Path::rounded(660, 750, 170, 130, 20)),
        ],
        Glyph::Rss => vec![
            Shape::Fill(Path::circle(280, 700, 95)),
            stroke(
                Path::new()
                    .move_to(180, 500)
                    .quad_to(304, 500, 392, 588)
                    .quad_to(480, 676, 480, 800),
            ),
            stroke(
                Path::new()
                    .move_to(180, 200)
                    .quad_to(429, 200, 604, 376)
                    .quad_to(780, 551, 780, 800),
            ),
        ],
    }
}

/// The back control, drawn rather than typed.
///
/// The built-in typeface has no arrow, and an application must never be able
/// to substitute this control by writing a character that looks like one. It
/// lives here, in the same geometry as the icons, because the version drawn
/// from rectangles was the fattest thing on the screen.
#[must_use]
pub fn back_arrow() -> Vec<Shape> {
    vec![Shape::Stroke {
        path: Path::new()
            .move_to(640, 180)
            .line_to(320, 500)
            .line_to(640, 820),
        width: 90,
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
    let mut shapes = vec![Shape::Fill(Path::circle(500, 830, 60))];
    let arcs = [
        Path::new().move_to(400, 710).quad_to(500, 630, 600, 710),
        Path::new().move_to(270, 560).quad_to(500, 380, 730, 560),
        Path::new().move_to(120, 400).quad_to(500, 100, 880, 400),
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
    use crate::Glyph;

    const EVERY: [Glyph; Glyph::ALL.len()] = Glyph::ALL;

    fn inked(glyph: Glyph, size: i32) -> usize {
        render(&shapes(glyph), size)
            .alpha
            .iter()
            .filter(|&&value| value > 0)
            .count()
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
    fn the_back_arrow_is_lighter_than_a_third_of_its_box() {
        // It was drawn from rectangles and was the heaviest mark on the panel.
        let coverage = render(&back_arrow(), 60);
        let inked = coverage.alpha.iter().filter(|&&value| value > 0).count();
        assert!(inked * 3 < 60 * 60, "the arrow still fills {inked} pixels");
        assert!(inked > 200, "the arrow is too faint at {inked} pixels");
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
