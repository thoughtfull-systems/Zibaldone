# Note fields

This inventory follows [ZIB0026][decision]. Date names end in `-on`, and timestamp names end in
`-at`. Dates use `YYYY-MM-DD`; timestamps use integer milliseconds since the Unix epoch.

## Built-in fields

These names live in each note's `fields` map. Every field is optional. Source fields apply to
articles, files, images, and videos. File references hold blob hashes, including extracted text.

| Name           | Kind         | Note types                   |
| -------------- | ------------ | ---------------------------- |
| `authors`      | List of text | Article, file, image, video  |
| `captured-at`  | Timestamp    | Article, file, image, video  |
| `completed-on` | Date         | Task                         |
| `due-on`       | Date         | Project, task                |
| `file`         | File         | File                         |
| `image`        | File         | Image                        |
| `parent-id`    | Link         | Project, task                |
| `published-on` | Date         | Article, file, image, video  |
| `scheduled-on` | Date         | Project, task                |
| `text`         | File         | File, derived extracted text |
| `thumbnail`    | File         | File, image, derived preview |
| `url`          | Text         | Article, file, image, video  |

The `agent-transcript` and `note` types define no fields. The [built-in types][types] and
[projects and tasks][work] specifications define field meanings, search modes, and inheritance.

## Note metadata and content

Every note has an identifier, type, markdown body, and metadata. These values live outside `fields`.

| Name         | Kind               | Storage                       |
| ------------ | ------------------ | ----------------------------- |
| `body`       | Markdown           | Note's `body` text container  |
| `created-at` | Timestamp          | Note's `meta` map             |
| `created-by` | Machine identifier | Note's `meta` map             |
| `id`         | Note identifier    | Note's key in the `notes` map |
| `type`       | Text               | Note's `type` key             |
| `updated-at` | Timestamp          | Note's `meta` map             |
| `updated-by` | Machine identifier | Note's `meta` map             |

The metadata popup shows `created-at` and `updated-at` in local time. Both names also support search
and field-name completion for every note type. They use the existing timestamp search syntax. Fields
named `file`, `text`, and `thumbnail` stay in storage and keep their search behavior, but the popup
doesn't show them.

## Derived values and ordering

The title comes from the body's first line that isn't blank; it isn't a separate stored field. The
virtual search field `project` identifies ancestor projects. The space's `work-order` movable list
stores sibling presentation order. Neither value lives in a note's `fields` map.

## Deleted notes

A deletion record in `tombstones` holds `title` as markdown, `deleted-at` as a timestamp, and
`deleted-by` as a machine identifier. These records preserve deleted notes' link labels.

[decision]: ../decisions/ZIB0026-name-date-and-timestamp-fields.md
[types]: built-in-types.md
[work]: projects-and-tasks.md
