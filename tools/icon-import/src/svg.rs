//! Turning SVG path data into the outlines this project draws.
//!
//! # Why hand-rolled
//!
//! The workspace has no dependencies and this runs once, offline, by hand. A
//! path parser is a few hundred lines of well-specified arithmetic, and every
//! line of it is covered by a test below. Pulling a crate in to avoid writing
//! it would put a dependency in the tree for the sake of a program that never
//! ships.
//!
//! # What comes out
//!
//! Only moves, lines, cubics and closes. Quadratics, smooth curves and
//! elliptical arcs are all resolved here, in `f64`, before anything is
//! rounded, so the runtime never has to carry arc trigonometry.

use std::f64::consts::PI;

/// One resolved step of an outline, in user units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Move(f64, f64),
    Line(f64, f64),
    /// Two control points and an end point.
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

/// Reads the `d` attribute of every `<path>` in a document, in order.
///
/// Deliberately not an XML parser. Tabler's files are machine-generated with
/// one attribute per path and no entities, and a real parser would be a
/// second thing to get wrong. Anything that does not match that shape is
/// reported rather than guessed at.
///
/// # Errors
///
/// When a `<path` has no `d="..."` before the tag ends.
pub fn path_data(document: &str) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    let mut rest = document;
    while let Some(start) = rest.find("<path") {
        rest = &rest[start + "<path".len()..];
        let end = rest
            .find('>')
            .ok_or_else(|| "a <path was never closed".to_owned())?;
        let tag = &rest[..end];
        let attribute = tag
            .find("d=\"")
            .ok_or_else(|| format!("a <path has no d attribute: {tag}"))?;
        let value = &tag[attribute + 3..];
        let quote = value
            .find('"')
            .ok_or_else(|| format!("a d attribute is unterminated: {tag}"))?;
        found.push(value[..quote].to_owned());
        rest = &rest[end..];
    }
    Ok(found)
}

