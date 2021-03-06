use crate::context::ColorClosure;
use crate::prompt::Prompt;
use crate::{util, Buffer, Cursor};
use sl_console::{clear, color, cursor};
use std::cmp::Ordering;
use std::fmt::Write;
use std::io;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Metrics {
    width: usize,
    prompt_width: usize,
    new_total_width: usize,
    new_total_width_to_cursor: usize,
    new_num_lines: usize,
}

fn calc_width_to_cursor<I>(
    prompt_width: usize,
    buf_widths: I,
    terminal_width: usize,
    last_arg_newline: bool,
) -> usize
where
    I: Iterator<Item = usize>,
{
    let mut total = 0;
    for line in buf_widths {
        if total % terminal_width != 0 {
            total = ((total / terminal_width) + 1) * terminal_width;
        }
        total += prompt_width + line;
    }
    if total == 0 {
        // no lines returned, treat buf_width as 0
        total += prompt_width
    } else if last_arg_newline {
        if total % terminal_width != 0 {
            total = ((total / terminal_width) + 1) * terminal_width;
        }
        total += prompt_width;
    }
    total
}

impl Metrics {
    pub fn new(
        prompt: &str,
        buf: &Buffer,
        cursor: &Cursor,
        autosuggestion: Option<&Buffer>,
    ) -> io::Result<Self> {
        let width = util::terminal_width()?;
        let prompt_width = util::last_prompt_line_width(prompt);
        let orig_buf_width = buf.line_widths();

        let (buf_widths, buf_widths_end_newline) = match autosuggestion {
            Some(suggestion) => (suggestion.line_widths(), suggestion.is_last_arg_newline()),
            None => (orig_buf_width, buf.is_last_arg_newline()),
        };

        // Width of the current buffer lines (including autosuggestion) from the start to the cursor
        let (buf_widths_to_cursor, buf_widths_cursor_end_newline) = match autosuggestion {
            // Cursor might overrun autosuggestion with history search.
            Some(suggestion) if cursor.curr_grapheme() < suggestion.num_graphemes() => (
                suggestion.line_width_until(cursor.curr_grapheme()),
                suggestion.is_last_arg_newline(),
            ),
            _ => (
                buf.line_width_until(cursor.curr_grapheme()),
                buf.is_last_arg_newline(),
            ),
        };

        // Total number of terminal spaces taken up by prompt and buffer
        let new_total_width =
            calc_width_to_cursor(prompt_width, buf_widths, width, buf_widths_end_newline);
        let new_total_width_to_cursor = calc_width_to_cursor(
            prompt_width,
            buf_widths_to_cursor,
            width,
            buf_widths_cursor_end_newline,
        );

        let new_num_lines = (new_total_width + width) / width;

        Ok(Metrics {
            width,
            prompt_width,
            new_total_width,
            new_total_width_to_cursor,
            new_num_lines,
        })
    }

    pub fn term_cursor_line(&self) -> usize {
        (self.new_total_width_to_cursor + self.width) / self.width
    }

    pub fn cursor_line_diff(&self) -> isize {
        self.new_num_lines as isize - self.term_cursor_line() as isize
    }

    pub fn at_end_of_line(&self) -> bool {
        self.new_total_width % self.width == 0
    }

    pub fn cursor_col_diff(&self) -> isize {
        self.new_total_width as isize
            - self.new_total_width_to_cursor as isize
            - self.cursor_line_diff() * self.width as isize
    }
}

/// An interface to simplify writing to and clearing the terminal. This is where most
/// of the display logic for interfacing with the console library exists.
pub struct Terminal<'a> {
    out: &'a mut dyn io::Write,
    // The line of the cursor relative to the prompt. 1-indexed.
    // So if the cursor is on the same line as the prompt, `term_cursor_line == 1`.
    // If the cursor is on the line below the prompt, `term_cursor_line == 2`.
    term_cursor_line: usize,
    // Last string that was colorized and last colorized version.
    color_lines: Option<(String, String)>,
    // A closure that is evaluated just before we write to out.
    // This allows us to do custom syntax highlighting and other fun stuff.
    closure: Option<ColorClosure>,
    // Use the closure if it is set.
    use_closure: bool,
    buf: &'a mut String,
}

fn fmt_io_err(err: std::fmt::Error) -> io::Error {
    let msg = format!("{}", err);
    io::Error::new(io::ErrorKind::Other, msg)
}

