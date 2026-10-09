//! A whole crossing, driven by a clock made up on the spot.
//!
//! Nothing here needs a terminal, a window or a GPU. A `Crossing` walked by
//! hand, the same trick `crt::Pacing::tick_by` plays, is what lets a test
//! watch a duck cross ten thousand times in a millisecond.
//!
//! The walk is driven by hand, with no clock and no scheduler: a `Crossing`
//! stepped one column at a time proves it goes on at one edge and off at the
//! other, and that no column is covered for longer than the art promises.
//!
//! The schedule cannot be driven by hand, because it reads the wall clock to
//! find which interval it is in. So the tests below run against real seconds
//! at a one- or two-second interval, which is the same arithmetic the quarter
//! hour uses.

use std::time::{Duration, Instant};

use critters::{Critters, Crossing, ART};

/// Seconds past the epoch. Intervals are counted from local midnight and
/// every timezone offset is a whole number of minutes, so these divide into
/// intervals the same way the schedule's clock does.
fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Tick real seconds' worth of frames and count how many times a critter came.
fn watch(critters: &mut Critters, seconds: f64) -> u32 {
    let (start, mut seen, mut standing) = (Instant::now(), 0, false);
    while Instant::now().duration_since(start).as_secs_f64() < seconds {
        critters.tick(Instant::now(), 80, 24);
        let crossing = critters.crossing().is_some();
        if crossing && !standing {
            seen += 1;
        }
        standing = crossing;
        std::thread::sleep(Duration::from_millis(20));
    }
    seen
}

/// One step of a walk: how far along, and what it wanted painted.
type Step = (u32, Vec<(usize, usize, char)>);

/// Walk one crossing by hand, without a scheduler: every step of it, on a
/// screen this size.
fn walk(name: &str, cols: usize, rows: usize) -> Vec<Step> {
    let art = ART.iter().find(|a| a.name == name).unwrap();
    let facing_left = art.right.is_empty();
    let mut out = Vec::new();
    let mut crossing = Crossing {
        art,
        facing_left,
        top: 2,
        step: 0,
    };
    while !crossing.done(cols) {
        let mut cells = Vec::new();
        crossing.paint(cols, rows, &mut cells);
        out.push((crossing.step, cells));
        crossing.step += 1;
    }
    out
}

#[test]
fn a_crossing_goes_on_at_one_edge_and_off_at_the_other() {
    for art in &ART {
        let steps = walk(art.name, 80, 24);
        assert_eq!(
            steps.len() as u32,
            80 + u32::from(art.width),
            "{} takes the wrong number of steps",
            art.name
        );
        // It is off the glass at both ends and on it in the middle.
        assert!(
            steps.first().unwrap().1.is_empty(),
            "{} starts on screen",
            art.name
        );
        assert!(
            steps[steps.len() / 2].1.len() > 3,
            "{} is invisible halfway across",
            art.name
        );
        // The last drawn step is a legitimate sliver at the far edge; the
        // step after it is the retirement, and paints nothing at all.
        let facing_left = art.right.is_empty();
        let mut after = Vec::new();
        Crossing {
            art,
            facing_left,
            top: 2,
            step: 80 + u32::from(art.width),
        }
        .paint(80, 24, &mut after);
        assert!(
            after.is_empty(),
            "{} is still on screen once retired",
            art.name
        );
    }
}

#[test]
fn no_piece_stands_on_one_cell_for_longer_than_a_second() {
    for art in &ART {
        let steps = walk(art.name, 80, 24);
        let mut worst = 0u32;
        for col in 0..80usize {
            let mut run = 0u32;
            for (_, cells) in &steps {
                if cells.iter().any(|(_, c, _)| *c == col) {
                    run += 1;
                    worst = worst.max(run);
                } else {
                    run = 0;
                }
            }
        }
        let ms = worst * u32::from(art.step_ms);
        assert!(
            ms <= 1100,
            "{} covers a cell for {ms} ms ({worst} steps)",
            art.name
        );
    }
}

#[test]
fn taller_than_the_glass_shows_a_band_of_itself() {
    let steps = walk("locomotive", 80, 4);
    let mid = &steps[steps.len() / 2].1;
    assert!(
        !mid.is_empty(),
        "a ten-row locomotive vanished on a four-row screen"
    );
    assert!(mid.iter().all(|(r, _, _)| *r < 4));
}

#[test]
fn wider_than_the_glass_still_crosses() {
    let steps = walk("locomotive", 20, 24);
    assert_eq!(steps.len(), 20 + 54);
    assert!(steps[steps.len() / 2].1.iter().all(|(_, c, _)| *c < 20));
}

#[test]
fn an_interval_brings_a_critter_and_only_one() {
    let mut critters = Critters::new(3, true, Duration::from_secs(1), false, [true; ART.len()]);
    // No interval brings two. The intervals are counted off the clock rather
    // than assumed from the watch's length: a stalled machine runs the watch
    // long, and the bound moves with it.
    let from = epoch();
    let seen = u64::from(watch(&mut critters, 3.2));
    let intervals = epoch() - from + 1;
    assert!(
        (1..=intervals).contains(&seen),
        "{seen} critters in {intervals} intervals"
    );
}

