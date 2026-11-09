# Instance

This specification defines how the one running instance of Zib on a Linux machine claims the
machine, and how the `zib` command reaches it. It follows
[decision ZIB0003](../decisions/ZIB0003-space-identity-and-machine-settings.md). The messages use
the framing that the [sync specification](sync.md#messages) defines.

## Files

The instance keeps two files in a directory of its own, `zibaldone` inside `$XDG_RUNTIME_DIR`, or
`/tmp/zibaldone-<uid>` when that variable isn't set, where `<uid>` is the user's numeric identifier.
The directory belongs to the user, and only the user may read, write, or enter it.

- **`instance.lock`:** an empty file that the instance holds an exclusive lock on, through `flock`,
  for as long as it runs. The operating system releases the lock when the process ends, however it
  ends.
- **`instance.sock`:** the Unix socket the instance listens on. Only the user may read or write it.

## Isolated development instances

The development build reads the environment variable `ZIB_DEV_ROOT`, as
[decision ZIB0015](../decisions/ZIB0015-isolated-development-instances.md) describes. When it's set
and not empty, the instance and the `zib` command keep their files under that directory, made
absolute from the current directory, instead of the base directories: data in `data`, the cache in
`cache`, and the lock and socket in `run`. The `run` directory takes the place of the `zibaldone`
directory under `$XDG_RUNTIME_DIR`, so each root runs an instance of its own. The window's title and
the tray icon's title and tooltip add the root's last path component to the name, such as "Zibaldone
Dev (a)". The production build never reads the variable.

## Starting

The app's own executable only ever runs the instance. People and launchers start Zib through the
`zib` command, which starts the instance when none runs.

When the instance starts, it creates the directory if it's missing and tries to take the lock
without waiting.

- **It gets the lock.** It's the instance. It removes any `instance.sock` that a previous instance
  left behind, listens on a new one, and only then opens spaces.
- **Another process holds the lock.** An instance already runs, so it exits.

The instance accepts a connection only from a process of the same user, which it checks with the
socket's peer credentials, and closes any other.

## Windows and the tray

Once it starts, the instance keeps running until the owner quits it. Closing its window doesn't end
it.

- **No window at first.** The instance starts with an icon in the system tray and no window. It
  opens its window only for `activate`, or when the owner chooses the tray icon. Other requests,
  such as a search, return their results without a window.
- **Closing the window.** Closing the window leaves the instance running, with its tray icon.
- **Quitting.** The instance quits when the owner presses Ctrl+Q in its window, chooses Quit from
  the tray icon's menu, or sends `stop` through the `zib` command. Before it exits, it syncs what it
  can within 10 seconds. It saves its spaces first, then exchanges each space only with the members
  that lack a change made on the machine and whose last exchange succeeded, and exits as soon as
  those exchanges finish.
- **No display.** On a machine without a display, such as a small server, the instance runs with
  neither a tray icon nor a window, and quits only on `stop` or when the system stops it.
- **Tray icon.** The icon uses the StatusNotifierItem interface over D-Bus, which Waybar's tray
  shows. The owner's desktop, sway on Wayland with Waybar, is the one Linux desktop the app targets.

## The last open note

The machine's `machine.json` configuration can hold `last-note`, an object with `note` and `space`.
Both values are canonical identifiers as text. The instance records this object after the window
opens a note. This setting stays on the machine and uses the build channel's data directory.

When the owner opens an empty window without a note URI, the window reopens the recorded note if
this machine still holds it. A deleted note or an unavailable space leaves the window empty. An
explicit request to open a note takes precedence over restoration. Closing the window keeps the
recorded note for the next opening.

## The `zib` command

The `zib` command never opens a space or the machine store itself. For each command, it:

- connects to `instance.sock`;
- if nothing listens there, starts the instance, and tries again for up to 10 seconds;
- sends `hello`, then its request, and prints the reply.

With no command, `zib` sends `activate`, so running `zib` alone starts Zib if needed and opens its
window. Given a `zib:` URI and nothing else, it sends `activate` with that URI. The desktop entry
runs `zib`, and registers `zib %u` as the handler of the `zib` scheme.

`zib search --space SPACE WORDS...` sends `search-space`. `zib search --json` prints a JavaScript
Object Notation (JSON) array of hits and removes the match markers from each `snippet`. Search
options precede the query. A `--` ends option parsing.

`zib get NOTE...` sends `get-notes` and prints the `notes` array as JSON. A failed entry carries its
own error. `zib get --raw NOTE` prints the body of one note verbatim and exits with an error if the
note is unavailable. `zib write NOTE --expect ORIGINAL` reads the expected body from the named file
and the replacement from standard input, both as UTF-8. It sends `write-note` and prints nothing on
success. Both commands accept note identifiers and `zib:note/` URIs.

## Messages

Each connection carries requests from the client and replies from the instance, as frames of Concise
Binary Object Representation (CBOR) maps.

- **`hello`.** The client sends `hello` first, with `features`, a list of text strings, and the
  instance replies `hello`, with its own `features`.
- **Requests.** Each request has `type`, the request's name, and `id`, an integer the client picks,
  unique on the connection, plus its own arguments.
- **Replies.** The instance answers each request with `result`, with the same `id` and the request's
  results, or with `error`, with the same `id`, a text `reason`, and a text `code` that names the
  kind of failure, such as `not-found`. Replies may arrive in any order.

The first requests:

- **`activate`,** with an optional `uri`: brings the instance's window to the front, or opens one,
  and opens the URI if there is one, such as a `zib:note/` URI from the
  [identifiers specification](identifiers.md#uris).
- **`stop`:** quits the instance, as the tray menu's Quit does.

The commands that the `zib` command offers, such as searching, creating notes, or printing, each add
a request here, as the core's operations become available to it.

- **`spaces`:** lists the machine's spaces. The result holds `spaces`, a list of maps, each with
  `id`, the space's identifier as text, and `name`, its pet name.
- **`create-space`,** with `name`, a text pet name: creates a space. The result holds `spaces`, with
  the new space alone.
- **`search`,** with `query`, the text to find, and an optional `limit`, an integer that defaults to
  20: searches every space. The result holds `hits`, a list of maps, best matches first, each with
  `space` and `note`, identifiers as text, `title`, the note's title as plain text, and `snippet`,
  words around the matches, with each match between the characters U+0002 and U+0003.
- **`print`,** with `notes`, a list of note identifiers as text, `linked`, a Boolean that defaults
  to false, and `path`, the absolute path of a file: prints the notes, in order, to a PDF at that
  path, each note starting on a new page. With `linked`, each note comes with the notes it links to,
  in the order its links first name them, unless the PDF already holds them. The result holds
  nothing, and the error code is `not-found` when this machine holds none of a note.

A result leaves out a list that would be empty.

The note commands add these requests:

- **`get-notes`,** with `notes`, a list of note identifiers as text. The result holds `notes`, with
  one map per requested note in input order. A successful entry has `body` and `title` as text and
  `note` and `space` as identifiers. A failed entry has `note` and a text `error`.
- **`search-space`,** with the same `query` and `limit` as `search`, plus `space`, a space
  identifier or exact pet name as text. An identifier that the machine holds takes precedence over
  names. A name that matches more than one space returns an error. The result holds the same fields
  as `search` and contains only hits from the chosen space.
- **`write-note`,** with `note`, an identifier, and `body` and `expected`, both text. The instance
  replaces the note's body when it matches `expected`. An already matching replacement also
  succeeds. A different body returns the error code `conflict`. The write preserves the note's
  identifier, type, and fields. It saves the space, updates the index, and schedules sync. A
  successful result holds nothing.

These requests access the machine's spaces regardless of which spaces show in the note list.

## Platforms

- **Linux.** This specification applies to the Linux app and the `zib` command, on sway with a
  Waybar tray. Other desktops may work, but nothing here plans for them.
- **Android.** The app runs as one process, so it needs no lock or socket.
- **Browser.** A web app, when one exists, takes the lock through the Web Locks interface and serves
  other tabs over a broadcast channel, as decision ZIB0003 describes. A later addition defines its
  messages.