/// A cursor over path data.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            at: 0,
        }
    }

    fn skip_separators(&mut self) {
        while let Some(&byte) = self.bytes.get(self.at) {
            if byte.is_ascii_whitespace() || byte == b',' {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    fn done(&mut self) -> bool {
        self.skip_separators();
        self.at >= self.bytes.len()
    }

    fn command(&mut self) -> Option<u8> {
        self.skip_separators();
        let byte = *self.bytes.get(self.at)?;
        if byte.is_ascii_alphabetic() {
            self.at += 1;
            Some(byte)
        } else {
            None
        }
    }

    fn number(&mut self) -> Result<f64, String> {
        self.skip_separators();
        let start = self.at;
        if matches!(self.bytes.get(self.at), Some(b'-' | b'+')) {
            self.at += 1;
        }
        while matches!(self.bytes.get(self.at), Some(byte) if byte.is_ascii_digit()) {
            self.at += 1;
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            while matches!(self.bytes.get(self.at), Some(byte) if byte.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.bytes.get(self.at), Some(b'-' | b'+')) {
                self.at += 1;
            }
            while matches!(self.bytes.get(self.at), Some(byte) if byte.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if self.at == start {
            return Err(format!("expected a number at byte {start}"));
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| "path data is not text".to_owned())?;
        text.parse::<f64>()
            .map_err(|_| format!("'{text}' is not a number"))
    }

    /// An arc flag, which the grammar allows to be written without any
    /// separator at all, so `0 0` and `00` mean the same thing.
    fn flag(&mut self) -> Result<bool, String> {
        self.skip_separators();
        match self.bytes.get(self.at) {
            Some(b'0') => {
                self.at += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.at += 1;
                Ok(true)
            }
            _ => Err(format!("expected an arc flag at byte {}", self.at)),
        }
    }
}

/// The state a path command needs beyond its own arguments.
struct Pen {
    /// Where the next command starts from.
    at: (f64, f64),
    /// Where the current subpath began, which is where a close returns to.
    start: (f64, f64),
    /// The previous cubic's second control point, for a smooth cubic.
    cubic_control: Option<(f64, f64)>,
    /// The previous quadratic's control point, for a smooth quadratic.
    quad_control: Option<(f64, f64)>,
}

/// Resolves one `d` attribute into steps.
///
/// # Errors
///
/// When the data is malformed or uses a command letter that is not in the
/// SVG path grammar.
#[allow(clippy::too_many_lines)]
pub fn parse(data: &str) -> Result<Vec<Step>, String> {
    let mut reader = Reader::new(data);
    let mut steps = Vec::new();
    let mut pen = Pen {
        at: (0.0, 0.0),
        start: (0.0, 0.0),
        cubic_control: None,
        quad_control: None,
    };
    let mut previous: Option<u8> = None;
    while !reader.done() {
        let command = match reader.command() {
            Some(letter) => letter,
            None => {
                // A repeated argument list without its letter repeats the
                // previous command, except that a repeated move draws lines.
                match previous {
                    Some(b'M') => b'L',
                    Some(b'm') => b'l',
                    Some(letter) => letter,
                    None => return Err("path data starts with a number".to_owned()),
                }
            }
        };
        let relative = command.is_ascii_lowercase();
        let (dx, dy) = if relative { pen.at } else { (0.0, 0.0) };
        match command.to_ascii_uppercase() {
            b'M' => {
                let x = reader.number()? + dx;
                let y = reader.number()? + dy;
                steps.push(Step::Move(x, y));
                pen.at = (x, y);
                pen.start = (x, y);
                pen.cubic_control = None;
                pen.quad_control = None;
            }
            b'L' => {
                let x = reader.number()? + dx;
                let y = reader.number()? + dy;
                steps.push(Step::Line(x, y));
                pen.at = (x, y);
                pen.cubic_control = None;
                pen.quad_control = None;
            }
            b'H' => {
                let x = reader.number()? + dx;
                let y = pen.at.1;
                steps.push(Step::Line(x, y));
                pen.at = (x, y);
                pen.cubic_control = None;
                pen.quad_control = None;
            }
            b'V' => {
                let x = pen.at.0;
                let y = reader.number()? + dy;
                steps.push(Step::Line(x, y));
                pen.at = (x, y);
                pen.cubic_control = None;
                pen.quad_control = None;
            }
            b'C' | b'S' => {
                let (x1, y1) = if command.eq_ignore_ascii_case(&b'C') {
                    (reader.number()? + dx, reader.number()? + dy)
                } else {
                    reflected(pen.at, pen.cubic_control)
                };
                let x2 = reader.number()? + dx;
                let y2 = reader.number()? + dy;
                let x = reader.number()? + dx;
                let y = reader.number()? + dy;
                steps.push(Step::Cubic(x1, y1, x2, y2, x, y));
                pen.at = (x, y);
                pen.cubic_control = Some((x2, y2));
                pen.quad_control = None;
            }
            b'Q' | b'T' => {
                let (cx, cy) = if command.eq_ignore_ascii_case(&b'Q') {
                    (reader.number()? + dx, reader.number()? + dy)
                } else {
                    reflected(pen.at, pen.quad_control)
                };
                let x = reader.number()? + dx;
                let y = reader.number()? + dy;
                steps.push(quadratic(pen.at, (cx, cy), (x, y)));
                pen.at = (x, y);
                pen.quad_control = Some((cx, cy));
                pen.cubic_control = None;
            }
            b'A' => {
                let rx = reader.number()?;
                let ry = reader.number()?;
                let rotation = reader.number()?;
                let large = reader.flag()?;
                let sweep = reader.flag()?;
                let x = reader.number()? + dx;
                let y = reader.number()? + dy;
                arc(pen.at, (rx, ry), rotation, large, sweep, (x, y), &mut steps);
                pen.at = (x, y);
                pen.cubic_control = None;
                pen.quad_control = None;
            }
            b'Z' => {
                steps.push(Step::Close);
                pen.at = pen.start;
                pen.cubic_control = None;
                pen.quad_control = None;
            }
            other => return Err(format!("'{}' is not a path command", other as char)),
        }
        previous = Some(command);
        // A close takes no arguments, so it can never repeat.
        if command.eq_ignore_ascii_case(&b'Z') {
            previous = None;
        }
    }
    Ok(steps)
}

/// The control point a smooth curve infers: the previous one mirrored through
/// the current point, or the current point when there was no previous one.
fn reflected(at: (f64, f64), control: Option<(f64, f64)>) -> (f64, f64) {
    match control {
        Some((cx, cy)) => (2.0f64.mul_add(at.0, -cx), 2.0f64.mul_add(at.1, -cy)),
        None => at,
    }
}

/// A quadratic as the cubic that draws exactly the same curve.
///
/// Exact, not approximate: every quadratic is a cubic whose control points sit
/// two thirds of the way from each end towards the quadratic's control point.
fn quadratic(from: (f64, f64), control: (f64, f64), to: (f64, f64)) -> Step {
    let third: f64 = 2.0 / 3.0;
    Step::Cubic(
        third.mul_add(control.0 - from.0, from.0),
        third.mul_add(control.1 - from.1, from.1),
        third.mul_add(control.0 - to.0, to.0),
        third.mul_add(control.1 - to.1, to.1),
        to.0,
        to.1,
    )
}

/// An elliptical arc as a run of cubics.
///
/// The endpoint parameterisation SVG uses says where the arc starts and ends
/// but not where its centre is, so this is the conversion the specification
/// spells out in its implementation notes, followed by one cubic per quarter
/// turn or less. A quarter turn is where the cubic approximation of a circular
/// arc stays within about one part in ten thousand of the true curve, which at
/// a thousand units is a hundredth of a unit.
#[allow(clippy::similar_names)]
fn arc(
    from: (f64, f64),
    radii: (f64, f64),
    rotation: f64,
    large: bool,
    sweep: bool,
    to: (f64, f64),
    steps: &mut Vec<Step>,
) {
    // An arc to where it already is draws nothing, and a zero radius is a
    // straight line. Both are what the specification asks for.
    if (from.0 - to.0).abs() < f64::EPSILON && (from.1 - to.1).abs() < f64::EPSILON {
        return;
    }
    let (mut rx, mut ry) = (radii.0.abs(), radii.1.abs());
    if rx < f64::EPSILON || ry < f64::EPSILON {
        steps.push(Step::Line(to.0, to.1));
        return;
    }
    let angle = rotation.to_radians();
    let (sin, cos) = angle.sin_cos();
    let half = (
        (from.0 - to.0).mul_add(0.5, 0.0),
        (from.1 - to.1).mul_add(0.5, 0.0),
    );
    let x1 = cos.mul_add(half.0, sin * half.1);
    let y1 = cos.mul_add(half.1, -(sin * half.0));

    // Radii too small to reach both ends are scaled up until they just do.
    let excess = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if excess > 1.0 {
        let grow = excess.sqrt();
        rx *= grow;
        ry *= grow;
    }

    let numerator = (rx * rx).mul_add(-(y1 * y1), (rx * rx) * (ry * ry)) - (ry * ry) * (x1 * x1);
    let denominator = (rx * rx).mul_add(y1 * y1, (ry * ry) * (x1 * x1));
    let mut factor = (numerator / denominator).max(0.0).sqrt();
    if large == sweep {
        factor = -factor;
    }
    let cx1 = factor * (rx * y1 / ry);
    let cy1 = factor * -(ry * x1 / rx);
    let centre = (
        cos.mul_add(cx1, -(sin * cy1)) + from.0.midpoint(to.0),
        sin.mul_add(cx1, cos * cy1) + from.1.midpoint(to.1),
    );

    let start = angle_of((1.0, 0.0), ((x1 - cx1) / rx, (y1 - cy1) / ry));
    let mut sweep_angle = angle_of(
        ((x1 - cx1) / rx, (y1 - cy1) / ry),
        ((-x1 - cx1) / rx, (-y1 - cy1) / ry),
    );
    if !sweep && sweep_angle > 0.0 {
        sweep_angle -= 2.0 * PI;
    } else if sweep && sweep_angle < 0.0 {
        sweep_angle += 2.0 * PI;
    }

    let quarters = (sweep_angle.abs() / (PI / 2.0)).ceil().max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = quarters as usize;
    let each = sweep_angle / quarters;
    // The control-point distance that makes a cubic match a circular arc of
    // this angle at both ends and in the middle.
    let alpha = (4.0 / 3.0) * ((each / 4.0).tan());
    let mut theta = start;
    for _ in 0..count {
        let next = theta + each;
        let (sin1, cos1) = theta.sin_cos();
        let (sin2, cos2) = next.sin_cos();
        let point = |c: f64, s: f64| {
            (
                cos.mul_add(rx * c, -(sin * ry * s)) + centre.0,
                sin.mul_add(rx * c, cos * ry * s) + centre.1,
            )
        };
        let (ex, ey) = point(cos2, sin2);
        let (c1x, c1y) = point(alpha.mul_add(-sin1, cos1), alpha.mul_add(cos1, sin1));
        let (c2x, c2y) = point(alpha.mul_add(sin2, cos2), alpha.mul_add(-cos2, sin2));
        steps.push(Step::Cubic(c1x, c1y, c2x, c2y, ex, ey));
        theta = next;
    }
}

/// The signed angle from one vector to another.
fn angle_of(u: (f64, f64), v: (f64, f64)) -> f64 {
    let dot = u.0.mul_add(v.0, u.1 * v.1);
    let length = u.0.hypot(u.1) * v.0.hypot(v.1);
    let cosine = (dot / length).clamp(-1.0, 1.0);
    let sign = if u.0.mul_add(v.1, -(u.1 * v.0)) < 0.0 {
        -1.0
    } else {
        1.0
    };
    sign * cosine.acos()
}

#[cfg(test)]
mod tests {
    use super::{parse, path_data, Step};

    /// Where a step ends, for comparing curves without comparing controls.
    fn ends(steps: &[Step]) -> Vec<(f64, f64)> {
        steps
            .iter()
            .filter_map(|step| match *step {
                Step::Move(x, y) | Step::Line(x, y) | Step::Cubic(_, _, _, _, x, y) => Some((x, y)),
                Step::Close => None,
            })
            .collect()
    }

    fn near(left: (f64, f64), right: (f64, f64)) -> bool {
        (left.0 - right.0).abs() < 1e-6 && (left.1 - right.1).abs() < 1e-6
    }

    #[test]
    fn every_path_in_a_document_is_read_in_order() {
        let document = "<svg><path d=\"M0 0\" /><path d=\"M1 1\" /></svg>";
        assert_eq!(path_data(document).unwrap(), vec!["M0 0", "M1 1"]);
    }

    #[test]
    fn a_relative_command_starts_where_the_last_one_finished() {
        let steps = parse("M10 10 l5 0 l0 5").unwrap();
        assert_eq!(
            steps,
            vec![
                Step::Move(10.0, 10.0),
                Step::Line(15.0, 10.0),
                Step::Line(15.0, 15.0),
            ]
        );
    }

    #[test]
    fn a_repeated_argument_list_repeats_the_command() {
        let steps = parse("M0 0 L1 1 2 2 3 3").unwrap();
        assert_eq!(
            steps,
            vec![
                Step::Move(0.0, 0.0),
                Step::Line(1.0, 1.0),
                Step::Line(2.0, 2.0),
                Step::Line(3.0, 3.0),
            ]
        );
    }

    #[test]
    fn a_repeated_move_draws_lines_rather_than_moving_again() {
        let steps = parse("M0 0 1 1").unwrap();
        assert_eq!(steps, vec![Step::Move(0.0, 0.0), Step::Line(1.0, 1.0)]);
    }

    #[test]
    fn a_minus_sign_separates_numbers_without_any_space() {
        let steps = parse("M0 0L-1-2").unwrap();
        assert_eq!(steps, vec![Step::Move(0.0, 0.0), Step::Line(-1.0, -2.0)]);
    }

    #[test]
    fn a_quadratic_becomes_the_cubic_that_draws_the_same_curve() {
        // Midpoints must agree: a quadratic at t=0.5 and its exact cubic.
        let steps = parse("M0 0 Q10 20 20 0").unwrap();
        let Step::Cubic(x1, y1, x2, y2, x, y) = steps[1] else {
            panic!("a quadratic should become a cubic");
        };
        let mid = |a: f64, b: f64, c: f64, d: f64| (a + 3.0 * b + 3.0 * c + d) / 8.0;
        assert!((mid(0.0, x1, x2, x) - 10.0).abs() < 1e-9);
        assert!((mid(0.0, y1, y2, y) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_smooth_cubic_mirrors_the_control_point_before_it() {
        let steps = parse("M0 0 C1 1 2 2 3 3 S5 5 6 6").unwrap();
        let Step::Cubic(x1, y1, ..) = steps[2] else {
            panic!("a smooth cubic should become a cubic");
        };
        assert!((x1 - 4.0).abs() < 1e-9, "{x1}");
        assert!((y1 - 4.0).abs() < 1e-9, "{y1}");
    }

    #[test]
    fn an_arc_lands_exactly_where_it_was_told_to() {
        for data in [
            "M0 0 A5 5 0 0 1 10 0",
            "M0 0 A5 5 0 1 0 10 0",
            "M0 0 A5 10 30 1 1 10 4",
            "M6 7 a2 2 0 0 1 2 2",
        ] {
            let steps = parse(data).unwrap();
            let last = *ends(&steps).last().unwrap();
            let expected = match data {
                "M6 7 a2 2 0 0 1 2 2" => (8.0, 9.0),
                "M0 0 A5 10 30 1 1 10 4" => (10.0, 4.0),
                _ => (10.0, 0.0),
            };
            assert!(near(last, expected), "{data} ended at {last:?}");
        }
    }

    #[test]
    fn a_half_circle_arc_bulges_the_way_the_sweep_flag_asks() {
        // y grows downward, so the sweep flag that the specification calls the
        // positive-angle direction is the one that goes over the top.
        let over = parse("M0 0 A5 5 0 0 1 10 0").unwrap();
        let under = parse("M0 0 A5 5 0 0 0 10 0").unwrap();
        let extreme = |steps: &[Step], pick: fn(f64, f64) -> f64| {
            ends(steps).into_iter().map(|(_, y)| y).fold(f64::NAN, pick)
        };
        assert!(extreme(&over, f64::min) < -4.9, "{:?}", &over);
        assert!(extreme(&under, f64::max) > 4.9, "{:?}", &under);
    }

    #[test]
    fn radii_too_small_to_reach_are_grown_until_they_do() {
        // A radius of 1 cannot span 10 units; the endpoint must still be hit.
        let steps = parse("M0 0 A1 1 0 0 1 10 0").unwrap();
        assert!(near(*ends(&steps).last().unwrap(), (10.0, 0.0)));
    }

    #[test]
    fn an_arc_with_no_radius_is_a_straight_line() {
        let steps = parse("M0 0 A0 0 0 0 1 10 0").unwrap();
        assert_eq!(steps, vec![Step::Move(0.0, 0.0), Step::Line(10.0, 0.0)]);
    }

    #[test]
    fn arc_flags_may_be_written_with_no_separator_at_all() {
        let packed = parse("M0 0a5 5 0 0110 0").unwrap();
        let spaced = parse("M0 0 a5 5 0 0 1 10 0").unwrap();
        assert_eq!(packed, spaced);
    }

    #[test]
    fn a_close_returns_the_pen_to_where_the_subpath_started() {
        let steps = parse("M5 5 L9 9 Z l1 0").unwrap();
        assert_eq!(*steps.last().unwrap(), Step::Line(6.0, 5.0));
    }

    #[test]
    fn an_unknown_command_is_reported_rather_than_ignored() {
        assert!(parse("M0 0 K1 1").is_err());
    }
}