impl<'a> Terminal<'a> {
    pub fn new(
        closure: Option<ColorClosure>,
        buf: &'a mut String,
        out: &'a mut dyn io::Write,
    ) -> Self {
        Terminal {
            out,
            term_cursor_line: 1,
            color_lines: None,
            closure,
            buf,
            use_closure: true,
        }
    }

    pub fn make_prompt(&mut self, prompt: Prompt) -> io::Result<Prompt> {
        self.out.write_all("???".as_bytes())?;
        for _ in 0..(util::terminal_width().unwrap_or(80) - 1) {
            self.out.write_all(b" ")?; // if the line is not empty, overflow on next line
        }
        self.out.write_all("\r \r".as_bytes())?; // Erase the "???" if nothing overwrites it
        let Prompt {
            prefix,
            mut prompt,
            suffix,
        } = prompt;
        for (i, pline) in prompt.split('\n').enumerate() {
            if i > 0 {
                self.out.write_all(b"\r\n")?;
            }
            self.out.write_all(pline.as_bytes())?;
        }
        if let Some(index) = prompt.rfind('\n') {
            prompt = prompt.split_at(index + 1).1.into()
        }
        Ok(Prompt {
            prefix,
            prompt,
            suffix,
        })
    }

    pub fn set_closure(&mut self, closure: ColorClosure) -> &mut Self {
        self.closure = Some(closure);
        self
    }

    pub fn use_closure(&mut self, use_closure: bool) {
        self.use_closure = use_closure;
    }

    fn colorize(&mut self, line: &str) -> String {
        match self.closure {
            Some(ref mut f) if self.use_closure => {
                let color = f(line);
                self.color_lines = Some((line.to_string(), color.clone()));
                color
            }
            Some(_) => {
                if let Some((old_line, colorized)) = &self.color_lines {
                    if line.starts_with(old_line) {
                        let mut new_line = colorized.clone();
                        new_line.push_str(&line[old_line.len()..]);
                        new_line
                    } else {
                        line.to_owned()
                    }
                } else {
                    line.to_owned()
                }
            }
            _ => line.to_owned(),
        }
    }

    fn display_with_suggest(
        &mut self,
        line: &str,
        is_search: bool,
        buf_num_remaining_bytes: usize,
    ) -> io::Result<()> {
        let start = self.colorize(&line[..buf_num_remaining_bytes]);
        if is_search {
            write!(self.buf, "{}", color::Yellow.fg_str()).map_err(fmt_io_err)?;
        }
        write!(self.buf, "{}", start).map_err(fmt_io_err)?;
        if !is_search {
            write!(self.buf, "{}", color::Yellow.fg_str()).map_err(fmt_io_err)?;
        }
        self.buf.push_str(&line[buf_num_remaining_bytes..]);
        Ok(())
    }

    pub fn clear(&mut self) -> io::Result<()> {
        write!(self.buf, "{}{}", clear::All, cursor::Goto(1, 1)).map_err(fmt_io_err)?;
        self.term_cursor_line = 1;
        Ok(())
    }

    pub fn write_prompt(&mut self, prompt: &str) -> io::Result<()> {
        write!(self.buf, "{}", prompt).map_err(fmt_io_err)
    }

    fn print_completion_list(
        completions: &[String],
        highlighted: Option<usize>,
        output_buf: &mut String,
    ) -> io::Result<usize> {
        use std::cmp::max;

        let (w, _) = sl_console::terminal_size()?;

        // XXX wide character support
        let max_word_size = completions.iter().fold(1, |m, x| max(m, x.chars().count()));
        let cols = max(1, w as usize / (max_word_size));
        let col_width = 2 + w as usize / cols;
        let cols = max(1, w as usize / col_width);

        let lines = completions.len() / cols;

        let mut i = 0;
        for (index, com) in completions.iter().enumerate() {
            match i.cmp(&cols) {
                Ordering::Greater => unreachable!(),
                Ordering::Less => {}
                Ordering::Equal => {
                    output_buf.push_str("\r\n");
                    i = 0;
                }
            }

            if Some(index) == highlighted {
                write!(
                    output_buf,
                    "{}{}",
                    color::Black.fg_str(),
                    color::White.bg_str()
                )
                .map_err(fmt_io_err)?;
            }
            write!(output_buf, "{:<1$}", com, col_width).map_err(fmt_io_err)?;
            if Some(index) == highlighted {
                write!(
                    output_buf,
                    "{}{}",
                    color::Reset.bg_str(),
                    color::Reset.fg_str()
                )
                .map_err(fmt_io_err)?;
            }

            i += 1;
        }

        Ok(lines)
    }

