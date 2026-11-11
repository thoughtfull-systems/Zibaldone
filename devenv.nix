{
  config,
  lib,
  pkgs,
  ...
}:

let
  gtkSchemas = "${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}";
  rustcWrapper = pkgs.writeShellScript "zib-rustc" ''
    cargo_home="''${CARGO_HOME:-$HOME/.cargo}"
    cargo_home="''${cargo_home%/}"
    case "''${CARGO_MANIFEST_DIR:-}" in
      "$cargo_home"/git/checkouts/* | "$cargo_home"/registry/src/*)
        export SCCACHE_IGNORE_SERVER_IO_ERROR=1
        exec ${pkgs.coreutils}/bin/nice ${lib.getExe pkgs.sccache} "$@"
        ;;
    esac
    exec ${pkgs.coreutils}/bin/nice "$@"
  '';
  vendored = "^\\.config/vale/styles/(Google|write-good)/";
in
{
  # keep-sorted start block=yes newline_separated=yes
  cachix.enable = false;

  # Nix builds wrap GTK apps to find GSettings schemas; apps run from the shell need them here.
  enterShell = "export XDG_DATA_DIRS=${gtkSchemas}:$XDG_DATA_DIRS";

  env = {
    # keep-sorted start block=yes
    # Eight compilers at once fill memory and push the desktop into swap, so build with four.
    CARGO_BUILD_JOBS = "4";
    # mold links far faster than the default linker, which dominates incremental builds and tests.
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
    # Share compiled dependencies across worktrees and keep local builds incremental.
    RUSTC_WRAPPER = "${rustcWrapper}";
    VALE_CONFIG_PATH = "${config.devenv.root}/.config/vale/vale.ini";
    ZIB_SPELL_DICTIONARY_DIR = "${pkgs.hunspellDicts.en-us}/share/hunspell";
    # keep-sorted end
  };

  git-hooks.hooks = {
    # keep-sorted start block=yes
    actionlint.enable = true;
    biome = {
      enable = true;
      entry = "${lib.getExe pkgs.biome} check --config-path=.config/biome.json";
      files = "^firefox/.*\\.(html|json|mjs|ts)$";
      name = "biome";
      types = [ "text" ];
    };
    check-added-large-files.enable = true;
    check-case-conflicts.enable = true;
    check-executables-have-shebangs.enable = true;
    check-merge-conflicts.enable = true;
    check-shebang-scripts-are-executable.enable = true;
    check-symlinks.enable = true;
    check-vcs-permalinks.enable = true;
    clippy = {
      enable = true;
      settings = {
        # keep-sorted start
        denyWarnings = true;
        # keep-sorted end
      };
    };
    deadnix.enable = true;
    detect-private-keys.enable = true;
    editorconfig-checker = {
      enable = true;
      excludes = [
        vendored
        "^devenv\\.lock$"
      ];
    };
    end-of-file-fixer = {
      enable = true;
      # devenv writes the lock file without a trailing newline.
      excludes = [
        vendored
        "^devenv\\.lock$"
      ];
    };
    fix-byte-order-marker.enable = true;
    forbid-new-submodules.enable = true;
    gitlint = {
      enable = true;
      # --msg-filename ends the stock entry, so args can't add the config flag.
      entry = lib.mkForce (
        lib.concatStringsSep " " [
          (lib.getExe config.git-hooks.hooks.gitlint.package)
          "--config .config/gitlint --staged --msg-filename"
        ]
      );
    };
    keep-sorted = {
      # The flag replaces every default option, so it repeats them all with only case changed.
      args = [
        "--default-options"
        "allow_yaml_lists=yes case=no group=yes order=asc remove_duplicates=yes sticky_comments=yes"
      ];
      enable = true;
    };
    # No stock hook checks line length in every file type, and nixfmt doesn't fail on long lines.
    line-length = {
      enable = true;
      entry = lib.getExe (
        pkgs.writeShellApplication {
          name = "line-length";
          runtimeInputs = [ pkgs.gawk ];
          text = ''
            LC_ALL=C.UTF-8 gawk '
              length > 100 { printf "%s:%d: %d characters\n", FILENAME, FNR, length; bad = 1 }
              END { exit bad }
            ' "$@"
          '';
        }
      );
      excludes = [
        vendored
        "^devenv\\.lock$"
      ];
      name = "line-length";
      types = [ "text" ];
    };
    markdownlint = {
      enable = true;
      # The stock entry passes a config generated from Nix; use the file in .config instead.
      entry = lib.mkForce (
        lib.concatStringsSep " " [
          (lib.getExe config.git-hooks.hooks.markdownlint.package)
          "--config .config/markdownlint.yaml"
        ]
      );
      excludes = [ vendored ];
    };
    mdformat = {
      args = [
        "--wrap"
        "100"
      ];
      enable = true;
      excludes = [ vendored ];
      package = pkgs.mdformat.withPlugins (ps: [ ps.mdformat-gfm ]);
    };
    mixed-line-endings = {
      args = [ "--fix=lf" ];
      enable = true;
    };
    nixfmt = {
      enable = true;
      settings.width = 100;
    };
    ripsecrets.enable = true;
    rustfmt = {
      enable = true;
      settings.config-path = ".config/rustfmt.toml";
    };
    sort-file-contents = {
      enable = true;
      files = lib.concatStrings [
        "^(\\.gitignore"
        "|\\.config/vale/styles/config/vocabularies/[^/]+/(accept|reject)\\.txt)$"
      ];
      settings.ignore-case = true;
    };
    statix.enable = true;
    taplo = {
      args = [
        "--config"
        ".config/taplo.toml"
      ];
      enable = true;
    };
    trim-trailing-whitespace = {
      args = [ "--markdown-linebreak-ext=md" ];
      enable = true;
      excludes = [ vendored ];
    };
    typos = {
      enable = true;
      excludes = [ vendored ];
    };
    vale = {
      enable = true;
      excludes = [ vendored ];
      settings.configPath = ".config/vale/vale.ini";
      types = [ "markdown" ];
    };
    vale-config = {
      enable = true;
      entry = "${lib.getExe pkgs.vale} --config .config/vale/vale.ini ls-config";
      files = "^\\.config/vale/";
      name = "vale-config";
      pass_filenames = false;
    };
    yaml-extension = {
      enable = true;
      entry = lib.getExe (
        pkgs.writeShellApplication {
          name = "yaml-extension";
          text = ''
            status=0
            for file in "$@"; do
              case "$file" in
                .config/vale/*.yaml)
                  echo "$file: Vale ignores rule files unless they end in .yml"
                  status=1
                  ;;
                .config/vale/*) ;;
                *.yml)
                  echo "$file: use the .yaml extension"
                  status=1
                  ;;
              esac
            done
            exit "$status"
          '';
        }
      );
      files = "\\.ya?ml$";
      name = "yaml-extension";
    };
    yamlfmt = {
      enable = true;
      excludes = [ vendored ];
      settings = {
        configPath = ".config/yamlfmt.yaml";
        lint-only = false;
      };
    };
    yamllint = {
      enable = true;
      excludes = [ vendored ];
      settings.configPath = ".config/yamllint.yaml";
    };
    # keep-sorted end
  };

  languages.rust.enable = true;

  packages = [
    # keep-sorted start
    pkgs.cargo-nextest
    pkgs.gtk4
    pkgs.libadwaita
    pkgs.mold
    pkgs.nodejs
    pkgs.pkg-config
    pkgs.poppler-utils
    pkgs.sccache
    pkgs.sqlite
    pkgs.vale
    # keep-sorted end
  ];

  # `cargo run --bin zib` builds only `zib`, which starts the `zibaldone` beside it, so it would
  # start a stale instance. This builds both first.
  scripts.zib-dev.exec = ''
    set -e
    root=${lib.escapeShellArg config.devenv.root}
    cargo build --quiet --manifest-path "$root/Cargo.toml" --package zib-linux \
      --target-dir "$root/target"
    exec "$root/target/debug/zib" "$@"
  '';
  # keep-sorted end
}
