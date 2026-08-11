//! The slice of arXiv's Atom answer this application reads.
//!
//! arXiv's export API replies in Atom with two extra namespaces bolted on:
//! `opensearch` for how many results exist beyond the page in hand, and
//! `arxiv` for the things a paper has and a blog post does not -- the author's
//! note about page count and conference, and the journal it eventually
//! appeared in. Both are read here by local name, because the prefix a server
//! picks for a namespace is its own business and matching on `arxiv:comment`
//! would break the day someone served the same document with a different one.
//!
//! Everything is gathered in one walk and the parser keeps no state beyond the
//! entry it is inside. A feed of a hundred papers is a few hundred kilobytes,
//! and this runs inside a lifecycle callback with a 250 ms deadline over it.

use kobo_xml::{scan, split_name, Event};

/// One paper, as far as the feed describes it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Paper {
    /// The bare arXiv identifier, such as `2401.12345v2`.
    ///
    /// Kept without the address around it because it is what the full-text and
    /// abstract URLs are both built from, and because it is what somebody
    /// reading over your shoulder would write down.
    pub id: String,
    pub title: String,
    pub summary: String,
    pub authors: Vec<String>,
    /// The date first submitted, as `YYYY-MM-DD`.
    pub published: String,
    /// The date of the version being described, as `YYYY-MM-DD`.
    pub updated: String,
    /// Subject classes, primary first, as arXiv writes them.
    pub categories: Vec<String>,
    /// The author's own note: page count, figure count, which conference.
    pub comment: String,
    /// Where it was published, once it was.
    pub journal: String,
}

impl Paper {
    /// The authors as one line, shortened once the list stops being a credit
    /// and starts being a wall.
    ///
    /// Six hundred author papers exist, and a row that tried to name them all
    /// would push every other paper off the screen.
    #[must_use]
    pub fn byline(&self) -> String {
        match self.authors.len() {
            0 => String::new(),
            1..=3 => self.authors.join(", "),
            _ => format!("{} and {} others", self.authors[0], self.authors.len() - 1),
        }
    }
}

/// A page of results, and how many there are behind it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Results {
    pub papers: Vec<Paper>,
    /// What arXiv says the whole query matches, which is almost always more
    /// than the page asked for.
    pub total: u32,
}

/// The elements worth stopping on. Everything else is structure or noise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Field {
    Entry,
    Id,
    Title,
    Summary,
    Author,
    Name,
    Published,
    Updated,
    Category,
    Comment,
    Journal,
    Total,
    None,
}

/// Matches on the local name, so the namespace prefix a server chose does not
/// decide whether a paper has a comment on it.
fn field(name: &str) -> Field {
    match split_name(name).1 {
        "entry" => Field::Entry,
        "id" => Field::Id,
        "title" => Field::Title,
        "summary" => Field::Summary,
        "author" => Field::Author,
        "name" => Field::Name,
        "published" => Field::Published,
        "updated" => Field::Updated,
        "category" => Field::Category,
        "comment" => Field::Comment,
        "journal_ref" => Field::Journal,
        "totalResults" => Field::Total,
        _ => Field::None,
    }
}

/// Collapses the feed's own line wrapping.
///
/// arXiv hard-wraps titles and abstracts at about eighty columns and indents
/// the continuations. Left alone those newlines survive into the paginator and
/// come out as a title broken across two lines in the middle of a word, so the
/// wrapping the server chose is undone before the panel chooses its own.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// The identifier out of an arXiv `id` element, which is an address.
fn identifier(url: &str) -> String {
    url.rsplit_once("/abs/")
        .map_or(url, |(_, tail)| tail)
        .trim()
        .to_owned()
}

/// The day out of an Atom timestamp, which is a full RFC 3339 instant.
///
/// The time of day a preprint was submitted is never the thing anybody wants
/// to read, and it costs a line on a panel this size.
fn day(stamp: &str) -> String {
    stamp.split('T').next().unwrap_or(stamp).trim().to_owned()
}

/// Reads a page of results out of an arXiv Atom document.
///
/// A truncated document yields the entries that did arrive rather than
/// nothing: the byte ceiling on a fetch cuts a long feed mid-entry, and half a
/// page of papers is worth reading.
#[must_use]
pub fn parse(input: &str) -> Results {
    let mut decoded: Vec<String> = Vec::new();
    let mut steps: Vec<Event<'_>> = Vec::new();
    // Gathered first so the decoded strings outlive the walk: the scanner
    // hands back an index into that scratch rather than a borrow of it.
    scan(input, &mut decoded, |event| steps.push(event));

    let mut results = Results::default();
    let mut paper = Paper::default();
    let mut buffer = String::new();
    let mut in_entry = false;
    let mut in_author = false;

    for event in steps {
        match event {
            Event::Open { name, attributes } => {
                match field(name) {
                    Field::Entry => {
                        in_entry = true;
                        paper = Paper::default();
                    }
                    Field::Author => in_author = true,
                    // A category is an empty element carrying its value in an
                    // attribute, so it is read on the way in or not at all.
                    Field::Category if in_entry => {
                        if let Some(term) = kobo_xml::attribute(attributes, "term") {
                            if !paper.categories.contains(&term) {
                                paper.categories.push(term);
                            }
                        }
                    }
                    _ => {}
                }
                buffer.clear();
            }
            Event::Text(text) => buffer.push_str(text),
            Event::Owned(index) => {
                if let Some(text) = decoded.get(index) {
                    buffer.push_str(text);
                }
            }
            Event::Close { name } => {
                let text = tidy(&buffer);
                match field(name) {
                    Field::Entry => {
                        in_entry = false;
                        if !paper.title.is_empty() || !paper.id.is_empty() {
                            results.papers.push(std::mem::take(&mut paper));
                        }
                    }
                    Field::Author => in_author = false,
                    // The feed carries its own id, title and updated stamp
                    // before the first entry. Those describe the query, not a
                    // paper, so only what is inside an entry is taken.
                    Field::Id if in_entry => paper.id = identifier(&text),
                    Field::Title if in_entry => paper.title = text,
                    Field::Summary if in_entry => paper.summary = text,
                    Field::Name if in_entry && in_author && !text.is_empty() => {
                        paper.authors.push(text);
                    }
                    Field::Published if in_entry => paper.published = day(&text),
                    Field::Updated if in_entry => paper.updated = day(&text),
                    Field::Comment if in_entry => paper.comment = text,
                    Field::Journal if in_entry => paper.journal = text,
                    Field::Total if !in_entry => results.total = text.parse().unwrap_or_default(),
                    _ => {}
                }
                buffer.clear();
            }
        }
    }
    // The byte ceiling cuts a long feed wherever it happens to land, which is
    // usually inside an entry rather than between two. That entry has its
    // identifier and its title -- arXiv puts both before the abstract -- so it
    // is kept rather than thrown away for want of a closing tag.
    if in_entry && !paper.title.is_empty() && !paper.id.is_empty() {
        results.papers.push(paper);
    }
    results
}

