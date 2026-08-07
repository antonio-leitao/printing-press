//! Markdown line anchors.
//!
//! SyncTeX answers a click in the PDF with a line of the LaTeX that produced
//! it. For a markdown project that LaTeX is pandoc's output, which the author
//! never wrote and never sees, so the answer has to be carried one step
//! further back.
//!
//! Press does that by marking the markdown before pandoc reads it. Each block
//! is preceded by a raw LaTeX comment naming the line it came from — a fenced
//! `{=latex}` block holding `%press:42`.
//!
//! Comments typeset as nothing, so the PDF is byte for byte what it would have
//! been. After the build the markers are read back out of pandoc's output,
//! which gives a table from a line of generated LaTeX to a line of the
//! markdown — and that is what turns a SyncTeX answer into a place in the
//! document the author wrote.
//!
//! `raw_attribute` is what makes this possible, and it is on by default in
//! pandoc's own markdown reader, so nothing about the dialect changes.

/// What a line of generated LaTeX came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// 1-based line in the generated LaTeX.
    pub generated: u32,
    /// 1-based line in the markdown.
    pub source: u32,
}

const OPEN: &str = "```{=latex}";
const CLOSE: &str = "```";
const MARKER: &str = "%press:";

/// Copies the markdown with a marker before each top-level block.
///
/// Only top-level blocks: a marker between the items of a list would end the
/// list and start a second one, and the same goes for anything else whose
/// parts are separated by blank lines. Paragraph granularity is all a peek
/// needs anyway.
pub fn mark(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len() + markdown.len() / 8);
    let mut fence: Option<String> = None;
    let mut in_front_matter = false;
    let mut at_block_start = true;
    let mut in_list = false;

    for (index, line) in markdown.lines().enumerate() {
        let number = index as u32 + 1;
        let trimmed = line.trim_start();

        // Metadata, not content. It is one block from the first `---` to the
        // second, and a marker inside it would be read as a field.
        if number == 1 && line.trim_end() == "---" {
            in_front_matter = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_front_matter {
            if line.trim_end() == "---" || line.trim_end() == "..." {
                in_front_matter = false;
                at_block_start = true;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Inside a fence everything is content, including blank lines.
        if let Some(open) = &fence {
            out.push_str(line);
            out.push('\n');
            if closes_fence(trimmed, open) {
                fence = None;
                at_block_start = false;
            }
            continue;
        }

        if line.trim().is_empty() {
            out.push_str(line);
            out.push('\n');
            at_block_start = true;
            continue;
        }

        if at_block_start {
            let indented = line.starts_with(' ') || line.starts_with('\t');
            let listish = starts_list(trimmed) || trimmed.starts_with(':');
            // A block that belongs to a list is left alone; one that starts a
            // list is marked, because the marker then sits before the list
            // rather than inside it.
            let inside_list = in_list && (indented || listish);
            if !inside_list && !indented {
                out.push_str(OPEN);
                out.push('\n');
                out.push_str(MARKER);
                out.push_str(&number.to_string());
                out.push('\n');
                out.push_str(CLOSE);
                out.push_str("\n\n");
            }
            if !inside_list {
                in_list = starts_list(trimmed);
            }
            at_block_start = false;
        }

        if let Some(open) = opens_fence(trimmed) {
            fence = Some(open);
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Reads the markers back out of pandoc's output, newest position last.
///
/// The result is sorted by generated line, which is the order they are written
/// in, so a lookup is a search for the last anchor at or above a line.
pub fn collect(generated_latex: &str) -> Vec<Anchor> {
    let mut anchors = Vec::new();
    for (index, line) in generated_latex.lines().enumerate() {
        let Some(rest) = line.trim().strip_prefix(MARKER) else {
            continue;
        };
        let Ok(source) = rest.trim().parse::<u32>() else {
            continue;
        };
        anchors.push(Anchor {
            generated: index as u32 + 1,
            source,
        });
    }
    anchors
}

/// The markdown line a line of generated LaTeX came from.
///
/// `None` for anything above the first marker: the preamble, the title block
/// and everything else pandoc writes on its own account, which is not a place
/// in the author's document.
pub fn source_line(anchors: &[Anchor], generated: u32) -> Option<u32> {
    anchors
        .iter()
        .take_while(|anchor| anchor.generated <= generated)
        .last()
        .map(|anchor| anchor.source)
}

/// `generated:source` pairs, one per line — the form stored beside a built PDF.
pub fn encode(anchors: &[Anchor]) -> String {
    let mut out = String::new();
    for anchor in anchors {
        out.push_str(&anchor.generated.to_string());
        out.push(':');
        out.push_str(&anchor.source.to_string());
        out.push('\n');
    }
    out
}

pub fn decode(text: &str) -> Vec<Anchor> {
    text.lines()
        .filter_map(|line| {
            let (generated, source) = line.split_once(':')?;
            Some(Anchor {
                generated: generated.trim().parse().ok()?,
                source: source.trim().parse().ok()?,
            })
        })
        .collect()
}

fn opens_fence(trimmed: &str) -> Option<String> {
    for marker in ["```", "~~~"] {
        if trimmed.starts_with(marker) {
            let run = trimmed
                .chars()
                .take_while(|character| *character == marker.chars().next().unwrap_or('`'))
                .count();
            if run >= 3 {
                return Some(marker.chars().next().unwrap_or('`').to_string().repeat(run));
            }
        }
    }
    None
}

/// A fence closes on a run of its own character at least as long as the one
/// that opened it, with nothing else on the line.
fn closes_fence(trimmed: &str, open: &str) -> bool {
    let character = open.chars().next().unwrap_or('`');
    let run = trimmed.chars().take_while(|c| *c == character).count();
    run >= open.len() && trimmed[run..].trim().is_empty()
}

fn starts_list(trimmed: &str) -> bool {
    let mut characters = trimmed.chars();
    match characters.next() {
        Some('-') | Some('*') | Some('+') => characters.next().is_some_and(|c| c == ' '),
        Some(digit) if digit.is_ascii_digit() => {
            let rest = trimmed.trim_start_matches(|c: char| c.is_ascii_digit());
            rest.starts_with(". ") || rest.starts_with(") ")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marked copy has to compile to the same document, so every marker
    /// goes between blocks and never inside one.
    #[test]
    fn marks_every_top_level_block_once() {
        let markdown = "---\ntitle: A Paper\n---\n\n# Heading\n\nA paragraph.\n\n$$x^2$$\n";
        let marked = mark(markdown);
        let markers: Vec<&str> = marked
            .lines()
            .filter(|line| line.starts_with(MARKER))
            .collect();
        assert_eq!(markers, ["%press:5", "%press:7", "%press:9"]);
        assert!(
            marked.starts_with("---\ntitle: A Paper\n---\n"),
            "the front matter is left alone: {marked}"
        );
    }

    #[test]
    fn leaves_the_inside_of_a_list_alone() {
        let markdown = "Before.\n\n- one\n\n- two\n\n  still two\n\nAfter.\n";
        let marked = mark(markdown);
        let markers: Vec<&str> = marked
            .lines()
            .filter(|line| line.starts_with(MARKER))
            .collect();
        // The list is marked once, at its first item, and `After.` again.
        assert_eq!(markers, ["%press:1", "%press:3", "%press:9"]);
    }

    /// A blank line inside a fence is not a block boundary, and a `#` inside
    /// one is not a heading.
    #[test]
    fn never_marks_inside_a_fence() {
        let markdown = "Text.\n\n```python\nx = 1\n\ny = 2\n```\n\nAfter.\n";
        let marked = mark(markdown);
        let markers: Vec<&str> = marked
            .lines()
            .filter(|line| line.starts_with(MARKER))
            .collect();
        assert_eq!(markers, ["%press:1", "%press:3", "%press:9"]);
        assert!(marked.contains("x = 1\n\ny = 2"), "the fence is untouched");
    }

    #[test]
    fn a_generated_line_resolves_to_the_block_above_it() {
        let anchors = vec![
            Anchor {
                generated: 10,
                source: 5,
            },
            Anchor {
                generated: 20,
                source: 9,
            },
        ];
        assert_eq!(source_line(&anchors, 9), None, "the preamble is nobody's");
        assert_eq!(source_line(&anchors, 10), Some(5));
        assert_eq!(source_line(&anchors, 14), Some(5));
        assert_eq!(source_line(&anchors, 20), Some(9));
        assert_eq!(source_line(&anchors, 900), Some(9));
    }

    #[test]
    fn anchors_survive_a_round_trip_through_storage() {
        let anchors = vec![
            Anchor {
                generated: 3,
                source: 1,
            },
            Anchor {
                generated: 44,
                source: 12,
            },
        ];
        assert_eq!(decode(&encode(&anchors)), anchors);
    }
}
