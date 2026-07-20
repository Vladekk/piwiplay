# piwiplay — Homebrew tap (third-party)

This directory holds the contents of a **separate** Homebrew tap repository,
`homebrew-piwiplay`. It is a third-party tap, **not** an official homebrew-core
formula.

```
homebrew-piwiplay/
├── Formula/
│   └── piwiplay.rb
└── README.md
```

## Why a separate repo

Homebrew taps must live in a repo named `homebrew-<tap>`. Users add it with
`brew tap <owner>/<tap>`, which maps to `github.com/<owner>/homebrew-<tap>`.
So the formula cannot live in the main `piwiplay` repo — it needs its own
`homebrew-piwiplay` repository.

## Creating and publishing the tap

The main repo ships `init-tap-repo.sh`, which materializes this directory into a
standalone git repo ready to push:

```sh
packaging/homebrew/init-tap-repo.sh /path/to/homebrew-piwiplay
cd /path/to/homebrew-piwiplay
gh repo create vladekk/homebrew-piwiplay --public --source=. --push
# or: git remote add origin git@github.com:vladekk/homebrew-piwiplay.git && git push -u origin main
```

## Installing via the tap

```sh
brew tap vladekk/piwiplay
brew install piwiplay          # release build
brew install --HEAD piwiplay   # latest main
```

## Updating the formula for a release

1. Tag a release in the main repo (e.g. `v0.1.0`) and create the source tarball.
2. `shasum -a 256 v0.1.0.tar.gz` and paste into `sha256` in `Formula/piwiplay.rb`.
3. Bump `url` to the new tag, commit, and push the tap repo.

## Notes

- piwiplay is **Linux-only** (PipeWire); the formula uses `depends_on :linux`.
- It depends on Homebrew's `pipewire` for headers and `rust`/`pkg-config` to
  build. At runtime it connects to your system PipeWire session.
