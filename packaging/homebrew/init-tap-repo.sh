#!/usr/bin/env bash
# Materialize the Homebrew tap into a standalone git repo named
# `homebrew-piwiplay`, ready to push to github.com/vladekk/homebrew-piwiplay.
#
# Usage: packaging/homebrew/init-tap-repo.sh [DEST_DIR]
#   DEST_DIR defaults to ./homebrew-piwiplay
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
dest="${1:-./homebrew-piwiplay}"

mkdir -p "$dest/Formula"
cp "$here/Formula/piwiplay.rb" "$dest/Formula/piwiplay.rb"
cp "$here/README.md" "$dest/README.md"

cd "$dest"
if [ ! -d .git ]; then
  git init -q
fi
# Derive the version from the formula so the commit message stays accurate.
ver=$(grep -oE 'v[0-9]+\.[0-9]+\.[0-9]+' Formula/piwiplay.rb | head -1)
git add -A
git commit -q -m "piwiplay tap: formula ${ver:-update}" || echo "nothing to commit"

cat <<EOF

Tap repo ready at: $dest

Next:
  cd "$dest"
  gh repo create vladekk/homebrew-piwiplay --public --source=. --push
  # or
  git remote add origin git@github.com:vladekk/homebrew-piwiplay.git
  git push -u origin main

Then users can:
  brew tap vladekk/piwiplay
  brew install piwiplay
EOF
