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

# A line both sides added survives a union merge twice; the catalog tests reject duplicates.
dedupe_translations() {
  python3 - crates/ui-egui/src/i18n/*.tsv <<'PY'
import sys
for path in sys.argv[1:]:
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    seen, out = set(), []
    for line in lines:
        # Entries without a context start with a tab; headers and comments stay as they are.
        if line.startswith("\t") and line.count("\t") >= 2:
            if line in seen:
                continue
            seen.add(line)
        out.append(line)
    if out != lines:
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(out))
        print(f"removed duplicate lines from {path}")
PY
  if ! git diff --quiet -- crates/ui-egui/src/i18n; then
    git add crates/ui-egui/src/i18n
    git commit -q -m "Drop translation lines both sides added"
  fi
}

if git merge --no-ff --no-edit -m "$msg" "$ref"; then
  dedupe_translations
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
dedupe_translations
echo "merged $ref"
