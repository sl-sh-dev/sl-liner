use crate::{util, Buffer, Editor};
use sl_console::{clear, color, cursor};
use std::cmp::Ordering;
use std::fmt::Write;
use std::io;

#[derive(Debug, Clone)]
pub struct Term {
    // The line of the cursor relative to the prompt. 1-indexed.
    // So if the cursor is on the same line as the prompt, `term_cursor_line == 1`.
    // If the cursor is on the line below the prompt, `term_cursor_line == 2`.
    term_cursor_line: usize,
}

impl Default for Term {
    fn default() -> Self {
        Self::new()
    }
}

fn fmt_io_err(err: std::fmt::Error) -> io::Error {
    let msg = format!("{}", err);
    io::Error::new(io::ErrorKind::Other, msg)
}

impl Term {
    pub fn new() -> Self {
        Term {
            term_cursor_line: 1,
        }
    }

    pub fn clear(&mut self, buf: &mut String) -> io::Result<()> {
        write!(buf, "{}{}", clear::All, cursor::Goto(1, 1)).map_err(fmt_io_err)?;
        self.term_cursor_line = 1;
        Ok(())
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

    /// Move the term cursor to the same line as the prompt.
    fn calc_width(&self, prompt_width: usize, buf_widths: &[usize], terminal_width: usize) -> usize {
        let mut total = 0;

        for line in buf_widths {
            if total % terminal_width != 0 {
                total = ((total / terminal_width) + 1) * terminal_width;
            }

            total += prompt_width + line;
        }

        total
    }

    pub fn display(
        &mut self,
        str_buf: &mut String,
        buf: &Buffer,
        prompt: String,
        cursor: usize,
        autosuggestion: Option<&Buffer>,
        show_completions_hint: Option<&(Vec<String>, Option<usize>)>,
        ed: &mut Editor,
        show_autosuggest: bool,
        no_eol: bool,
    ) -> io::Result<usize> {
        let mut cur = cursor;
        let terminal_width = util::terminal_width()?;
        let prompt_width = util::last_prompt_line_width(&prompt);

        let buf_width = buf.width();

        // Don't let the cursor go over the end!
        let buf_num_chars = buf.num_chars();
        if buf_num_chars < cur {
            cur = buf_num_chars;
        }

        // Can't move past the last character in vi normal mode
        if no_eol && cur != 0 && cur == buf_num_chars {
            cur -= 1;
        }
        let buf_widths = match autosuggestion {
            Some(suggestion) => suggestion.width(),
            None => buf_width,
        };
        // Width of the current buffer lines (including autosuggestion) from the start to the cursor
        let buf_widths_to_cursor = match autosuggestion {
            // Cursor might overrun autosuggestion with history search.
            Some(suggestion) if cur < suggestion.num_chars() => suggestion.range_width(0, cur),
            _ => buf.range_width(0, cur),
        };
        // Total number of terminal spaces taken up by prompt and buffer
        let new_total_width = self.calc_width(prompt_width, &buf_widths, terminal_width);
        let new_total_width_to_cursor =
            self.calc_width(prompt_width, &buf_widths_to_cursor, terminal_width);

        let new_num_lines = (new_total_width + terminal_width) / terminal_width;

        str_buf.push_str("\x1B[?1000l\x1B[?1l");

        if self.term_cursor_line > 1 {
            write!(str_buf, "{}", cursor::Up(self.term_cursor_line as u16 - 1)).map_err(fmt_io_err)?;
        }

        write!(str_buf, "\r{}", clear::AfterCursor).map_err(fmt_io_err)?;

        // If we're cycling through completions, show those
        let mut completion_lines = 0;
        if let Some((completions, i)) = show_completions_hint {
            completion_lines = 1 + Self::print_completion_list(completions, *i, str_buf)?;
            str_buf.push_str("\r\n");
        }

        ed.show_lines(show_autosuggest, prompt_width)?;

        // at the end of the line, move the cursor down a line
        if new_total_width % terminal_width == 0 {
            str_buf.push_str("\r\n");
        }

        self.term_cursor_line = (new_total_width_to_cursor + terminal_width) / terminal_width;

        // The term cursor is now on the bottom line. We may need to move the term cursor up
        // to the line where the true cursor is.
        let cursor_line_diff = new_num_lines as isize - self.term_cursor_line as isize;
        match cursor_line_diff.cmp(&0) {
            Ordering::Greater => write!(str_buf, "{}", cursor::Up(cursor_line_diff as u16))
                .map_err(fmt_io_err)?,
            Ordering::Less => unreachable!(),
            Ordering::Equal => {}
        }

        // Now that we are on the right line, we must move the term cursor left or right
        // to match the true cursor.
        let cursor_col_diff = new_total_width as isize
            - new_total_width_to_cursor as isize
            - cursor_line_diff * terminal_width as isize;
        match cursor_col_diff.cmp(&0) {
            Ordering::Greater => write!(str_buf, "{}", cursor::Left(cursor_col_diff as u16))
                .map_err(fmt_io_err)?,
            Ordering::Less => {
                write!(
                    str_buf,
                    "{}",
                    cursor::Right((-cursor_col_diff) as u16)
                )
                .map_err(fmt_io_err)?;
            }
            Ordering::Equal => {}
        }

        self.term_cursor_line += completion_lines;

        write!(
            str_buf,
            "{}{}",
            color::Reset.fg_str(),
            color::Reset.bg_str()
        )
        .map_err(fmt_io_err)?;

        Ok(cur)
    }
}
