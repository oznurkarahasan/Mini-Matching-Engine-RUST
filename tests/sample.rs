use assignment::run;

#[test]
#[ignore = "engine not implemented yet; un-ignore in step 7"]
fn sample_matches_expected_byte_for_byte() {
    let input = include_str!("../sample_input.txt");
    let expected = include_str!("../sample_expected.txt");

    let mut out = Vec::new();
    let skipped = run(&mut input.as_bytes(), &mut out).expect("in-memory I/O");

    assert_eq!(skipped, 0);
    assert_eq!(String::from_utf8(out).expect("output is ASCII"), expected);
}
