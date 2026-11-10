# Zib web clipper

The private Firefox extension saves web articles with their images, web PDFs, and YouTube videos to
a space in Zib.

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
to save web PDFs and images hosted on other sites. Pin the button and click it on a page to save.

On a YouTube video, the clipper tries to retrieve the captions and saves them as a timestamped
transcript in a `video` note. The badge reports when a video saves without a transcript.

## Build the production add-on from source

The release workflow runs on Ubuntu 24.04 with [Node.js 24.14.0][node]. From the source archive's
root, run:

```sh
cd firefox
npm ci
ZIB_CLIPPER_CHANNEL=production npm run build
```

The resulting extension is in `firefox/dist` relative to the archive root. It targets Firefox 140
and later. When you click the toolbar button, the add-on sends the selected page's address and
captured content to the local Zib native host. Firefox stores your destination space choice.

## Publish a private signed build

Create [Mozilla Add-ons signing credentials][amo-credentials] and add the issuer and secret to the
private GitHub repository's Actions secrets as `AMO_JWT_ISSUER` and `AMO_JWT_SECRET`.

Push a tag that matches the version in `firefox/package.json`, such as `clipper-v0.1.0`. The
workflow builds the production add-on and submits it and its source to Mozilla for
[unlisted signing][signing]. After approval, the workflow attaches `zib-web-clipper.xpi` to the
GitHub Release for that tag. Get the signed package with authenticated repository access:

```sh
gh release download clipper-v0.1.0 \
  --repo thoughtfull-systems/Zibaldone \
  --pattern zib-web-clipper.xpi
```

Use a new package version and tag for each update. If Mozilla approves the submission after the
workflow times out, download the signed package from [My Add-ons][amo-addons] and attach it to the
release for the original tag.

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
**Note metadata** in the header to see its source metadata.

[amo-addons]: https://addons.mozilla.org/developers/addons
[amo-credentials]: https://addons.mozilla.org/developers/addon/api/key/
[node]: https://nodejs.org/en/download
[signing]: https://extensionworkshop.com/documentation/publish/signing-and-distribution-overview/