#[cfg(test)]
mod tests {
    use super::{parse, Paper};

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"
      xmlns:arxiv="http://arxiv.org/schemas/atom"
      xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <title>ArXiv Query</title>
  <id>http://arxiv.org/api/query</id>
  <updated>2024-01-02T00:00:00-05:00</updated>
  <opensearch:totalResults>1204</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/2401.00001v2</id>
    <updated>2024-01-09T18:00:00Z</updated>
    <published>2024-01-01T09:30:00Z</published>
    <title>Attention Is All You
      Need Again</title>
    <summary>  We revisit the transformer
      and find it still works. </summary>
    <author><name>Ada Lovelace</name></author>
    <author><name>Alan Turing</name></author>
    <arxiv:comment>12 pages, 3 figures. Accepted at NeurIPS</arxiv:comment>
    <arxiv:journal_ref>J. Irrepr. Res. 4 (2024) 1-12</arxiv:journal_ref>
    <link href="http://arxiv.org/abs/2401.00001v2" rel="alternate" type="text/html"/>
    <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.CL" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2401.00002v1</id>
    <published>2024-01-02T09:30:00Z</published>
    <title>A Second Paper</title>
    <summary>Shorter.</summary>
    <author><name>Grace Hopper</name></author>
    <category term="cs.SE" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    #[test]
    fn every_entry_in_the_feed_becomes_a_paper() {
        let results = parse(FEED);
        assert_eq!(results.papers.len(), 2);
        assert_eq!(results.total, 1204);
    }

    /// The feed carries its own id, title and stamp before the first entry.
    /// Read carelessly those become the first paper's.
    #[test]
    fn the_feeds_own_heading_is_not_mistaken_for_a_paper() {
        let first = &parse(FEED).papers[0];
        assert_eq!(first.id, "2401.00001v2");
        assert_ne!(first.title, "ArXiv Query");
    }

    /// arXiv hard-wraps at about eighty columns, and those newlines would
    /// otherwise survive into the paginator and break a title mid-phrase.
    #[test]
    fn the_servers_own_line_wrapping_is_undone_before_the_panel_does_its_own() {
        let first = &parse(FEED).papers[0];
        assert_eq!(first.title, "Attention Is All You Need Again");
        assert_eq!(
            first.summary,
            "We revisit the transformer and find it still works."
        );
    }

    #[test]
    fn a_papers_subjects_are_read_in_the_order_arxiv_gives_them() {
        let first = &parse(FEED).papers[0];
        assert_eq!(first.categories, ["cs.LG", "cs.CL"]);
    }

    /// The prefix a server binds to a namespace is its own business, so these
    /// are matched by local name.
    #[test]
    fn the_fields_arxiv_adds_beyond_atom_are_read_too() {
        let first = &parse(FEED).papers[0];
        assert_eq!(first.comment, "12 pages, 3 figures. Accepted at NeurIPS");
        assert_eq!(first.journal, "J. Irrepr. Res. 4 (2024) 1-12");
    }

    #[test]
    fn a_timestamp_is_reduced_to_the_day_it_names() {
        let first = &parse(FEED).papers[0];
        assert_eq!(first.published, "2024-01-01");
        assert_eq!(first.updated, "2024-01-09");
    }

    /// A long author list is a credit, not a byline. Naming six hundred people
    /// would push every other paper off the screen.
    #[test]
    fn a_byline_names_everybody_until_that_stops_being_useful() {
        let two = Paper {
            authors: vec!["Ada Lovelace".into(), "Alan Turing".into()],
            ..Paper::default()
        };
        assert_eq!(two.byline(), "Ada Lovelace, Alan Turing");
        let many = Paper {
            authors: (0..40).map(|n| format!("Author {n}")).collect(),
            ..Paper::default()
        };
        assert_eq!(many.byline(), "Author 0 and 39 others");
    }

    #[test]
    fn a_paper_with_no_authors_has_no_byline_rather_than_an_empty_comma() {
        assert_eq!(Paper::default().byline(), "");
    }

    /// The byte ceiling on a fetch cuts a long feed mid-entry. Half a page of
    /// papers is worth reading; nothing is not.
    #[test]
    fn a_feed_cut_off_mid_entry_still_yields_the_papers_that_arrived() {
        let cut = &FEED[..FEED.len() - 400];
        let results = parse(cut);
        assert_eq!(results.papers.len(), 1);
        assert_eq!(results.papers[0].id, "2401.00001v2");
    }
}
