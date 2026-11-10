# Web clipper

This specification defines the browser import, as
[decision ZIB0020](../decisions/ZIB0020-clip-web-articles.md) describes.

## Import request

The instance request `import-web-clip` has an `id`, a `clip` map, and an `images` list. The `clip`
map contains:

- **`authors`:** an optional list of up to 64 nonempty author names, each up to 1,024 bytes.
- **`body`:** a list of markdown strings and image maps. An image map has `image`, a zero-based
  index into `images`, and `alt`, its alternative text, up to 4,096 bytes. Each image has at least
  one reference. The combined size of the strings and image labels, plus 32 bytes per image
  reference, is at most 1,048,576 bytes. The body has text or an image reference.
- **`captured-at`:** the capture time, in milliseconds since the Unix epoch.
- **`note`:** the article's new identifier.
- **`published-on`:** an optional date as the space document specification defines it.
- **`space`:** the destination space's identifier.
- **`title`:** a nonempty title as text, up to 4,096 bytes.
- **`url`:** the captured HTTP or HTTPS address as text, up to 8,192 bytes. It has a host and omits
  credentials.

Each entry of `images` has `note`, the new image note identifier, and `bytes`, a byte string. The
list has at most 64 entries. An image has at most 8,388,608 bytes and the list has at most
33,554,432 bytes of image data. The instance accepts Graphics Interchange Format (GIF), Joint
Photographic Experts Group (JPEG), Portable Network Graphics (PNG), and WebP images. All note
identifiers are distinct version 7 identifiers as the identifiers specification defines them.

The instance creates an `article` note and the referenced `image` notes in the destination space. It
formats the title as a heading and replaces each image reference with an embed that points to its
image note. It writes the blobs and saves the space before returning the note identifiers. The
caller uses the same request to retry an import after a disconnect. If the article already exists on
the machine, the instance returns its current space and the same identifiers. A deleted article or
an image identifier already in use for a new import returns an error.

The extension escapes literal dollar signs in article text so they don't start or end math spans.

### Import request for videos

The importer also accepts YouTube video pages in the same request. It creates a `video` note for
`youtube.com` or its subdomains with a `/watch?v=ID` or `/shorts/ID` address, or a `youtu.be/ID`
address. The identifier contains one or more letters from `A` to `Z` or `a` to `z`, digits, hyphens,
or underscores. The short and share paths allow a trailing slash. The same rules for metadata,
images, and retries apply. A retry can also return an existing `article` note if the identifier and
source URL match a previous import of the video.

Firefox tries to include a timestamped transcript as markdown in the body. When it can't read a
transcript, it saves the available video details and reports the missing transcript on the toolbar
badge. When the body would be empty, Firefox uses a link to the video.

## Import request for PDFs

The instance request `import-web-pdf` has an `id` and a `pdf` map. The map contains:

- **`bytes`:** a byte string starting with `%PDF-` and holding at most 33,554,432 bytes.
- **`name`:** a nonempty filename of at most 4,096 bytes without control characters.
- **`note`:** the file note's new version 7 identifier.
- **`space`:** the destination space's identifier.
- **`url`:** the source address with the same rules as an article's `url`.

The instance stores the bytes as an `application/pdf` blob and creates a `file` note that holds the
blob in its `file` field. The note's first line holds the filename, followed by a source link in a
separate paragraph. The instance saves the space before returning its identifiers. A retry returns
the same file note in its current space. A deleted note or a note of another type returns an error.

### Source fields for PDFs

The importer writes the request's `url` into the note's `url` field and reads `captured-at` from the
millisecond timestamp in the note's version 7 identifier. Firefox sets that timestamp when the
capture starts. The importer fills `authors` from the embedded author names and `published-on` from
an embedded publication date when it can read those values. PDF creation and modification timestamps
leave `published-on` empty. When the importer writes the URL to the field, the new note's body holds
only the filename and a blank line. A retry leaves the existing note's body and fields intact.

## Firefox native messaging

Firefox sends one UTF-8 JavaScript Object Notation (JSON) message per host invocation with the
four-byte length that Firefox defines. The host accepts these messages:

- **`import-web-clip`:** a map with `type` set to `import-web-clip` and the same `clip` and `images`
  keys as the instance request. Each image has `data`, a Base64 string, in place of `bytes`. Success
  returns `ok` set to true and the result's `clip`.
- **`import-web-pdf`:** a map with `type` set to `import-web-pdf` and the keys of the instance
  request's `pdf` map. The `data` key holds a Base64 string in place of `bytes`. Success returns
  `ok` set to true and the result's `clip`.
- **`spaces`:** a map with `type` set to `spaces`. Success returns `ok` set to true and a `spaces`
  list with entries that hold `id` and `name`.

A request has at most 50,331,648 bytes. Failure returns `ok` set to false and an `error` string. The
host replies in the same framing, with at most 1,048,576 bytes.

## Platforms

- **Firefox on Linux.** The native host uses the local instance. The development host requires
  `ZIB_DEV_ROOT` and a development build of the extension.
