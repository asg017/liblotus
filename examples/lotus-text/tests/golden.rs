//! End-to-end test: parse a fixture, snapshot it, verify the snapshot.

use lotus_text::{format_snapshot, parse, verify_snapshot};

const GOLDEN: &str = "\
# a small sheet demonstrating the engine pipeline
A1: =SUM(B1:B3)
A2: =AVERAGE(B1:B3)
A3: =CONCAT(\"sum=\", A1)
B1: 10
B2: 20
B3: 30
";

#[test]
fn golden_parses_and_evaluates() {
    let sheet = parse(GOLDEN).unwrap();
    assert_eq!(sheet.get("A1").to_string(), "60");
    assert_eq!(sheet.get("A2").to_string(), "20");
    assert_eq!(sheet.get("A3").to_string(), "sum=60");
}

#[test]
fn golden_snapshot_verifies() {
    let sheet = parse(GOLDEN).unwrap();
    let snap = format_snapshot(&sheet);
    verify_snapshot(&snap).expect("self-snapshot should verify");
}

#[test]
fn golden_snapshot_content() {
    let sheet = parse(GOLDEN).unwrap();
    let snap = format_snapshot(&sheet);
    // One line per authored cell, each with a computed tail.
    for line in snap.lines() {
        assert!(line.contains(" => "), "line missing tail: {line:?}");
    }
}

#[test]
fn format_round_trips_through_parse() {
    let once = parse(GOLDEN).unwrap().format_compact();
    let twice = parse(&once).unwrap().format_compact();
    assert_eq!(once, twice);
}
