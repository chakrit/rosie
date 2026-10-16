//! Lines in readable columns, as `rosie check` and the plan table print them
//! (`docs/spec/cli.md#check`, `docs/spec/plan.md#plan-table`).

/// Which side of its column a cell keeps to.
#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Right,
}

/// One printed line: `N` cells in columns, then free text that may span several lines.
pub struct Row<const N: usize> {
    pub cells: [String; N],
    pub text: String,
}

/// The rows in columns: each column as wide as its widest cell, two spaces apart. The
/// text's later lines start at the text column.
pub fn layout<const N: usize>(aligns: [Align; N], rows: &[Row<N>]) -> Vec<String> {
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
                .zip(aligns.iter().zip(widths))
                .map(|(cell, (align, width))| match align {
                    Align::Left => format!("{cell:<width$}  "),
                    Align::Right => format!("{cell:>width$}  "),
                })
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
    fn columns_are_as_wide_as_their_widest_cell_and_keep_their_side() {
        let rows = [
            row(["clean", "1.2 GB"], "a"),
            row(["outside-roots", "?"], "b"),
        ];

        assert_eq!(
            layout([Align::Left, Align::Right], &rows),
            ["clean          1.2 GB  a", "outside-roots       ?  b"]
        );
    }

    #[test]
    fn later_text_lines_start_at_the_text_column() {
        let rows = [row(["keep", "1 B"], "first\n  second")];

        assert_eq!(
            layout([Align::Left, Align::Right], &rows),
            ["keep  1 B  first\n             second"]
        );
    }

    #[test]
    fn a_line_ends_at_its_last_printed_character() {
        let rows = [row(["report", ""], ""), row(["clean", "1.2 GB"], "a")];

        assert_eq!(layout([Align::Left, Align::Left], &rows)[0], "report");
    }
}
