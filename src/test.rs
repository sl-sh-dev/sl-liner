use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};

use context;

use crate::cursor::CursorPosition;

use super::*;

fn assert_cursor_pos(s: &str, cursor: usize, expected_pos: CursorPosition) {
    let buf = Buffer::from(s.to_owned());
    let words = context::get_buffer_words(&buf);
    let pos = CursorPosition::get(cursor, &words[..]);
    assert!(
        expected_pos == pos,
        "buffer: {:?}, cursor: {}, expected pos: {:?}, pos: {:?}",
        s,
        cursor,
        expected_pos,
        pos
    );
}

#[test]
fn test_get_cursor_position() {
    use crate::cursor::CursorPosition::*;

    let tests = &[
        ("hi", 0, OnWordLeftEdge(0)),
        ("hi", 1, InWord(0)),
        ("hi", 2, OnWordRightEdge(0)),
        ("abc  abc", 4, InSpace(Some(0), Some(1))),
        ("abc  abc", 5, OnWordLeftEdge(1)),
        ("abc  abc", 6, InWord(1)),
        ("abc  abc", 8, OnWordRightEdge(1)),
        (" a", 0, InSpace(None, Some(0))),
        ("a ", 2, InSpace(Some(0), None)),
        ("", 0, InSpace(None, None)),
    ];

    for t in tests {
        assert_cursor_pos(t.0, t.1, t.2);
    }
}

fn assert_buffer_actions(start: &str, expected: &str, actions: &[Action]) {
    let mut buf = Buffer::from(start.to_owned());
    for a in actions {
        a.do_on(&mut buf);
    }

    assert_eq!(expected, String::from(buf));
}

#[test]
fn test_buffer_actions() {
    assert_buffer_actions(
        "",
        "h",
        &[
            Action::Insert {
                start: 0,
                text: "hi".chars().collect(),
            },
            Action::Remove {
                start: 1,
                text: ".".chars().collect(),
            },
        ],
    );
}

#[test]
fn test_history_indexing() {
    let mut h = History::new();
    h.push(Buffer::from("a")).unwrap();
    h.push(Buffer::from("b")).unwrap();
    h.push(Buffer::from("c")).unwrap();
    assert_eq!(h.len(), 3);
    assert_eq!(&h[0], "a");
    assert_eq!(&h[1], "b");
    assert_eq!(&h[2], "c");
}

#[test]
fn test_in_memory_history_truncating() {
    let mut h = History::new();
    h.set_max_history_size(2);
    for _ in 0..4 {
        h.push(Buffer::from("a")).unwrap();
        h.push(Buffer::from("b")).unwrap();
    }
    assert_eq!(h.len(), 2);
}

#[test]
fn test_in_file_history_truncating() {
    let mut tmp_file = env::temp_dir();
    tmp_file.push("liner_test_file123.txt");

    {
        let mut h = History::new();
        let _ = h.set_file_name_and_load_history(&tmp_file).unwrap();
        h.set_max_history_size(5);
        for bytes in b'a'..b'z' {
            h.push(Buffer::from(format!("{}", bytes as char))).unwrap();
        }
        let r = h.commit_to_file();
        assert_eq!(r.is_ok(), true);
    }

    let f = fs::File::open(&tmp_file).unwrap();
    let r = BufReader::new(f);
    let count = r.lines().count();
    assert_eq!(count, 5);

    fs::remove_file(tmp_file).unwrap();
}

static TEXT: &'static str = "a
b
c
d
";

#[test]
fn test_reading_from_file() {
    let mut tmp_file = env::temp_dir();
    tmp_file.push("liner_test_file456.txt");
    {
        let mut f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(tmp_file.clone())
            .unwrap();
        f.write_all(TEXT.as_bytes()).unwrap();
    }
    let mut h = History::new();
    h.set_file_name_and_load_history(tmp_file).unwrap();
    assert_eq!(&h[0], "a");
    assert_eq!(&h[1], "b");
    assert_eq!(&h[2], "c");
    assert_eq!(&h[3], "d");
}

