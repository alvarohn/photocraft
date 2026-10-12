"""Resolves a file's merge conflicts when both sides only added lines (diff3 markers, empty base).

Used by merge-upstream.sh for code: two branches adding a line at the same place in a list
(a `mod` line, a match arm, a registration) conflict even though keeping both is right. Exits 1
and leaves the file alone if any conflict has a non-empty base, i.e. one side changed or removed
lines the other side also touched.
"""

import sys


def resolve(lines):
    out, state, hunks = [], None, 0
    ours, base, theirs = [], [], []
    for line in lines:
        if state is None:
            if line.startswith("<<<<<<< "):
                state, ours, base, theirs = "ours", [], [], []
                hunks += 1
            else:
                out.append(line)
        elif state == "ours":
            if line.startswith("||||||| "):
                state = "base"
            elif line.startswith("======="):
                return None  # no base section: not diff3, can't tell
            else:
                ours.append(line)
        elif state == "base":
            if line == "=======":
                state = "theirs"
            else:
                base.append(line)
        elif line.startswith(">>>>>>> "):
            if any(l.strip() for l in base):
                return None
            out.extend(ours)
            if theirs != ours:
                out.extend(theirs)
            state = None
        else:
            theirs.append(line)
    # No markers at all: a modify/delete conflict, which needs a person.
    return out if state is None and hunks else None


def main():
    path = sys.argv[1]
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    out = resolve(lines)
    if out is None:
        sys.exit(1)
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(out))


if __name__ == "__main__":
    main()
