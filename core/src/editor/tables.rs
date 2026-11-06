//! Navigation through the editable cells of list tables and tables nested in their cells.

use std::ops::Range;

use crate::body::ListTable;
use crate::editing::{self, Drawing};

#[derive(Clone)]
struct Path {
    column: usize,
    nested: Vec<(usize, usize, usize)>,
    row: usize,
}

struct Navigator<'a> {
    block: Range<usize>,
    body: &'a str,
    groups: Vec<Vec<Range<usize>>>,
}

impl Navigator<'_> {
    fn visit(&mut self, table: &ListTable, parent: Option<(&Path, usize)>) {
        let mut cells = Vec::new();
        for (row, columns) in table.rows.iter().enumerate() {
            for (column, text) in columns.iter().enumerate() {
                let path = parent.map_or_else(
                    || Path { row, column, nested: Vec::new() },
                    |(path, index)| {
                        let mut path = path.clone();
                        path.nested.push((index, row, column));
                        path
                    },
                );
                if let Some(range) = editing::table_cell_in_body(
                    self.body,
                    self.block.clone(),
                    path.row,
                    path.column,
                    &path.nested,
                ) {
                    cells.push(range);
                }
                let nested = editing::drawings(text).into_iter().filter_map(|drawn| {
                    if let Drawing::Table(table) = drawn.drawing { Some(table) } else { None }
                });
                for (index, inner) in nested.enumerate() {
                    self.visit(&inner, Some((&path, index)));
                }
            }
        }
        self.groups.push(cells);
    }
}

/// Stays within the innermost table that contains the selection. Boundary cells stay selected.
/// Pipe tables outside list-table fences use `pipe_tables::table_edit`, which also aligns them.
pub(super) fn next_cell(body: &str, selection: &Range<usize>, back: bool) -> Option<Range<usize>> {
    let drawn = editing::drawings(body).into_iter().find(|d| {
        matches!(d.drawing, Drawing::Table(_))
            && d.range.start <= selection.start
            && selection.end <= d.range.end
    })?;
    let Drawing::Table(table) = drawn.drawing else { return None };
    let mut nav = Navigator { body, block: drawn.range, groups: Vec::new() };
    nav.visit(&table, None);
    let (group, index, _) = nav
        .groups
        .iter()
        .enumerate()
        .flat_map(|(group, cells)| {
            cells.iter().enumerate().filter_map(move |(index, cell)| {
                (cell.start <= selection.start && selection.end <= cell.end).then_some((
                    group,
                    index,
                    cell.len(),
                ))
            })
        })
        .min_by_key(|(_, _, length)| *length)?;
    let cells = &nav.groups[group];
    let index = if back { index.saturating_sub(1) } else { (index + 1).min(cells.len() - 1) };
    Some(cells[index].clone())
}