    pub fn clear_after_cursor(&mut self) -> io::Result<()> {
        self.buf.push_str("\x1B[?1000l\x1B[?1l");

        if self.term_cursor_line > 1 {
            write!(self.buf, "{}", cursor::Up(self.term_cursor_line as u16 - 1))
                .map_err(fmt_io_err)?;
        }

        write!(self.buf, "\r{}", clear::AfterCursor).map_err(fmt_io_err)
    }

    pub fn maybe_write_completions(
        &mut self,
        show_completions_hint: Option<&(Vec<String>, Option<usize>)>,
    ) -> io::Result<usize> {
        // If we're cycling through completions, show those
        let mut completion_lines = 0;
        if let Some((completions, i)) = show_completions_hint {
            completion_lines = 1 + Self::print_completion_list(completions, *i, self.buf)?;
            self.buf.push_str("\r\n");
        }
        Ok(completion_lines)
    }

    pub(crate) fn show_lines(
        &mut self,
        buf: &Buffer,
        autosuggestion: Option<&Buffer>,
        show_autosuggest: bool,
        metrics: Metrics,
        is_search: bool,
    ) -> io::Result<()> {
        // If we have an autosuggestion, we make the autosuggestion the buffer we print out.
        // We get the number of bytes in the buffer (but NOT the autosuggestion).
        // Then, we loop and subtract from that number until it's 0, in which case we are printing
        // the autosuggestion from here on (in a different color).
        let (lines, lines_len) = match autosuggestion {
            Some(suggestion) if show_autosuggest => (suggestion.lines(), suggestion.num_lines()),
            _ => (buf.lines(), buf.num_lines()),
        };
        let mut buf_num_remaining_bytes = buf.num_bytes();

        for (i, line) in lines.enumerate() {
            if i > 0 {
                write!(self.buf, "{}", cursor::Right(metrics.prompt_width as u16))
                    .map_err(fmt_io_err)?;
            }

            if buf_num_remaining_bytes == 0 {
                self.buf.push_str(line);
            } else if line.as_bytes().len() > buf_num_remaining_bytes {
                self.display_with_suggest(line, is_search, buf_num_remaining_bytes)?;
                buf_num_remaining_bytes = 0;
            } else {
                buf_num_remaining_bytes -= line.as_bytes().len();
                let written_line = self.colorize(line);
                if is_search {
                    write!(self.buf, "{}", color::Yellow.fg_str()).map_err(fmt_io_err)?;
                }
                self.buf.push_str(&written_line);
            }
            if i + 1 < lines_len {
                self.buf.push_str("\r\n");
            }
        }
        Ok(())
    }

