# The production and development builds of Zibaldone for Linux, and the tests, as decision ZIB0014
# describes. Crane builds the dependencies once, and every package reuses them.
{ craneLib, pkgs }:

let
  inherit (pkgs) lib;
  root = ../.;
  common = {
    # keep-sorted start block=yes
    buildInputs = [
      # keep-sorted start
      pkgs.gtk4
      pkgs.gtksourceview5
      pkgs.libadwaita
      pkgs.sqlite
      # keep-sorted end
    ];
    # mold links far faster than the default linker.
    env.RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
    inherit ((lib.importTOML (root + "/Cargo.toml")).workspace.package) version;
    nativeBuildInputs = [
      # keep-sorted start
      pkgs.mold
      pkgs.pkg-config
      # keep-sorted end
    ];
    pname = "zibaldone";
    src = lib.fileset.toSource {
      inherit root;
      fileset = lib.fileset.unions [
        # keep-sorted start
        (root + "/Cargo.lock")
        (root + "/Cargo.toml")
        (root + "/core")
        (root + "/linux/Cargo.toml")
        (root + "/linux/src")
        # keep-sorted end
      ];
    };
    strictDeps = true;
    # keep-sorted end
  };
  cargoArtifacts = craneLib.buildDepsOnly common;
  data = root + "/linux/data";
  # One channel's package: its commands, desktop entry, and icon, named so both install together.
  channel =
    {
      appId,
      features,
      pname,
      suffix,
    }:
    let
      # Cargo names the executables the same in every channel, so the development build renames
      # them.
      rename = lib.optionals (suffix != "") [
        "mv $out/bin/zib $out/bin/zib${suffix}"
        "mv $out/bin/zibaldone $out/bin/zibaldone${suffix}"
      ];
    in
    craneLib.buildPackage (
      common
      // {
        # keep-sorted start block=yes
        cargoExtraArgs = "--package zib-linux ${features}";
        doCheck = false;
        inherit cargoArtifacts;
        inherit pname;
        meta = {
          # keep-sorted start
          description = "A personal, local-first notebook";
          mainProgram = "zib${suffix}";
          platforms = lib.platforms.linux;
          # keep-sorted end
        };
        nativeBuildInputs = common.nativeBuildInputs ++ [ pkgs.wrapGAppsHook4 ];
        postInstall = lib.concatLines (
          rename
          ++ [
            "install -Dm644 ${data}/${appId}.desktop -t $out/share/applications"
            "install -Dm644 ${data}/icons/${appId}.svg -t $out/share/icons/hicolor/scalable/apps"
          ]
        );
        # The app reads PDFs with the command-line tools from poppler.
        preFixup = "gappsWrapperArgs+=(--prefix PATH : ${lib.makeBinPath [ pkgs.poppler-utils ]})";
        # keep-sorted end
      }
    );
in
{
  # keep-sorted start block=yes newline_separated=yes
  tests = craneLib.cargoNextest (
    common
    // {
      inherit cargoArtifacts;
      cargoNextestExtraArgs = "--workspace";
    }
  );

  zibaldone = channel {
    appId = "systems.thoughtfull.Zibaldone";
    features = "--features production";
    pname = "zibaldone";
    suffix = "";
  };

  zibaldone-dev = channel {
    appId = "systems.thoughtfull.Zibaldone.Devel";
    features = "";
    pname = "zibaldone-dev";
    suffix = "-dev";
  };
  # keep-sorted end
}
