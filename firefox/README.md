# Zib web clipper

The private Firefox extension saves web articles with their images and web PDFs to a space in Zib.

## Try a development build

### Build Zib and the add-on

Run `devenv shell` from the repository. Build Zib with `cargo build --package zib-linux`. In
`firefox/`, run `npm ci` and `npm run build` to build `dist/manifest.json` and its files.

### Register the native host

Copy `firefox/native-host.example.json` to
`~/.mozilla/native-messaging-hosts/systems.thoughtfull.zibaldone.dev.json`. Set `path` to an
executable script that runs the absolute path to `target/debug/zib` with `firefox-host` as its first
argument.

### Load and configure the add-on

Start a separate Firefox profile with `ZIB_DEV_ROOT` set to an absolute scratch directory. In
`about:debugging`, choose **This Firefox**, then **Load Temporary Add-on**, and open
`dist/manifest.json`.

Open the extension's preferences from `about:addons`. Choose a space. Select **Allow web downloads**
to save web PDFs and images hosted on other sites. Pin the button and click it on an article or PDF.

## Install for local use

- Build `nix build .#zib-web-clipper` to get the local extension package in `result/`. Use
  `nix build .#zib-web-clipper-dev` for a build with the development host and add-on identifiers.
- Register the Zib package with `programs.firefox.nativeMessagingHosts.packages` on NixOS. Use the
  Zib package for the same channel as the add-on.
- Load the package temporarily in Firefox. For a persistent install, use Firefox Developer Edition
  with `xpinstall.signatures.required` set to false in the profile you use for the clipper. In
  `about:addons`, choose **Install Add-on From File** and select the local package.

The green check stays on the clipped tab until you navigate away or close it. If an image download
fails, Zib keeps its web address as a link when available. Open the saved article in Zib and choose
**Article details** in the header to see its source metadata.
