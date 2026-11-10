# ZIB0020 Capture articles from the web

## Context

The owner wants to save a page's main content, images, and source details with one click in Firefox.
Obsidian Web Clipper extracts page content through Defuddle, a separate library.

## Decision

Bundle Defuddle in a private Firefox extension. The extension reads the current page and downloads
the images that appear in the article. It sends the capture through Firefox Native Messaging to a
host that uses the same instance socket and startup path as the `zib` command. The extension has a
settings page for choosing the destination space and granting access to images from other sites. The
toolbar button reports the outcome.

Store the clip as an `article` note with a schema for its source details. Curio is a name the
interface can give works by other people. A future book can have its own type and fields. Store each
picture as an `image` note in the same space and embed it by its note identifier. This gives the
article the same storage, search, and sync behavior as other notes and images.

Save the article and its image notes as one import. The extension supplies the note identifiers and
reuses them when it retries a request after losing the reply. The instance saves the complete change
before replying.

Firefox's built-in Portable Document Format (PDF) viewer restricts page extraction. For a web PDF,
the extension downloads the original bytes and imports them as a `file` note with a source link. Zib
uses its existing file storage, text extraction, and thumbnails.

For new PDF imports, store the source URL in a field and use the filename as the body. Use the same
source fields for file notes, image notes, and articles. Record the PDF's capture time and read the
author names and publication date from its embedded metadata when available.

Capture a YouTube video as a `video` note with the same source fields as an article. Firefox tries
to retrieve the captions on the toolbar click and includes the transcript in the note body.

Nix builds a local extension package and registers the native host. Development uses its own host
and add-on identifiers and requires an isolated development root.

Distribute the signed production build as a GitHub Release asset. Use Mozilla's unlisted signing
channel for self-distribution.

## Consequences

- Firefox owns page extraction and image downloads. The native host receives the content and bytes.
- The space holds regular image notes. Moving an article keeps its links to their original image
  notes, as [decision ZIB0005](ZIB0005-note-identifiers-and-links.md) describes.
- The built-in types and instance specifications grow alongside the
  [web clipper specification](../specs/web-clipper.md).
