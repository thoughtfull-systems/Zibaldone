//! Collapse two concurrent replacements of the same Markdown checkbox character.

use loro::{Frontiers, LoroDoc, cursor::Side};
use pulldown_cmark::{Event, Parser, Tag};

use crate::space::note_body;
use crate::{Id, Result, Space, body};

pub(super) fn reconcile(space: &Space, id: Id) -> Result<bool> {
    let text = space.body(id)?;
    let source = text.to_string();
    if !source.contains("[xx") && !source.contains("[  ") {
        return Ok(false);
    }

    let mut duplicates = Vec::new();
    for (event, range) in Parser::new_ext(&source, body::options()).into_offset_iter() {
        if !matches!(event, Event::Start(Tag::Item)) {
            continue;
        }
        let item = &source[range.clone()];
        let Some(separator) = item.find([' ', '\t']) else { continue };
        let rest = item[separator..].trim_start_matches([' ', '\t']);
        let Some((inside, _)) =
            rest.strip_prefix('[').and_then(|box_text| box_text.split_once(']'))
        else {
            continue;
        };
        if inside.len() < 2
            || !(inside.bytes().all(|b| b == b'x') || inside.bytes().all(|b| b == b' '))
        {
            continue;
        }
        let start = range.start + item.len() - rest.len() + 1;
        let first = source[..start].chars().count();
        let cursors: Option<Vec<_>> = (first..first + inside.len())
            .map(|index| text.get_cursor(index, Side::Middle))
            .collect();
        let Some(cursors) = cursors else { continue };
        if cursors.iter().all(|cursor| was_valid_marker(space.doc(), id, cursor)) {
            duplicates.push((first + 1, inside.len() - 1));
        }
    }
    for &(start, count) in duplicates.iter().rev() {
        text.delete(start, count)?;
    }
    Ok(!duplicates.is_empty())
}

fn was_valid_marker(doc: &LoroDoc, id: Id, cursor: &loro::cursor::Cursor) -> bool {
    let Some(operation) = cursor.id else { return false };
    let Ok(version) = doc.fork_at(&Frontiers::from_id(operation)) else { return false };
    let Ok(text) = note_body(&version, id) else { return false };
    let Ok(position) = version.get_cursor_pos(cursor) else { return false };
    let source = text.to_string();
    let Some((byte, _)) = source.char_indices().nth(position.current.pos) else { return false };
    Parser::new_ext(&source, body::options())
        .into_offset_iter()
        .any(|(event, range)| matches!(event, Event::TaskListMarker(_)) && range.start + 1 == byte)
}
