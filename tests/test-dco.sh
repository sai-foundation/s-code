#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$ROOT/.work"
task="$(mktemp -d "$ROOT/.work/dco.XXXXXX")"
trap 'find "$task" -depth -delete' EXIT HUP INT TERM

repository="$task/repository"
git -C "$task" init --initial-branch=main repository >/dev/null
printf 'base\n' >"$repository/file.txt"
git -C "$repository" add file.txt
git -C "$repository" -c user.name='Base Author' -c user.email='base@example.invalid' \
  commit -s -m 'base' >/dev/null
base="$(git -C "$repository" rev-parse HEAD)"

printf 'valid\n' >>"$repository/file.txt"
git -C "$repository" add file.txt
git -C "$repository" -c user.name='Alice Example' -c user.email='alice@example.invalid' \
  commit -s -m 'valid sign-off' >/dev/null
valid="$(git -C "$repository" rev-parse HEAD)"
(cd "$repository" && python3 "$ROOT/scripts/check-dco.py" --base "$base" --head "$valid")

printf 'spoofed\n' >>"$repository/file.txt"
git -C "$repository" add file.txt
git -C "$repository" -c user.name='Alice Example' -c user.email='alice@example.invalid' \
  commit -m 'spoofed sign-off

Signed-off-by: Mallory Example <mallory@example.invalid>' >/dev/null
spoofed="$(git -C "$repository" rev-parse HEAD)"
if (cd "$repository" && python3 "$ROOT/scripts/check-dco.py" \
  --base "$valid" --head "$spoofed" >"$task/spoofed.out" 2>"$task/spoofed.err"); then
  echo 'DCO accepted a different signer identity' >&2
  exit 1
fi
grep -F 'author-matching Signed-off-by' "$task/spoofed.err" >/dev/null

printf 'fake bot\n' >>"$repository/file.txt"
git -C "$repository" add file.txt
git -C "$repository" -c user.name='Evil [bot]' -c user.email='evil@example.invalid' \
  commit -m 'fake bot bypass' >/dev/null
fake_bot="$(git -C "$repository" rev-parse HEAD)"
if (cd "$repository" && python3 "$ROOT/scripts/check-dco.py" \
  --base "$spoofed" --head "$fake_bot" >"$task/bot.out" 2>"$task/bot.err"); then
  echo 'DCO accepted an untrusted bot identity' >&2
  exit 1
fi
grep -F 'author-matching Signed-off-by' "$task/bot.err" >/dev/null

printf 'DCO identity binding passed\n'
