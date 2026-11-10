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
      hostName = "systems.thoughtfull.zibaldone" + lib.optionalString (suffix != "") ".dev";
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
            ''
              mkdir -p "$out/libexec" "$out/lib/mozilla/native-messaging-hosts"
              host="$out/libexec/zib-firefox-host"
              cat > "$host" <<EOF
              #!${pkgs.runtimeShell}
              exec "$out/bin/zib${suffix}" firefox-host
              EOF
              chmod +x "$host"
              cat > "$out/lib/mozilla/native-messaging-hosts/${hostName}.json" <<EOF
              {
                "allowed_extensions": ["web-clipper${suffix}@zibaldone.thoughtfull.systems"],
                "description": "Zib Web Clipper",
                "name": "${hostName}",
                "path": "$host",
                "type": "stdio"
              }
              EOF
            ''
          ]
        );
        # The app reads PDFs with the command-line tools from poppler.
        preFixup = "gappsWrapperArgs+=(--prefix PATH : ${lib.makeBinPath [ pkgs.poppler-utils ]})";
        # keep-sorted end
      }
    );
  clipper =
    channelName: pname:
    pkgs.buildNpmPackage {
      # keep-sorted start block=yes
      env.ZIB_CLIPPER_CHANNEL = channelName;
      inherit (lib.importJSON (root + "/firefox/package.json")) version;
      inherit pname;
      installPhase = lib.concatLines [
        "runHook preInstall"
        "mkdir -p \"$out\""
        "touch -d '1980-01-01 UTC' dist/*"
        "zip -X -j \"$out/${pname}.xpi\" dist/*"
        "runHook postInstall"
      ];
      nativeBuildInputs = [ pkgs.zip ];
      npmDepsHash = "sha256-i9fKD83wfPbETTlQ6C1lXEuIVzz/a5foV7ONnPC3ios=";
      sourceRoot = "source/firefox";
      src = lib.fileset.toSource {
        inherit root;
        fileset = lib.fileset.unions [
          # keep-sorted start
          (data + "/icons")
          (root + "/firefox")
          # keep-sorted end
        ];
      };
      # keep-sorted end
    };
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

  zib-web-clipper = clipper "production" "zib-web-clipper";

  zib-web-clipper-dev = clipper "dev" "zib-web-clipper-dev";

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
