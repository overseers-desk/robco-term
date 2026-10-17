//! URL hotspots.
//!
//! A filter runs a regex over a flattened copy of the visible screen and
//! records each match as a *hotspot*: a rectangle-ish span with a start and
//! end line and column, which the renderer underlines and the pointer code
//! queries on click. The one shipped filter is [`url_filter`], matching URLs
//! and bare email addresses.
//!
//! Hotspot lines are numbered relative to the first line the buffer was built
//! from, since the buffer holds the visible window rather than the whole
//! grid.
//!
//! ## Pattern design notes
//!
//! [`FULL_URL_PATTERN`] and [`EMAIL_PATTERN`] make two deliberate choices
//! that are visible in what matches, not just in how the pattern is spelled:
//!
//! 1. **No lookaround.** A URL like `www..example.com` should not be a link
//!    -- "www." must not be followed by a second dot. The `regex` crate has
//!    no lookahead by design, so the constraint is folded into a consumed
//!    character class instead: `www\.[^.\s<>'"]` matches the same language,
//!    because the rest of the pattern (`[^\s<>'"]+` followed by one more
//!    character) always consumes at least one character after the dot
//!    anyway. There is no scheme capture group, since only the whole match
//!    is ever read.
//! 2. **`\w` and `\b` are ASCII, not Unicode, in the email pattern.** The
//!    `regex` crate's `\w` is Unicode-aware by default, which would make
//!    `naïve@example.com` match. The email pattern says `(?-u:\w)` and
//!    `(?-u:\b)` to pin both to the ASCII meaning: `[0-9A-Za-z_]`.
//!
//! Both patterns are case-sensitive by default (so `HTTP://` is deliberately
//! not a link), and `.` does not cross a newline, which matters because the
//! buffer is one string with the whole screen in it.

use regex::Regex;

use crate::grid::{filter_buffer, string_width, GridView};

/// A full URL: a scheme or a bare `www.`, then a run of non-delimiters, ending
/// on a character that cannot be trailing punctuation, so the sentence
/// "see https://example.com." yields a link without the full stop.
pub const FULL_URL_PATTERN: &str =
    r#"(?:www\.[^.\s<>'"]|[a-z][a-z0-9+.-]*://[^\s<>'"])[^\s<>'"]*[^!,.\s<>'"\]]"#;

/// A bare email address: word characters, dots or dashes, an `@`, the same
/// again, a dot, and a final run of word characters.
pub const EMAIL_PATTERN: &str = r"(?-u:\b)((?-u:\w)|\.|-)+@((?-u:\w)|\.|-)+\.(?-u:\w)+(?-u:\b)";

/// What a hotspot is for. Only `Link` is actionable; `Marker` is the base
/// `RegExpFilter` type, for a caller adding a filter of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotSpotType {
    NotSpecified,
    Marker,
    Link,
}

/// Which kind of link a URL hotspot turned out to hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlType {
    StandardUrl,
    Email,
    Unknown,
}

/// One match, located on the grid.
///
/// `end_column` is the column of the position one *past* the last character,
/// which is what `Filter::process` produced by passing the match's end offset
/// through `getLineColumn`. [`UrlFilterChain::hot_spot_at`] is written to that
/// convention, so a click on the last character of a link still hits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotSpot {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub kind: HotSpotType,
    /// Capture 0 is the whole match; the rest follow the pattern's groups.
    pub captured_texts: Vec<String>,
}

impl HotSpot {
    /// The whole matched text.
    pub fn text(&self) -> &str {
        self.captured_texts
            .first()
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// Both tests are unanchored searches, so a match anywhere in the text
    /// decides.
    pub fn url_type(&self) -> UrlType {
        let url = self.text();
        if full_url_regex().is_match(url) {
            UrlType::StandardUrl
        } else if email_regex().is_match(url) {
            UrlType::Email
        } else {
            UrlType::Unknown
        }
    }

    /// What activating this hotspot would open: a schemeless URL gets
    /// `http://`, an email address gets `mailto:`, anything else is not
    /// openable.
    pub fn activation_url(&self) -> Option<String> {
        let url = self.text();
        match self.url_type() {
            UrlType::StandardUrl => Some(if url.contains("://") {
                url.to_string()
            } else {
                format!("http://{url}")
            }),
            UrlType::Email => Some(format!("mailto:{url}")),
            UrlType::Unknown => None,
        }
    }
}

fn full_url_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(FULL_URL_PATTERN).expect("FULL_URL_PATTERN compiles"))
}

fn email_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(EMAIL_PATTERN).expect("EMAIL_PATTERN compiles"))
}

/// A regex run over the screen buffer, and the hotspots it produced.
pub struct RegExpFilter {
    name: String,
    regex: Regex,
    kind: HotSpotType,
    hotspots: Vec<HotSpot>,
}

