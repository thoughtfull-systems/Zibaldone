{ pkgs }:

let
  fonts = pkgs.makeFontsConf {
    fontDirectories = [
      # keep-sorted start
      pkgs.adwaita-fonts
      pkgs.dejavu_fonts
      pkgs.noto-fonts-cjk-sans
      # keep-sorted end
    ];
  };
  text = ''
    export XDG_RUNTIME_DIR
    XDG_RUNTIME_DIR=$(mktemp -d /tmp/zib-test.XXXXXX)
    unset DISPLAY SWAYSOCK WAYLAND_DISPLAY WAYLAND_SOCKET
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
    export FONTCONFIG_FILE=${fonts} GDK_BACKEND=wayland GSETTINGS_BACKEND=memory
    export GSK_RENDERER=cairo GTK_A11Y=none
    gtkSchemas=${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}
    export XDG_DATA_DIRS="$gtkSchemas''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"

    children=()
    trap '
      kill "''${children[@]}" 2>/dev/null || true
      wait "''${children[@]}" 2>/dev/null || true
      rm -rf "$XDG_RUNTIME_DIR"
    ' EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM

    # Let each test choose its window size on an output with room for the widest test window.
    cat > "$XDG_RUNTIME_DIR/sway.conf" <<'SWAY'
    for_window [shell="xdg_shell"] floating enable
    output * mode --custom 1920x1080
    xwayland disable
    SWAY
    WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman \
      sway --config "$XDG_RUNTIME_DIR/sway.conf" > "$XDG_RUNTIME_DIR/sway.log" 2>&1 &
    compositor=$!
    children+=("$compositor")

    # A fresh runtime directory leaves Sway's first Wayland socket available.
    export WAYLAND_DISPLAY=wayland-1
    ipc="$XDG_RUNTIME_DIR/sway-ipc.$UID.$compositor.sock"
    for _ in {1..250}; do
      [[ -S $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY && -S $ipc ]] && break
      kill -0 "$compositor" 2>/dev/null || break
      sleep 0.02
    done
    if [[ ! -S $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY || ! -S $ipc ]]; then
      cat "$XDG_RUNTIME_DIR/sway.log" >&2
      echo "Could not start the test display" >&2
      exit 1
    fi

    # A seat needs a keyboard to give a GTK window input focus. This one stays idle.
    wtype -s 3600000 &
    keyboard=$!
    children+=("$keyboard")
    for _ in {1..250}; do
      if swaymsg -s "$ipc" -t get_inputs -r |
        jq -e 'any(.[]; .type == "keyboard")' > /dev/null; then
        export ZIB_TEST_DISPLAY=1
        "$@"
        exit 0
      fi
      kill -0 "$keyboard" 2>/dev/null || break
      sleep 0.02
    done
    echo "Could not attach a virtual keyboard to the test display" >&2
    exit 1
  '';
in
pkgs.writeShellApplication {
  # keep-sorted start block=yes
  inherit text;
  name = "zib-test-display";
  runtimeInputs = [
    # keep-sorted start
    pkgs.coreutils
    pkgs.jq
    pkgs.sway-unwrapped
    pkgs.wtype
    # keep-sorted end
  ];
  # keep-sorted end
}
