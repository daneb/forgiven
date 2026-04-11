use super::Editor;

/// Map a delimiter character to its (open, close) pair.
pub(super) fn surround_pair(ch: char) -> (char, char) {
    match ch {
        '(' | ')' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        _ => (ch, ch),
    }
}

/// On `row`, find the innermost enclosing `(open, close)` pair relative to
/// `cursor_col`.  Returns `(open_col, close_col)` or `None`.
pub(super) fn find_surround_on_line(
    chars: &[char],
    cursor_col: usize,
    open: char,
    close: char,
) -> Option<(usize, usize)> {
    let at = cursor_col.min(chars.len().saturating_sub(1));
    // Search backwards for the open char.
    let open_pos = (0..=at).rev().find(|&i| chars[i] == open)?;
    // Search forwards for the close char after the open.
    let close_pos = (open_pos + 1..chars.len()).find(|&i| chars[i] == close)?;
    Some((open_pos, close_pos))
}

impl Editor {
    pub(super) fn apply_surround_delete(&mut self, ch: char) {
        let (open, close) = surround_pair(ch);
        let result = self.current_buffer().and_then(|buf| {
            let row = buf.cursor.row;
            let col = buf.cursor.col;
            let chars: Vec<char> = buf.lines()[row].chars().collect();
            find_surround_on_line(&chars, col, open, close).map(|pair| (row, pair))
        });
        match result {
            Some((row, (op, cp))) => {
                self.with_buffer(|buf| buf.surround_delete_chars(row, op, cp));
            },
            None => {
                self.set_status(format!("No surrounding '{ch}' found on current line"));
            },
        }
    }

    pub(super) fn apply_surround_change(&mut self, from: char, to: char) {
        let (open_from, close_from) = surround_pair(from);
        let (open_to, close_to) = surround_pair(to);
        let result = self.current_buffer().and_then(|buf| {
            let row = buf.cursor.row;
            let col = buf.cursor.col;
            let chars: Vec<char> = buf.lines()[row].chars().collect();
            find_surround_on_line(&chars, col, open_from, close_from).map(|pair| (row, pair))
        });
        match result {
            Some((row, (op, cp))) => {
                self.with_buffer(|buf| buf.surround_replace_chars(row, op, cp, open_to, close_to));
            },
            None => {
                self.set_status(format!("No surrounding '{from}' found on current line"));
            },
        }
    }

    pub(super) fn apply_surround_add_word(&mut self, ch: char) {
        let (open, close) = surround_pair(ch);
        if let Some(buf) = self.current_buffer() {
            let row = buf.cursor.row;
            let col = buf.cursor.col;
            let chars: Vec<char> = buf.lines()[row].chars().collect();
            if chars.is_empty() {
                return;
            }
            let at = col.min(chars.len().saturating_sub(1));
            // Find word start (non-whitespace run containing `at`).
            let word_start =
                (0..=at).rev().find(|&i| chars[i].is_whitespace()).map(|i| i + 1).unwrap_or(0);
            // Find word end (exclusive).
            let word_end =
                (at..chars.len()).find(|&i| chars[i].is_whitespace()).unwrap_or(chars.len());
            let row_copy = row;
            let ws = word_start;
            let we = word_end;
            self.with_buffer(move |buf| buf.surround_insert_chars(row_copy, ws, we, open, close));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn surround_pair_parens() {
        assert_eq!(surround_pair('('), ('(', ')'));
        assert_eq!(surround_pair(')'), ('(', ')'));
    }

    #[test]
    fn surround_pair_braces() {
        assert_eq!(surround_pair('{'), ('{', '}'));
    }

    #[test]
    fn surround_pair_symmetric() {
        assert_eq!(surround_pair('"'), ('"', '"'));
        assert_eq!(surround_pair('\''), ('\'', '\''));
    }

    #[test]
    fn find_surround_basic() {
        let chars: Vec<char> = "(hello)".chars().collect();
        assert_eq!(find_surround_on_line(&chars, 3, '(', ')'), Some((0, 6)));
    }

    #[test]
    fn find_surround_none() {
        let chars: Vec<char> = "hello".chars().collect();
        assert_eq!(find_surround_on_line(&chars, 2, '(', ')'), None);
    }

    #[test]
    fn find_surround_cursor_at_open() {
        let chars: Vec<char> = "(hi)".chars().collect();
        assert_eq!(find_surround_on_line(&chars, 0, '(', ')'), Some((0, 3)));
    }

    #[test]
    fn find_surround_cursor_at_close() {
        // cursor sitting on the ')' itself (col=3) — should still find the pair
        let chars: Vec<char> = "(hi)".chars().collect();
        assert_eq!(find_surround_on_line(&chars, 3, '(', ')'), Some((0, 3)));
    }

    #[test]
    fn find_surround_multichar_line() {
        // "((foo))" with cursor at col=3 (inner 'o') — finds innermost pair (1,5)
        let chars: Vec<char> = "((foo))".chars().collect();
        assert_eq!(find_surround_on_line(&chars, 3, '(', ')'), Some((1, 5)));
    }
}
