//! Lines in readable columns, as `rosie check` prints them (`docs/spec/cli.md#check`).

/// One printed line: `N` cells in columns, then free text that may span several lines.
pub struct Row<const N: usize> {
    pub cells: [String; N],
    pub text: String,
}

/// The rows in columns: each column as wide as its widest cell, two spaces apart. The
/// text's later lines start at the text column.
pub fn layout<const N: usize>(rows: &[Row<N>]) -> Vec<String> {
    let widths: [usize; N] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row.cells[column].chars().count())
            .max()
            .unwrap_or(0)
    });
    let text_column: usize = widths.iter().map(|width| width + 2).sum();
    let text_break = format!("\n{}", " ".repeat(text_column));

    rows.iter()
        .map(|row| {
            let cells: String = row
                .cells
                .iter()
                .zip(widths)
                .map(|(cell, width)| format!("{cell:<width$}  "))
                .collect();
            let line = format!("{cells}{}", row.text.replace('\n', &text_break));
            line.trim_end().to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: [&str; 2], text: &str) -> Row<2> {
        Row {
            cells: cells.map(str::to_owned),
            text: text.to_owned(),
        }
    }

    #[test]
    fn columns_are_as_wide_as_their_widest_cell() {
        let rows = [
            row(["clean", "~/a"], "x"),
            row(["outside-roots", "~/bb"], "y"),
        ];

        assert_eq!(
            layout(&rows),
            ["clean          ~/a   x", "outside-roots  ~/bb  y"]
        );
    }

    #[test]
    fn later_text_lines_start_at_the_text_column() {
        let rows = [row(["keep", "~/a"], "first\n  second")];

        assert_eq!(layout(&rows), ["keep  ~/a  first\n             second"]);
    }

    #[test]
    fn a_line_ends_at_its_last_printed_character() {
        let rows = [row(["skipped", ""], ""), row(["clean", "~/a"], "x")];

        assert_eq!(layout(&rows)[0], "skipped");
    }
}
