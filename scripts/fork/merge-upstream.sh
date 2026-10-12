#!/usr/bin/env bash
# Merges the official PhotoCraft (default: upstream/main) into the current branch of this fork.
#
# Conflicts this fork expects are resolved here: Markdown docs (generated and prose) take the
# official text, Cargo.lock takes the official lockfile and is re-resolved for the fork's extra
# dependencies, and translation tables keep both sides (.gitattributes: merge=union). Any other
# conflict aborts the merge, lists the files on stderr and exits 2, so a person can look.
set -euo pipefail

ref=${1:-upstream/main}
if git merge-base --is-ancestor "$ref" HEAD; then
  echo "already up to date with $ref"
  exit 0
fi
msg="Merge the official PhotoCraft ($(git rev-parse --short=12 "$ref"))"
if git merge --no-ff --no-edit -m "$msg" "$ref"; then
  exit 0
fi

lock=false
left=()
while IFS= read -r f; do
  [ -n "$f" ] || continue
  case "$f" in
    *.md | Cargo.lock)
      # Deleted upstream: follow it.
      if git checkout --theirs -- "$f" 2>/dev/null; then git add -- "$f"; else git rm -q -- "$f"; fi
      [ "$f" = Cargo.lock ] && lock=true
      ;;
    *) left+=("$f") ;;
  esac
done < <(git diff --name-only --diff-filter=U)

if ((${#left[@]})); then
  echo "conflicts that need a person:" >&2
  printf '  %s\n' "${left[@]}" >&2
  git merge --abort
  exit 2
fi
if $lock; then
  # Adds what the fork's crates need to the official lockfile without upgrading anything else.
  cargo metadata --format-version 1 > /dev/null
  git add Cargo.lock
fi
git commit --no-edit -q
echo "merged $ref"
