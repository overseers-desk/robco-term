//! URL hotspots: a fixture table of what must and must not be a link, and
//! the exact span each link occupies on the grid.

use term::grid::ScriptedGrid;
use term::hotspots::{UrlFilterChain, UrlType};

fn spots(rows: &[&str]) -> Vec<(usize, usize, usize, usize, String)> {
    let g = ScriptedGrid::new(80, rows);
    let mut chain = UrlFilterChain::with_url_filter();
    chain.set_image(&g, 0, rows.len());
    chain
        .hot_spots()
        .into_iter()
        .map(|s| {
            (
                s.start_line,
                s.start_column,
                s.end_line,
                s.end_column,
                s.text().to_string(),
            )
        })
        .collect()
}

fn matched(row: &str) -> Vec<String> {
    spots(&[row]).into_iter().map(|s| s.4).collect()
}

#[test]
fn the_url_fixtures_match_the_recorded_matches() {
    // Each row: the text on screen, and the exact substrings that must become
    // links. Empty means "nothing here is a link".
    let fixtures: &[(&str, &[&str])] = &[
        ("visit https://example.com now", &["https://example.com"]),
        (
            "visit http://example.com/a/b?c=1 now",
            &["http://example.com/a/b?c=1"],
        ),
        ("bare www.example.com works", &["www.example.com"]),
        (
            "ftp://files.example.org/pub",
            &["ftp://files.example.org/pub"],
        ),
        ("mail me at user@example.com ok", &["user@example.com"]),
        (
            "dotted first.last@sub.example.co",
            &["first.last@sub.example.co"],
        ),
        // The trailing-character class: a sentence's punctuation is not part
        // of the link.
        ("see https://example.com.", &["https://example.com"]),
        ("see https://example.com, then", &["https://example.com"]),
        ("see https://example.com! wow", &["https://example.com"]),
        ("[https://example.com]", &["https://example.com"]),
        // Quotes and angle brackets delimit, they do not join.
        ("<https://example.com>", &["https://example.com"]),
        (r#""https://example.com" said"#, &["https://example.com"]),
        // A doubled dot after www is not a link.
        ("www..example.com", &[]),
        // Case: the scheme is matched lowercase-only.
        ("HTTP://EXAMPLE.COM", &[]),
        ("no url on this line at all", &[]),
        ("not.an.email.address", &[]),
        ("a@b", &[]),
        // Two on one line, both found.
        (
            "http://a.example http://b.example",
            &["http://a.example", "http://b.example"],
        ),
    ];

    for (row, want) in fixtures {
        assert_eq!(
            matched(row),
            want.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "fixture: {row:?}"
        );
    }
}

#[test]
fn a_hotspot_covers_exactly_the_columns_its_text_occupies() {
    let row = "visit https://example.com now";
    let got = spots(&[row]);
    assert_eq!(got.len(), 1);
    let (sl, sc, el, ec, text) = got.into_iter().next().expect("one spot");
    assert_eq!((sl, el), (0, 0));
    assert_eq!(sc, 6, "the column the link starts at");
    assert_eq!(
        ec,
        6 + text.chars().count(),
        "the end column is one past the last character"
    );
}

#[test]
fn hot_spot_at_hits_along_the_whole_link_and_nowhere_else() {
    let row = "visit https://example.com now";
    let g = ScriptedGrid::new(80, &[row]);
    let mut chain = UrlFilterChain::with_url_filter();
    chain.set_image(&g, 0, 1);

    assert!(chain.hot_spot_at(0, 5).is_none(), "the space before it");
    for col in 6..=25 {
        assert!(
            chain.hot_spot_at(0, col).is_some(),
            "column {col} is inside the link"
        );
    }
    assert!(chain.hot_spot_at(0, 26).is_none(), "past the end");
    assert!(chain.hot_spot_at(1, 10).is_none(), "a line with no spots");
}

#[test]
fn a_link_does_not_fuse_with_the_line_below_it() {
    // The imaginary newline after every unwrapped line is exactly what stops
    // this: without it `example.com` and `evil` are adjacent in the buffer.
    let got = matched_rows(&["http://example.com", "evil"]);
    assert_eq!(got, vec!["http://example.com".to_string()]);
}

fn matched_rows(rows: &[&str]) -> Vec<String> {
    spots(rows).into_iter().map(|s| s.4).collect()
}

#[test]
fn a_link_that_wraps_spans_two_lines() {
    // Again: the wrapped line fills its 20 columns exactly.
    let g = ScriptedGrid::new(20, &["see http://example.c", "om/path here"]).wrap(&[0]);
    let mut chain = UrlFilterChain::with_url_filter();
    chain.set_image(&g, 0, 2);
    let spots = chain.hot_spots();
    assert_eq!(spots.len(), 1);
    let s = spots[0];
    assert_eq!(s.text(), "http://example.com/path");
    assert_eq!((s.start_line, s.start_column), (0, 4));
    assert_eq!(s.end_line, 1);
    // Every cell along the middle answers, which is what makes a wrapped link
    // clickable end to end.
    assert!(chain.hot_spot_at(0, 17).is_some());
    assert!(chain.hot_spot_at(1, 0).is_some());
}

#[test]
fn hotspot_lines_are_relative_to_the_window_not_the_grid() {
    let g = ScriptedGrid::with_history(80, 2, &["old", "older", "see http://example.com", "after"]);
    let mut chain = UrlFilterChain::with_url_filter();
    chain.set_image(&g, 2, 2);
    let spots = chain.hot_spots();
    assert_eq!(spots.len(), 1);
    assert_eq!(spots[0].start_line, 0, "first line of the window");
    assert_eq!(chain.first_line(), 2, "which the caller maps back");
}

#[test]
fn url_type_and_activation_match_each_url_kind() {
    let g = ScriptedGrid::new(80, &["www.example.com user@example.com https://x.example"]);
    let mut chain = UrlFilterChain::with_url_filter();
    chain.set_image(&g, 0, 1);
    let spots = chain.hot_spots();
    assert_eq!(spots.len(), 3);

    assert_eq!(spots[0].url_type(), UrlType::StandardUrl);
    assert_eq!(
        spots[0].activation_url().as_deref(),
        Some("http://www.example.com"),
        "a schemeless URL gets http:// prepended"
    );

    assert_eq!(spots[1].url_type(), UrlType::Email);
    assert_eq!(
        spots[1].activation_url().as_deref(),
        Some("mailto:user@example.com")
    );

    assert_eq!(spots[2].url_type(), UrlType::StandardUrl);
    assert_eq!(
        spots[2].activation_url().as_deref(),
        Some("https://x.example"),
        "a URL that already has a scheme is left alone"
    );
}

#[test]
fn a_hotspot_after_a_wide_character_lands_where_it_is_drawn() {
    // The column is a display width, not a character count: two cells per
    // wide character.
    let g = ScriptedGrid::new(80, &["漢字 http://example.com"]);
    let mut chain = UrlFilterChain::with_url_filter();
    chain.set_image(&g, 0, 1);
    let spots = chain.hot_spots();
    assert_eq!(spots.len(), 1);
    assert_eq!(
        spots[0].start_column, 5,
        "two double-width characters and a space"
    );
}

#[test]
fn an_empty_matching_pattern_produces_nothing_instead_of_hanging() {
    // A guard against a pattern that matches the empty string: it would
    // otherwise never advance the scan.
    let g = ScriptedGrid::new(80, &["anything at all"]);
    let mut chain = UrlFilterChain::new();
    chain.add(term::hotspots::RegExpFilter::new(
        "empty",
        regex::Regex::new("x*").expect("pattern"),
    ));
    chain.set_image(&g, 0, 1);
    assert!(chain.hot_spots().is_empty());
}
