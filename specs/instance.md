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
  can within 10 seconds.
- **No display.** On a machine without a display, such as a small server, the instance runs with
  neither a tray icon nor a window, and quits only on `stop` or when the system stops it.
- **Tray icon.** The icon uses the StatusNotifierItem interface over D-Bus, which Waybar's tray
  shows. The owner's desktop, sway on Wayland with Waybar, is the one Linux desktop the app targets.

## The `zib` command

The `zib` command never opens a space or the machine store itself. For each command, it:

- connects to `instance.sock`;
- if nothing listens there, starts the instance, and tries again for up to 10 seconds;
- sends `hello`, then its request, and prints the reply.

With no command, `zib` sends `activate`, so running `zib` alone starts Zib if needed and opens its
window. Given a `zib:` URI and nothing else, it sends `activate` with that URI. The desktop entry
runs `zib`, and registers `zib %u` as the handler of the `zib` scheme.

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

## Platforms

- **Linux.** This specification applies to the Linux app and the `zib` command, on sway with a
  Waybar tray. Other desktops may work, but nothing here plans for them.
- **Android.** The app runs as one process, so it needs no lock or socket.
- **Browser.** A web app, when one exists, takes the lock through the Web Locks interface and serves
  other tabs over a broadcast channel, as decision ZIB0003 describes. A later addition defines its
  messages.