    pub(crate) fn display(&mut self, metrics: Metrics, completion_lines: usize) -> io::Result<()> {
        // at the end of the line, move the cursor down a line
        if metrics.at_end_of_line() {
            self.buf.push_str("\r\n");
        }

        // The term cursor is now on the bottom line. We may need to move the term cursor up
        // to the line where the true cursor is.
        let cursor_line_diff = metrics.cursor_line_diff();
        match cursor_line_diff.cmp(&0) {
            Ordering::Greater => {
                write!(self.buf, "{}", cursor::Up(cursor_line_diff as u16)).map_err(fmt_io_err)?;
            }
            Ordering::Less => unreachable!(),
            Ordering::Equal => {}
        }

        // Now that we are on the right line, we must move the term cursor left or right
        // to match the true cursor.
        let cursor_col_diff = metrics.cursor_col_diff();
        match cursor_col_diff.cmp(&0) {
            Ordering::Greater => {
                write!(self.buf, "{}", cursor::Left(cursor_col_diff as u16)).map_err(fmt_io_err)?
            }
            Ordering::Less => {
                write!(self.buf, "{}", cursor::Right((-cursor_col_diff) as u16))
                    .map_err(fmt_io_err)?;
            }
            Ordering::Equal => {}
        }

        self.term_cursor_line = completion_lines + metrics.term_cursor_line();

        write!(
            self.buf,
            "{}{}",
            color::Reset.fg_str(),
            color::Reset.bg_str()
        )
        .map_err(fmt_io_err)?;

        self.out.write_all(self.buf.as_bytes())?;
        self.buf.clear();
        self.out.flush()?;

        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    pub fn write_newline(&mut self) -> io::Result<()> {
        self.out.write_all(b"\r\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DefaultEditorRules;

    #[test]
    fn test_metrics_cursor_beg_no_autosuggestion() {
        let prompt = "&>";

        let rules = DefaultEditorRules::default();
        let cur = Cursor::new_with_divider(&rules);

        let buf = Buffer::from("hello hello".to_owned());
        let autosuggestion = Buffer::default();
        let m = Metrics::new(prompt, &buf, &cur, Some(&autosuggestion)).unwrap();
        assert_eq!(m.width, 80);
        assert_eq!(m.prompt_width, 2);
        assert_eq!(m.new_total_width, 2);
        assert_eq!(m.new_total_width_to_cursor, 2);
        assert_eq!(m.new_num_lines, 1);
    }

    #[test]
    fn test_metrics_cursor_end_no_autosuggestion() {
        let prompt = "&>";

        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);
        let buf = Buffer::from("hello hello".to_owned());
        cur.move_cursor_to_end_of_line(&buf);
        let autosuggestion = Buffer::from("".to_owned());
        let m = Metrics::new(prompt, &buf, &cur, Some(&autosuggestion)).unwrap();
        assert_eq!(m.width, 80);
        assert_eq!(m.prompt_width, 2);
        assert_eq!(m.new_total_width, 2);
        assert_eq!(m.new_total_width_to_cursor, 13);
        assert_eq!(m.new_num_lines, 1);
    }

    #[test]
    fn test_metrics_cursor_beg_with_autosuggestion() {
        let prompt = "&>";

        let rules = DefaultEditorRules::default();
        let cur = Cursor::new_with_divider(&rules);
        let buf = Buffer::from("hello hello".to_owned());
        let autosuggestion = Buffer::from("hello hello hello".to_owned());
        let m = Metrics::new(prompt, &buf, &cur, Some(&autosuggestion)).unwrap();
        assert_eq!(m.width, 80);
        assert_eq!(m.prompt_width, 2);
        assert_eq!(m.new_total_width, 19);
        assert_eq!(m.new_total_width_to_cursor, 2);
        assert_eq!(m.new_num_lines, 1);
    }

    #[test]
    fn test_metrics_cursor_end_with_autosuggestion() {
        let prompt = "&>";

        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);
        let buf = Buffer::from("hello hello".to_owned());
        cur.move_cursor_to_end_of_line(&buf);
        let autosuggestion = Buffer::from("hello hello hello".to_owned());
        let m = Metrics::new(prompt, &buf, &cur, Some(&autosuggestion)).unwrap();
        assert_eq!(m.width, 80);
        assert_eq!(m.prompt_width, 2);
        assert_eq!(m.new_total_width, 19);
        assert_eq!(m.new_total_width_to_cursor, 13);
        assert_eq!(m.new_num_lines, 1);
    }

    #[test]
    fn test_metrics_cursor_multiline() {
        let prompt = "&>";

        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);
        let buf = Buffer::from("hello hello\nhello hello\nhello hello\nhello hello\nhello hello\nhello hello\nhello hello\nhello hello\nhello hello\nhello hello\nhello hello\nhello hello\n".to_owned());
        cur.move_cursor_to_end_of_line(&buf);
        let autosuggestion = Buffer::from("".to_owned());
        let m = Metrics::new(prompt, &buf, &cur, Some(&autosuggestion)).unwrap();
        assert_eq!(m.width, 80);
        assert_eq!(m.prompt_width, 2);
        assert_eq!(m.new_total_width, 2);
        assert_eq!(m.new_total_width_to_cursor, 962);
        assert_eq!(m.new_num_lines, 1);
    }

    #[test]
    fn test_last_arg_is_newline() {
        let newline = Buffer::from("\n".to_owned());
        assert!(newline.is_last_arg_newline());
        let carriage_return = Buffer::from("\n\r".to_owned());
        assert!(carriage_return.is_last_arg_newline());
    }
}
