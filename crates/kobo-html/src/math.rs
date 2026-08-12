//! Reading `MathML` as a line of text.
//!
//! A paper is mostly prose with formulae set into it, and the formulae are
//! `MathML` by the time they reach a reader: arXiv renders LaTeX with `LaTeXML`,
//! which emits presentation `MathML` wrapped in `<semantics>` alongside an
//! `<annotation encoding="application/x-tex">` holding the original source.
//!
//! Stripping the tags and keeping the text, which is what a general HTML
//! converter does, produces *both* -- the rendered form and the LaTeX, run
//! together with no space between them. `4\times` came out as `44\times` and
//! `\sim 14\times` as `14\sim 14\times`. Every number in every formula was
//! doubled and every operator arrived as a backslash command. That is not a
//! rendering that needs improving; it is two renderings on top of each other.
//!
//! So the element is read rather than stripped. The annotation is dropped, the
//! presentation is walked, and what comes back is one line of characters.
//!
//! # Why not the LaTeX
//!
//! The annotation is right there and is what the author wrote, which makes it
//! tempting. But it is source, not text:
//! `\displaystyle\theta^{*}_{q,a^{*}(q)}(N)=\operatorname{argmax}_{\theta}` is
//! harder to read than the thing it describes. The presentation markup already
//! carries the characters an author would have used -- `θ`, `∗`, `=` -- and
//! only the structure needs saying.
//!
//! # Why the structure is written in ASCII
//!
//! Unicode has superscripts and subscripts, and they would be prettier. They
//! are also sparse: there is no subscript `q`, no superscript `θ`, and a face
//! that has the letter often lacks its subscript. A missing glyph on the panel
//! is a hole, and a formula rendered half in raised digits and half in holes is
//! worse than one rendered plainly. `x^2` and `\theta_q` are drawable
//! everywhere, so the structure is spelled with `^` and `_` and grouped with
//! parentheses when the group is longer than a single character.

/// Renders one `<math>` element as a line of text.
///
/// Takes the whole element, opening tag included, and returns what should
/// stand in its place. Anything it cannot make sense of comes back as the
/// characters it found, which is what the converter would have produced
/// anyway: this can leave a formula plain but never blanks one.
#[must_use]
pub fn render(element: &str) -> String {
    let nodes = parse(inner(element));
    let mut out = String::new();
    for node in &nodes {
        draw(node, &mut out);
    }
    // Formulae are set into a sentence, so what is returned is joined to the
    // words either side of it and cannot carry the layout whitespace the
    // markup was indented with.
    let drawn = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // A trailing operator has no right-hand side to hang on, and `LaTeXML`
    // answers that by leaving it out of the rendering altogether: `4\times`
    // arrives as the presentation `<mn>4</mn>` and nothing else. The paper's
    // own abstract claims a speed-up of "4x", so dropping the operator loses
    // the claim. Where the source says everything the rendering said and more,
    // the source is the one that kept it.
    let source = from_source(element);
    if !source.is_empty() && source.len() > drawn.len() && source.starts_with(&drawn) {
        return source;
    }
    drawn
}

/// The commands that turn up as a formula's whole content.
///
/// Short on purpose. This is not a LaTeX reader -- it exists for the fragments
/// `LaTeXML` declines to render, which are lone operators and relations, and a
/// command missing from here simply leaves the rendering as it was.
const COMMANDS: [(&str, &str); 16] = [
    ("\\times", "\u{d7}"),
    ("\\sim", "~"),
    ("\\approx", "\u{2248}"),
    ("\\leq", "\u{2264}"),
    ("\\geq", "\u{2265}"),
    ("\\neq", "\u{2260}"),
    ("\\pm", "\u{b1}"),
    ("\\cdot", "\u{b7}"),
    ("\\to", "\u{2192}"),
    ("\\rightarrow", "\u{2192}"),
    ("\\infty", "\u{221e}"),
    ("\\ll", "\u{226a}"),
    ("\\gg", "\u{226b}"),
    ("\\percent", "%"),
    ("\\%", "%"),
    ("\\ ", " "),
];

