# Release checklist

The procedure that cut v0.1.0, kept current so the next cut repeats it instead of rediscovering it. Artifacts are named `robco-term-<VERSION>-<platform>-<arch>` bare where a user runs them directly, and by each package format's own convention otherwise. The Windows artifact is one bare `.exe`: the settings window and a static Tcl/Tk ride inside the terminal binary, and right-click re-executes that same file with `--settings`. The macOS artifact is a `.dmg` carrying `RobCo Terminal.app` (built by `packaging/macos/bundle.sh`), the terminal and the settings binary inside the bundle, which is how a macOS user expects an app to arrive.

## Before tagging

- [ ] Version in `[workspace.package]` (root `Cargo.toml`) is the target version. Bumping it is its own decision, made ahead of cutting, not during.
- [ ] `debian/changelog` has an entry for the target version.
- [ ] All changes committed and pushed to main.
- [ ] The release commit's CI run is green: `windows` proves the Windows build, including the settings payload's own `--settings-selftest`; `macos` proves the terminal and the settings image build there; `linux` builds the tarball and the deb and reads the package's dependencies back out. Every release asset is built fresh by `release.yml` when the tag is pushed, not carried over from this run.

The tag push checks the first two: `release.yml` builds nothing when the tag, `Cargo.toml` and `debian/changelog` disagree.

## Where the Linux artifacts come from

The tag push builds them: `release.yml`'s `linux` job builds the settings image, rolls the tarball, runs `dpkg-buildpackage`, and attaches both files. Its `lintian` step suppresses the findings this project accepts, the two missing manual pages and `bad-distribution-in-changes-file`, and fails on any other.

The job is pinned to `ubuntu-24.04`: a deb installs only where glibc is at least the build host's, so the pin is the decision about who can install the release, and moving it forward drops the systems below it. `tcl9.0`, absent before 25.04, puts the practical floor there. [BUILD.md](../BUILD.md) holds the by-hand route.

## Publish

```bash
git tag v<VERSION> && git push origin refs/tags/v<VERSION>
gh release create v<VERSION> --title "RobCo Terminal <VERSION>" -F <notes file>
```

The release is created empty: the tag push above triggers `.github/workflows/release.yml`, which carries all three platforms and waits for the release to exist before uploading, so run `gh release create` promptly after the push rather than long after it. Its `windows` job builds the embedded-settings exe, proves the payload with `--settings-selftest`, and uploads `robco-term-<VERSION>-windows-x86_64.exe`. Its `macos` job builds the terminal and the self-contained settings image on `macos-latest`, runs the image's own `--selftest`, bundles both into `RobCo Terminal.app` via `packaging/macos/bundle.sh`, and uploads `robco-term-<VERSION>-macos-arm64.dmg` (Apple Silicon; an Intel or universal build is unbuilt). Its `linux` job uploads `robco-term-<VERSION>-linux-x86_64.tar.gz` and `robco-term_<VERSION>_amd64.deb`. The Windows exe and the disk image are unsigned: Windows meets SmartScreen once, and macOS refuses the first launch until the user opens it from the context menu or clears the quarantine attribute, which the notes have to say. Nothing to fetch or stamp by hand for any of them; check the workflow run finished before moving on to Verify below.

Write the notes for the stranger on the releases page: what each asset is, what it needs, and what is honestly untested. A recut (assets or notes corrected after publish) keeps the version number: `gh release upload`/`delete-asset`/`edit` against the same tag.

## Verify

Read the asset list back and check every artifact above appears:

```bash
gh release view v<VERSION> --json assets --jq '.assets[].name'
```

A release the page does not show is not released, whatever was built.

## Point the cask at it

A Mac installs this through Homebrew as well as by hand, so the tap has to
be told the release exists. In `overseers-desk/homebrew-od`, one edit to
`Casks/robco-term.rb`: the `version`, and the `sha256` of the disk image
this release published.

```bash
gh release download v<VERSION> -p '*-macos-arm64.dmg' -O - | sha256sum
```

Until that edit lands, `brew install --cask robco-term` installs the
release before this one.

## Open

Every platform's assets are built and attached by `.github/workflows/release.yml`; what remains to do by hand is the tag, the notes, and the cask. On macOS, signing and notarisation and an Intel or universal build are open. Notarisation needs one thing first: it demands a signature on every Mach-O in the bundle, and the settings image carries its script archive past the end of its own, which `codesign` will not sign at any depth. That archive has to move into `Contents/Resources` beside a plain interpreter, or the settings program has to be embedded in the terminal the way the Windows build embeds it, before a Developer ID can be applied at all.
