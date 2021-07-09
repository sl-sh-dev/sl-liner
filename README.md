# sl-liner ![Rust](https://github.com/sl-sh-dev/sl-liner/workflows/Rust/badge.svg?branch=master)
A Rust library offering readline-like functionality.

This was forked from https://gitlab.redox-os.org/redox-os/liner.
It has some changes to history (non-duplicating and context sensitive by default)
and it supports Windows (10 with an ansi escape capable console).

[CONTRIBUTING.md](/CONTRIBUTING.md)

## Featues
- [x] Autosuggestions
- [x] Emacs and Vi keybindings
- [x] Multi-line editing
- [x] History
- [x] Basic and filename completions
- [x] Reverse search
- [ ] Remappable keybindings

## Basic Usage
In `Cargo.toml`:
```toml
[dependencies]
sl_liner = "https://github.com/sl-sh-dev/sl-liner"
...
```

In `src/main.rs`:

```rust
extern crate liner;

use liner::{Context, Completer};

struct EmptyCompleter;

impl<W: std::io::Write> Completer<W> for EmptyCompleter {
    fn completions(&mut self, _start: &str) -> Vec<String> {
        Vec::new()
    }
}

fn main() {
    let mut con = Context::new();

    loop {
        let res = con.read_line("[prompt]$ ", &mut EmptyCompleter).unwrap();

        if res.is_empty() {
            break;
        }

        con.history.push(res.into());
    }
}
```

**See src/main.rs for a more sophisticated example.**

## License
MIT licensed. See the `LICENSE` file.