/// Reads the LaTeX the element carries alongside its rendering.
///
/// Returns nothing unless the whole of it is plain characters and commands
/// from [`COMMANDS`]. Anything with structure in it -- a fraction, a
/// subscript, a brace group -- is a formula the presentation markup renders
/// better than this could, so it is left to the presentation.
fn from_source(element: &str) -> String {
    let Some(start) = element.find("<annotation") else {
        return String::new();
    };
    let after = &element[start..];
    let Some(open) = after.find('>') else {
        return String::new();
    };
    let Some(close) = after.find("</annotation") else {
        return String::new();
    };
    if open + 1 > close {
        return String::new();
    }
    let source = crate::decode_entities(&after[open + 1..close]);
    let mut out = String::new();
    let mut rest = source.as_str();
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some((command, replacement)) = COMMANDS
            .iter()
            .filter(|(command, _)| tail.starts_with(command))
            // The longest match, so that `\rightarrow` is not read as `\r`.
            .max_by_key(|(command, _)| command.len())
        else {
            return String::new();
        };
        out.push_str(replacement);
        rest = &tail[command.len()..];
    }
    out.push_str(rest);
    if out.contains(['{', '}', '^', '_']) {
        return String::new();
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The content between a `<math>` tag and its closing tag.
fn inner(element: &str) -> &str {
    let after = element
        .find('>')
        .map_or(element, |at| &element[at.saturating_add(1)..]);
    match after.rfind("</math") {
        Some(at) => &after[..at],
        None => after,
    }
}

/// One piece of the markup: characters, or an element holding more pieces.
#[derive(Debug)]
enum Node {
    Text(String),
    Elem { name: String, children: Vec<Node> },
}

/// Builds the tree.
///
/// Forgiving in the same way the rest of this crate is: an unclosed element
/// simply owns what follows it, and a closing tag with nothing open is
/// ignored. Neither can lose the characters around it.
fn parse(markup: &str) -> Vec<Node> {
    let mut stack: Vec<(String, Vec<Node>)> = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut rest = markup;
    while let Some(at) = rest.find('<') {
        let text = &rest[..at];
        if !text.trim().is_empty() {
            push(&mut stack, &mut nodes, Node::Text(text.to_owned()));
        }
        let tail = &rest[at..];
        let Some(end) = tail.find('>') else {
            // A tag that never closes is a truncated tag, not prose. The text
            // before it has already been taken; clearing what is left is what
            // stops the loop taking it a second time on the way out.
            rest = "";
            break;
        };
        let inside = &tail[1..end];
        rest = &tail[end + 1..];
        let closing = inside.starts_with('/');
        let empty = inside.ends_with('/');
        let name = crate::element_name(inside);
        if name.is_empty() {
            continue;
        }
        if closing {
            // Unwind to the matching open element. A stray close is dropped
            // rather than allowed to collapse the whole stack.
            if let Some(at) = stack.iter().rposition(|(open, _)| *open == name) {
                while stack.len() > at {
                    let (open, children) = stack.pop().expect("checked by rposition");
                    push(
                        &mut stack,
                        &mut nodes,
                        Node::Elem {
                            name: open,
                            children,
                        },
                    );
                }
            }
        } else if empty {
            push(
                &mut stack,
                &mut nodes,
                Node::Elem {
                    name,
                    children: Vec::new(),
                },
            );
        } else {
            stack.push((name, Vec::new()));
        }
    }
    if !rest.trim().is_empty() {
        push(&mut stack, &mut nodes, Node::Text(rest.to_owned()));
    }
    while let Some((name, children)) = stack.pop() {
        push(&mut stack, &mut nodes, Node::Elem { name, children });
    }
    nodes
}

/// Adds a node to whatever is currently open, or to the top level.
fn push(stack: &mut [(String, Vec<Node>)], nodes: &mut Vec<Node>, node: Node) {
    match stack.last_mut() {
        Some((_, children)) => children.push(node),
        None => nodes.push(node),
    }
}

/// Writes one node.
fn draw(node: &Node, out: &mut String) {
    match node {
        Node::Text(text) => push_characters(text, out),
        Node::Elem { name, children } => draw_element(name, children, out),
    }
}

/// Writes the children of a node, in order.
fn draw_all(children: &[Node], out: &mut String) {
    for child in children {
        draw(child, out);
    }
}

/// Renders children to their own string, for the places that need to measure
/// what came back before deciding how to set it.
fn drawn(children: &[Node]) -> String {
    let mut out = String::new();
    draw_all(children, &mut out);
    out
}

fn draw_element(name: &str, children: &[Node], out: &mut String) {
    match name {
        // The LaTeX source, and the tag that introduces it. Dropping these is
        // the whole reason this module exists.
        "annotation" | "annotation-xml" => {}
        // `<semantics>` holds the presentation first and the annotation after,
        // so taking everything but the annotation is taking the presentation.
        "math" | "semantics" | "mrow" | "mstyle" | "mpadded" | "menclose" => {
            draw_all(children, out);
        }
        // Limits sit under and over their operator in print and after it in
        // a line, which is how they are read aloud either way, so they are set
        // exactly as a subscript and a superscript are.
        "msub" | "munder" => script(children, out, "_"),
        "msup" | "mover" => script(children, out, "^"),
        "msubsup" => {
            let (base, rest) = children
                .split_first()
                .map_or((None, &[][..]), |(head, tail)| (Some(head), tail));
            if let Some(base) = base {
                draw(base, out);
            }
            if let Some(under) = rest.first() {
                attach(out, "_", &drawn(std::slice::from_ref(under)));
            }
            if let Some(over) = rest.get(1) {
                attach(out, "^", &drawn(std::slice::from_ref(over)));
            }
        }
        // A fraction is the one construction that cannot be written in a line
        // without saying where it ends, so both halves are grouped unless a
        // half is a single character.
        "mfrac" => {
            let top = children
                .first()
                .map(|node| drawn(std::slice::from_ref(node)));
            let bottom = children
                .get(1)
                .map(|node| drawn(std::slice::from_ref(node)));
            match (top, bottom) {
                (Some(top), Some(bottom)) => {
                    out.push_str(&grouped(&top));
                    out.push('/');
                    out.push_str(&grouped(&bottom));
                }
                (Some(only), None) | (None, Some(only)) => out.push_str(&only),
                (None, None) => {}
            }
        }
        "msqrt" => {
            out.push('\u{221a}');
            out.push_str(&grouped(&drawn(children)));
        }
        // A root's degree comes first in the markup and reads first in text.
        "mroot" => {
            let radicand = children
                .first()
                .map(|node| drawn(std::slice::from_ref(node)));
            let degree = children
                .get(1)
                .map(|node| drawn(std::slice::from_ref(node)));
            if let Some(degree) = degree {
                out.push_str(&degree);
            }
            out.push('\u{221a}');
            out.push_str(&grouped(&radicand.unwrap_or_default()));
        }
        "munderover" => draw_element("msubsup", children, out),
        // A table is a system of equations or a matrix. Rows are separated so
        // the reader can tell where one ends, since a line cannot stack them.
        "mtable" => {
            let rows: Vec<String> = children
                .iter()
                .map(|row| drawn(std::slice::from_ref(row)))
                .filter(|row| !row.trim().is_empty())
                .collect();
            out.push_str(&rows.join("; "));
        }
        "mtr" | "mlabeledtr" => {
            let cells: Vec<String> = children
                .iter()
                .map(|cell| drawn(std::slice::from_ref(cell)))
                .filter(|cell| !cell.trim().is_empty())
                .collect();
            out.push_str(&cells.join(" "));
        }
        // Everything unrecognised keeps its characters. A construction this
        // does not know is still text somebody wrote.
        _ => draw_all(children, out),
    }
}

/// Sets a base and the thing attached to it.
fn script(children: &[Node], out: &mut String, marker: &str) {
    let Some((base, rest)) = children.split_first() else {
        return;
    };
    draw(base, out);
    let attached = drawn(rest);
    attach(out, marker, &attached);
}

/// Writes one `^` or `_` and its argument, grouping when the argument is more
/// than a single character so that `x^2y` cannot be read as `x^(2y)`.
fn attach(out: &mut String, marker: &str, attached: &str) {
    let attached = attached.trim();
    if attached.is_empty() {
        return;
    }
    out.push_str(marker);
    out.push_str(&grouped(attached));
}

/// Parenthesises unless the text is a single character or already bracketed.
fn grouped(text: &str) -> String {
    let text = text.trim();
    if text.chars().take(2).count() < 2 {
        return text.to_owned();
    }
    if starts_and_ends_bracketed(text) {
        return text.to_owned();
    }
    format!("({text})")
}

/// Whether the text is already wrapped in one matched pair of brackets.
fn starts_and_ends_bracketed(text: &str) -> bool {
    let mut characters = text.chars();
    let (Some(first), Some(last)) = (characters.next(), characters.next_back()) else {
        return false;
    };
    if !matches!((first, last), ('(', ')') | ('[', ']') | ('{', '}')) {
        return false;
    }
    // Only when that pair is the outermost one: `(a)+(b)` opens and closes
    // with brackets and is not a bracketed group.
    let mut depth = 0i32;
    for (at, character) in text.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 && at + character.len_utf8() < text.len() {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// Appends the characters of a leaf, dropping what has no drawing.
fn push_characters(text: &str, out: &mut String) {
    for character in text.chars() {
        // `MathML` marks where a multiplication or a function application is
        // meant without printing anything for it. They are structure, not
        // characters, and on a panel they are at best nothing and at worst a
        // missing-glyph box in the middle of a formula.
        if matches!(character, '\u{2061}'..='\u{2064}' | '\u{200b}' | '\u{feff}') {
            continue;
        }
        match plain(character) {
            Some(plain) => out.push(plain),
            None => out.push(character),
        }
    }
}

/// Folds a mathematical alphanumeric symbol onto the letter it is drawn from.
///
/// `𝔼` and `𝟙` are the expectation and the indicator, and a paper uses them
/// constantly. They live in a plane the reading face does not cover, so drawn
/// as themselves they are holes. The letter underneath is the letter the
/// author would have typed, and `E` and `1` are what a reader says out loud,
/// so that is what is drawn.
fn plain(character: char) -> Option<char> {
    // The block is a run of alphabets, each 52 letters, with the digit runs
    // at the end. Anything irregular falls through to the letter tables.
    const DIGITS: u32 = 0x1D7CE;
    let code = character as u32;
    if !(0x1D400..=0x1D7FF).contains(&code) {
        return None;
    }
    if code >= DIGITS {
        let digit = (code - DIGITS) % 10;
        return char::from_u32('0' as u32 + digit);
    }
    let offset = (code - 0x1D400) % 52;
    if offset < 26 {
        char::from_u32('A' as u32 + offset)
    } else {
        char::from_u32('a' as u32 + offset - 26)
    }
}

#[cfg(test)]
mod tests {
    use super::render;

    /// The defect this module was written for: `LaTeXML` ships the rendered
    /// form and the LaTeX source in the same element, so keeping the text of
    /// both printed every formula twice.
    #[test]
    fn the_latex_source_beside_a_formula_is_not_read_as_part_of_it() {
        let doubled =
            "<math alttext=\"4\\times\"><semantics><mrow><mn>4</mn><mo>\u{d7}</mo></mrow>\
             <annotation encoding=\"application/x-tex\">4\\times</annotation></semantics></math>";
        assert_eq!(render(doubled), "4\u{d7}");
    }

    #[test]
    fn a_subscript_and_a_superscript_are_spelled_out_where_they_attach() {
        let squared = "<math><msup><mi>x</mi><mn>2</mn></msup></math>";
        assert_eq!(render(squared), "x^2");
        let indexed = "<math><msub><mi>a</mi><mi>n</mi></msub></math>";
        assert_eq!(render(indexed), "a_n");
    }

    /// `x^2y` would be read as `x` raised to `2y`, which is a different
    /// statement from the one the markup made.
    #[test]
    fn an_attachment_of_more_than_one_character_says_where_it_ends() {
        let markup = "<math><msup><mi>e</mi><mrow><mi>i</mi><mi>\u{3c0}</mi></mrow></msup></math>";
        assert_eq!(render(markup), "e^(i\u{3c0})");
    }

    #[test]
    fn a_fraction_is_written_on_the_line_with_its_halves_kept_apart() {
        let markup =
            "<math><mfrac><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow><mn>2</mn></mfrac></math>";
        assert_eq!(render(markup), "(a+b)/2");
    }

    #[test]
    fn a_square_root_covers_exactly_what_was_under_it() {
        let markup = "<math><msqrt><mrow><mi>x</mi><mo>+</mo><mn>1</mn></mrow></msqrt></math>";
        assert_eq!(render(markup), "\u{221a}(x+1)");
    }

    /// The invisible operators are structure. Drawn, they are missing-glyph
    /// boxes in the middle of a formula.
    #[test]
    fn the_marks_that_were_never_meant_to_be_seen_are_not_drawn() {
        let markup = "<math><mrow><mi>f</mi><mo>\u{2061}</mo><mrow><mo>(</mo><mi>x</mi>\
             <mo>)</mo></mrow></mrow></math>";
        assert_eq!(render(markup), "f(x)");
    }

    /// A paper writes the expectation and the indicator constantly, and both
    /// live in a plane the reading face does not cover.
    #[test]
    fn a_letter_the_panel_cannot_draw_is_folded_onto_the_one_it_is_drawn_from() {
        let markup = "<math><msub><mi>\u{1d53c}</mi><mi>y</mi></msub></math>";
        assert_eq!(render(markup), "E_y");
        let indicator = "<math><mn>\u{1d7d9}</mn></math>";
        assert_eq!(render(indicator), "1");
    }

    /// Nothing here may lose characters. A construction this does not know is
    /// still something an author wrote.
    #[test]
    fn an_unknown_construction_keeps_its_characters_rather_than_vanishing() {
        let markup = "<math><munderover><mo>\u{2211}</mo><mrow><mi>i</mi><mo>=</mo><mn>1</mn>\
             </mrow><mi>n</mi></munderover></math>";
        let text = render(markup);
        assert!(text.contains('\u{2211}'), "{text}");
        assert!(text.contains('n'), "{text}");
    }

    /// `LaTeXML` leaves a trailing operator out of the rendering entirely, so
    /// the paper's own claim of a "4x" speed-up reached the panel as "4".
    #[test]
    fn an_operator_the_rendering_dropped_is_taken_from_the_source() {
        let dangling = "<math alttext=\"4\\times\"><semantics><mn>4</mn>\
             <annotation encoding=\"application/x-tex\">4\\times</annotation></semantics></math>";
        assert_eq!(render(dangling), "4\u{d7}");
        let about = "<math alttext=\"\\sim 14\\times\"><semantics><mn>14</mn>\
             <annotation encoding=\"application/x-tex\">\\sim 14\\times</annotation>\
             </semantics></math>";
        // The source says more here but does not begin with what was drawn, so
        // the drawn form stands rather than being replaced by a guess.
        assert_eq!(render(about), "14");
    }

    /// The source is only ever preferred when it agrees with the rendering and
    /// carries more. A formula with real structure is rendered better by the
    /// presentation markup than by its own LaTeX.
    #[test]
    fn a_formula_with_structure_is_read_from_its_rendering_not_its_source() {
        let structured = "<math alttext=\"\\frac{a}{b}\"><semantics>\
             <mfrac><mi>a</mi><mi>b</mi></mfrac>\
             <annotation encoding=\"application/x-tex\">\\frac{a}{b}</annotation></semantics></math>";
        assert_eq!(render(structured), "a/b");
    }

    #[test]
    fn markup_that_never_closes_is_read_rather_than_dropped() {
        assert_eq!(render("<math><mi>x</mi"), "x");
        assert_eq!(render("<math><mrow><mi>y</mi></math>"), "y");
        assert_eq!(render(""), "");
    }
}
