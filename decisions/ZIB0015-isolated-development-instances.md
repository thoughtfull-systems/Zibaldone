# ZIB0015 Isolated development instances

## Context

Sync works between machines, and testing it needs more than one. Running a few development instances
on one computer stands in for a few machines, and makes sync quick to try.

[Decision ZIB0013](ZIB0013-development-and-production-channels.md) gives each channel its own
directories under the X Desktop Group (XDG) base directories, and the
[instance specification](../specs/instance.md) runs one instance per set of directories, which it
claims with a lock and a socket. A second instance therefore needs directories of its own. Setting
`XDG_DATA_HOME`, `XDG_CACHE_HOME`, and `XDG_RUNTIME_DIR` gives it them, but moving `XDG_RUNTIME_DIR`
also moves where Wayland, D-Bus helpers, and the desktop's settings store look for their sockets,
and the instances look alike on screen.

Decision ZIB0013 rejected a variable for the data directory, because a forgotten one would run a
development change on production data.

## Criteria

- **Production untouched.** Nothing here can make any build read or write production data, or make
  the production build behave differently.
- **One setting.** Starting another instance takes one variable, set the same way for the instance
  and the `zib` command.
- **No side effects.** The setting moves Zib's own files and nothing else in the desktop session.
- **Obvious.** The owner can tell the instances apart at a glance.

## Decision

The development build reads the environment variable `ZIB_DEV_ROOT`. When it's set and not empty,
the instance and the `zib` command keep every file under that directory instead of the XDG base
directories: data in `data`, the cache in `cache`, and the lock and socket in `run`. A relative path
counts from the current directory. The production build never reads the variable.

An instance with a root shows the root's last path component after its name, in the window title and
the tray icon's tooltip, such as "Zibaldone Dev (a)".

To run two instances, the owner sets a different root for each, such as `ZIB_DEV_ROOT=/tmp/zib/a`
and `ZIB_DEV_ROOT=/tmp/zib/b`, and pairs them like any two machines.

Against the criteria:

- **Production untouched:** the production build doesn't look at the variable, and a forgotten one
  in the development build only chooses between development directories.
- **One setting:** one variable moves all three directories.
- **No side effects:** the variable means nothing to other programs, so the desktop session keeps
  its own runtime directory.
- **Obvious:** each instance's title and tooltip name its root.

## Rejected alternatives

- **The XDG variables alone.** They need no code, but they take three variables and a fourth for
  Wayland's socket, and they move other programs' runtime files. They fail one setting and no side
  effects.
- **A command-line option.** The `zib` command starts the instance itself, so it would have to pass
  the option on, and every call of `zib` for that instance would need it too. A variable reaches
  both by inheritance. It does no better than a variable on any criterion and adds plumbing.
- **A distinct app identifier and color for each instance.** It would set them apart further, but
  the identifier shows up in D-Bus and desktop files, which the instance doesn't install for a root.
  The name in the title meets the obvious criterion without it.

## Consequences

- The instance specification gains a section on the variable.
- The root's socket path must fit within the limit of about 100 bytes for a Unix socket path, so
  roots belong in short paths.
- Instances with roots still share the development build's app identifier and icon.
