#!/usr/bin/env bash
# Merges the official PhotoCraft (default: upstream/main) into the current branch of this fork.
#
# Conflicts this fork expects are resolved here: Markdown docs (generated and prose) take the
# official text, Cargo.lock takes the official lockfile and is re-resolved for the fork's extra
# dependencies, translation tables keep both sides (.gitattributes: merge=union), and code where
# both sides only added lines at the same place keeps both (union-additions.py). Any other
# conflict aborts the merge, lists the files on stderr and exits 2, so a person can look. The
# workflow then builds the result, so a bad union fails the build instead of shipping.
set -euo pipefail

ref=${1:-upstream/main}
if git merge-base --is-ancestor "$ref" HEAD; then
  echo "already up to date with $ref"
  exit 0
fi
msg="Merge the official PhotoCraft ($(git rev-parse --short=12 "$ref"))"

# A line both sides added survives a union merge twice; the catalog tests reject duplicates.
# Duplicates the official file already has are left alone, so the fork doesn't diverge from it.
# A string both sides translated, differently, keeps the official translation.
dedupe_translations() {
  python3 - "$ref" crates/ui-egui/src/i18n/*.tsv <<'PY'
import collections, subprocess, sys
ref = sys.argv[1]
for path in sys.argv[2:]:
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    official = subprocess.run(["git", "show", f"{ref}:{path}"], capture_output=True, text=True, encoding="utf-8").stdout
    allowed = {l for l, n in collections.Counter(official.split("\n")).items() if n > 1}
    # context <TAB> source <TAB> translation; headers and comments stay as they are.
    entry = lambda l: not l.startswith("#") and l.count("\t") >= 2
    key = lambda l: tuple(l.split("\t")[:2])
    official_by_key = collections.defaultdict(set)
    for l in official.split("\n"):
        if entry(l):
            official_by_key[key(l)].add(l)
    present = set(lines)
    seen, out = set(), []
    for line in lines:
        if entry(line):
            theirs = official_by_key.get(key(line), set())
            if theirs and line not in theirs and theirs & present:
                continue
        # Entries without a context start with a tab.
        if line.startswith("\t") and line.count("\t") >= 2 and line not in allowed:
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

# The fork's crates take the workspace version, so an official version bump leaves their
# Cargo.lock entries behind even when the lockfile merges without a conflict (cargo --locked
# then refuses it). The official lockfile again, plus what the fork's crates need.
sync_lock() {
  if cargo metadata --locked --format-version 1 > /dev/null 2>&1; then
    return
  fi
  git show "$ref:Cargo.lock" > Cargo.lock
  cargo metadata --format-version 1 > /dev/null
  git add Cargo.lock
  git commit -q -m "Cargo.lock: the official lockfile plus the fork's crates"
  echo "re-resolved Cargo.lock"
}

here=$(cd "$(dirname "$0")" && pwd)
if git -c merge.conflictStyle=diff3 merge --no-ff --no-edit -m "$msg" "$ref"; then
  dedupe_translations
  sync_lock
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
    *.rs | *.toml)
      if python3 "$here/union-additions.py" "$f"; then git add -- "$f"; else left+=("$f"); fi
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
sync_lock
echo "merged $ref"