impl RegExpFilter {
    pub fn new(name: &str, regex: Regex) -> Self {
        Self {
            name: name.to_string(),
            regex,
            kind: HotSpotType::Marker,
            hotspots: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn hot_spots(&self) -> &[HotSpot] {
        &self.hotspots
    }

    /// Removes every hotspot found on a previous pass.
    pub fn reset(&mut self) {
        self.hotspots.clear();
    }

    /// Every non-overlapping match in the buffer, located.
    ///
    /// The guard against a pattern that matches the empty string is not
    /// optional: without it the scan never advances.
    pub fn process(&mut self, buffer: &str, line_positions: &[usize]) {
        self.hotspots.clear();
        if self.regex.is_match("") {
            return;
        }
        for caps in self.regex.captures_iter(buffer) {
            let m = caps.get(0).expect("capture 0 is always the whole match");
            let (start_line, start_column) =
                line_column(buffer, line_positions, m.start()).unwrap_or((0, 0));
            let (end_line, end_column) =
                line_column(buffer, line_positions, m.end()).unwrap_or((0, 0));

            let captured_texts: Vec<String> = caps
                .iter()
                .map(|g| g.map(|g| g.as_str().to_string()).unwrap_or_default())
                .collect();

            self.hotspots.push(HotSpot {
                start_line,
                start_column,
                end_line,
                end_column,
                kind: self.kind,
                captured_texts,
            });
        }
    }
}

/// `Filter::getLineColumn`: turn a buffer offset into a grid position.
///
/// The column is the *display width* of the text before the offset on that
/// line, not a character count, so a hotspot after a double-width character
/// lands where the character is drawn rather than where it is stored.
fn line_column(buffer: &str, line_positions: &[usize], position: usize) -> Option<(usize, usize)> {
    for i in 0..line_positions.len() {
        let next_line = if i == line_positions.len() - 1 {
            buffer.len() + 1
        } else {
            line_positions[i + 1]
        };
        if line_positions[i] <= position && position < next_line {
            let from = line_positions[i];
            let to = position.min(buffer.len());
            return Some((i, string_width(&buffer[from..to])));
        }
    }
    None
}

/// The filters that run over one screen, in order. `hot_spot_at` returns the
/// first hit, so an earlier filter wins a contested cell.
pub struct UrlFilterChain {
    filters: Vec<RegExpFilter>,
    first_line: usize,
}

impl Default for UrlFilterChain {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlFilterChain {
    pub fn new() -> Self {
        Self {
            filters: Vec::new(),
            first_line: 0,
        }
    }

    /// The chain the terminal ships with: URLs and email addresses.
    pub fn with_url_filter() -> Self {
        let mut chain = Self::new();
        chain.add(url_filter());
        chain
    }

    pub fn add(&mut self, filter: RegExpFilter) {
        self.filters.push(filter);
    }

    pub fn filters(&self) -> &[RegExpFilter] {
        &self.filters
    }

    /// The absolute line the hotspot line numbers are relative to.
    pub fn first_line(&self) -> usize {
        self.first_line
    }

    /// `TerminalImageFilterChain::setImage` followed by `FilterChain::process`:
    /// flatten the window, run every filter over it.
    pub fn set_image(&mut self, grid: &impl GridView, first_line: usize, lines: usize) {
        self.first_line = first_line;
        let (buffer, positions) = filter_buffer(grid, first_line, lines);
        for filter in &mut self.filters {
            filter.process(&buffer, &positions);
        }
    }

    /// Every hotspot on the window, in filter order.
    pub fn hot_spots(&self) -> Vec<&HotSpot> {
        self.filters.iter().flat_map(|f| f.hot_spots()).collect()
    }

    /// The hotspot covering a window-relative cell, if any.
    ///
    /// The two `continue`s read backwards at first: a spot is rejected only
    /// when the cell is outside it *on the spot's own boundary line*. A cell
    /// on an interior line of a multi-line spot passes both tests whatever
    /// its column, which is what makes a wrapped link clickable along its
    /// whole middle.
    pub fn hot_spot_at(&self, line: usize, column: usize) -> Option<&HotSpot> {
        for filter in &self.filters {
            for spot in filter.hot_spots() {
                if line < spot.start_line || line > spot.end_line {
                    continue;
                }
                if spot.start_line == line && spot.start_column > column {
                    continue;
                }
                if spot.end_line == line && spot.end_column < column {
                    continue;
                }
                return Some(spot);
            }
        }
        None
    }
}

/// `UrlFilter`: `CompleteUrlRegExp`, the alternation of the full-URL and
/// email patterns, tagging its hotspots as links.
pub fn url_filter() -> RegExpFilter {
    let pattern = format!("({FULL_URL_PATTERN}|{EMAIL_PATTERN})");
    let mut filter = RegExpFilter::new(
        "url",
        Regex::new(&pattern).expect("the composed URL pattern compiles"),
    );
    filter.kind = HotSpotType::Link;
    filter
}