/// The rule that keeps a critter from greeting every return to the terminal.
///
/// A window counts as unattended the moment it loses the keyboard, so leaving
/// and coming back is the ordinary rhythm of using a computer. An interval
/// whose moment passed while nobody was ticking brings nothing; only the next
/// interval, watched from its start, does.
#[test]
fn an_interval_joined_late_brings_nothing() {
    // Wait for the middle of a four-second interval before joining, then
    // watch only while that interval lasts. The clock is read after each
    // tick: a tick that landed in the next interval, after a stall, is one
    // of an interval watched from its start, and that one is owed a critter.
    while epoch() % 4 != 2 {
        std::thread::sleep(Duration::from_millis(50));
    }
    let interval = epoch() / 4;
    let mut critters = Critters::new(5, true, Duration::from_secs(4), false, [true; ART.len()]);
    loop {
        critters.tick(Instant::now(), 80, 24);
        if epoch() / 4 != interval {
            break;
        }
        assert!(
            critters.crossing().is_none(),
            "an interval joined late brought a critter"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn switched_off_it_is_a_build_without_it() {
    let now = Instant::now();
    let mut critters = Critters::new(7, false, Duration::from_secs(1), false, [true; ART.len()]);
    assert_eq!(watch(&mut critters, 2.2), 0);
    assert!(critters.cells().is_empty());
    assert!(critters.wake_at(now).is_none());
}

#[test]
fn every_piece_retired_is_the_same_silence_as_switched_off() {
    let mut critters = Critters::new(9, true, Duration::from_secs(1), false, [false; ART.len()]);
    assert_eq!(watch(&mut critters, 2.2), 0);
    assert!(critters.cells().is_empty());
}

#[test]
fn a_retired_piece_never_comes() {
    let only = ART.iter().position(|a| a.name == "swan").unwrap();
    let mut allowed = [false; ART.len()];
    allowed[only] = true;
    let mut critters = Critters::new(11, true, Duration::from_secs(1), false, allowed);
    let start = Instant::now();
    while Instant::now().duration_since(start) < Duration::from_secs(3) {
        critters.tick(Instant::now(), 80, 24);
        if let Some(name) = critters.crossing() {
            assert_eq!(name, "swan", "a retired piece crossed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Once a crossing is in hand its walk is the caller's clock, so the rest can
/// be driven by hand from whatever moment the schedule started it.
fn crossing_in_hand(seed: u64) -> Critters {
    let mut critters = Critters::new(seed, true, Duration::from_secs(1), false, [true; ART.len()]);
    let start = Instant::now();
    // On the glass, not merely started: a crossing at step nought is still
    // wholly off the edge it came in by, and paints nothing.
    while critters.cells().is_empty() {
        assert!(
            Instant::now().duration_since(start) < Duration::from_secs(5),
            "nothing crossed in five seconds of one-second intervals"
        );
        critters.tick(Instant::now(), 80, 24);
        std::thread::sleep(Duration::from_millis(10));
    }
    critters
}

#[test]
fn a_crossing_ends_however_the_screen_moves_under_it() {
    let mut critters = crossing_in_hand(13);
    let t0 = Instant::now();
    let mut sizes = [(80usize, 24usize), (40, 8), (200, 60), (3, 1)]
        .into_iter()
        .cycle();
    let mut size = sizes.next().unwrap();
    for i in 0..2_000u64 {
        if i % 97 == 0 {
            size = sizes.next().unwrap();
        }
        critters.tick(t0 + Duration::from_millis(i * 13), size.0, size.1);
        for &(row, col, _) in critters.cells() {
            assert!(
                row < size.1 && col < size.0,
                "{row},{col} is off a {size:?} screen"
            );
        }
    }
    assert!(critters.crossing().is_none(), "one was left standing");
}

/// The caller drives its redraw off this, so it means exactly what it says.
/// A piece crossing the middle of a wide screen paints the same number of
/// cells in the same rows on every step, and only the columns move.
#[test]
fn the_answer_is_whether_the_cells_differ_and_nothing_looser() {
    let mut critters = crossing_in_hand(17);
    let t0 = Instant::now();
    let mut previous: Vec<(usize, usize, char)> = critters.cells().to_vec();
    let mut moved_without_growing = 0;
    for i in 0..2_000u64 {
        let changed = critters.tick(t0 + Duration::from_millis(i * 5), 200, 24);
        let cells = critters.cells().to_vec();
        assert_eq!(
            changed,
            cells != previous,
            "at tick {i} the answer disagreed"
        );
        if cells != previous && cells.len() == previous.len() && !cells.is_empty() {
            moved_without_growing += 1;
        }
        previous = cells;
    }
    assert!(
        moved_without_growing > 20,
        "the case this guards was barely exercised"
    );
}

#[test]
fn withdrawing_takes_it_off_and_leaves_nothing() {
    let mut critters = crossing_in_hand(19);
    assert!(critters.withdraw(), "the glass did not change by it");
    assert!(critters.cells().is_empty());
    assert_eq!(critters.crossing(), None);
    assert!(
        !critters.withdraw(),
        "an empty glass changed by being cleared again"
    );
}

/// The caller sleeps on this, so it may never be later than the next thing
/// that changes the picture.
#[test]
fn it_never_asks_to_sleep_through_its_own_next_step() {
    let mut critters = crossing_in_hand(23);
    let t0 = Instant::now();
    for i in 0..1_000u64 {
        let now = t0 + Duration::from_millis(i * 7);
        critters.tick(now, 80, 24);
        let wake = critters
            .wake_at(now)
            .expect("enabled, so it always has a next");
        if let Some(name) = critters.crossing() {
            let art = ART.iter().find(|a| a.name == name).unwrap();
            assert!(
                wake <= now + Duration::from_millis(u64::from(art.step_ms)),
                "{name} would sleep past its next column"
            );
        }
    }
}