#[test]
fn test_shared_history() {
    let mut tmp_file = env::temp_dir();
    tmp_file.push("liner_shared_file456.txt");
    {
        let mut _f = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(tmp_file.clone())
            .unwrap();
    }
    let mut h1 = History::new();
    assert_eq!(h1.len(), 0);
    h1.set_file_name_and_load_history(tmp_file.clone()).unwrap();
    assert_eq!(h1.len(), 0);
    let mut h2 = History::new();
    h2.set_file_name_and_load_history(tmp_file).unwrap();
    h1.set_search_context(Some("/a".to_string()));
    h1.push(Buffer::from("a")).unwrap();
    h1.push(Buffer::from("b")).unwrap();
    h2.push(Buffer::from("c")).unwrap();
    h2.push(Buffer::from("d")).unwrap();
    /*assert_eq!(h1.len(), 2);
    assert_eq!(h2.len(), 2);
    assert_eq!(h1.load_history(false).is_ok(), true);
    assert_eq!(h2.load_history(false).is_ok(), true);*/
    assert_eq!(h1.load_history(false).is_ok(), true);
    assert_eq!(h1.len(), 4);
    assert_eq!(h2.len(), 4);
    assert_eq!(&h1[0], "c");
    assert_eq!(&h1[1], "d");
    assert_eq!(&h1[2], "a");
    assert_eq!(&h1[3], "b");
    assert_eq!(&h2[0], "a");
    assert_eq!(&h2[1], "b");
    assert_eq!(&h2[2], "c");
    assert_eq!(&h2[3], "d");

    h1.set_search_context(Some("/a/b".to_string()));
    h1.push(Buffer::from("a")).unwrap();
    assert_eq!(&h1[0], "c");
    assert_eq!(&h1[1], "d");
    assert_eq!(&h1[2], "b");
    assert_eq!(&h1[3], "a");
    assert_eq!(h2.load_history(false).is_ok(), true);
    assert_eq!(h2.len(), 4);
    assert_eq!(&h2[0], "b");
    assert_eq!(&h2[1], "a");
    assert_eq!(&h2[2], "c");
    assert_eq!(&h2[3], "d");
    h1.set_search_context(Some("/a/b/1".to_string()));
    h1.push(Buffer::from("a")).unwrap();
    h1.set_search_context(Some("/a/b/2".to_string()));
    h1.push(Buffer::from("a")).unwrap();
    h1.set_search_context(Some("/a/b/3".to_string()));
    h1.push(Buffer::from("a")).unwrap();
    assert_eq!(h2.load_history(false).is_ok(), true);
    assert_eq!(&h2[1], "a");
    assert_eq!(h1.get_context(3).as_ref().unwrap().len(), 5);
    assert_eq!(
        h1.get_context(3)
            .as_ref()
            .unwrap()
            .contains(&"/a".to_string()),
        true
    );
    assert_eq!(
        h1.get_context(3)
            .as_ref()
            .unwrap()
            .contains(&"/a/b".to_string()),
        true
    );
    assert_eq!(
        h1.get_context(3)
            .as_ref()
            .unwrap()
            .contains(&"/a/b/1".to_string()),
        true
    );
    assert_eq!(
        h1.get_context(3)
            .as_ref()
            .unwrap()
            .contains(&"/a/b/2".to_string()),
        true
    );
    assert_eq!(
        h1.get_context(3)
            .as_ref()
            .unwrap()
            .contains(&"/a/b/3".to_string()),
        true
    );
    assert_eq!(h2.get_context(1).as_ref().unwrap().len(), 5);
    assert_eq!(
        h2.get_context(1)
            .as_ref()
            .unwrap()
            .contains(&"/a".to_string()),
        true
    );
    assert_eq!(
        h2.get_context(1)
            .as_ref()
            .unwrap()
            .contains(&"/a/b".to_string()),
        true
    );
    assert_eq!(
        h2.get_context(1)
            .as_ref()
            .unwrap()
            .contains(&"/a/b/1".to_string()),
        true
    );
    assert_eq!(
        h2.get_context(1)
            .as_ref()
            .unwrap()
            .contains(&"/a/b/2".to_string()),
        true
    );
    assert_eq!(
        h2.get_context(1)
            .as_ref()
            .unwrap()
            .contains(&"/a/b/3".to_string()),
        true
    );
    h1.set_search_context(Some("/a/b/4".to_string()));
    h1.push(Buffer::from("a")).unwrap();
    h1.set_search_context(Some("/a/b/1".to_string()));
    h1.push(Buffer::from("a")).unwrap();
    assert_eq!(h2.load_history(false).is_ok(), true);
    assert_eq!(h1.get_context(3).as_ref().unwrap().len(), 1);
    assert_eq!(h1.get_context(3).as_ref().unwrap().get(0).unwrap(), "*");
    assert_eq!(h2.get_context(1).as_ref().unwrap().len(), 1);
    assert_eq!(h2.get_context(1).as_ref().unwrap().get(0).unwrap(), "*");
}
