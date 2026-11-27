#!/usr/bin/env bash
# Builds the git-workspace fixture of scripts/selftest.sh inside WSL for scripts/selftest-git.ps1 -Wsl.
#   selftest-git-wsl-fixture.sh <empty target folder>
# Keep it in step with the git-workspace branch of scripts/selftest.sh and New-GitWorkspaceFixture.
set -e
T="$1"
rm -rf "$T"
mkdir -p "$T/td/Projects"
cd "$T/td"
git init -q -b main
git config user.name Test
git config user.email test@example.com
git config core.autocrlf false
git config commit.gpgSign false
git config core.hooksPath /dev/null
: > .git/empty-excludes
git config core.excludesFile "$PWD/.git/empty-excludes"
seq 1 30 > file.txt
git add file.txt
git commit -qm 'initial file'
base=$(git rev-parse HEAD)
for n in alpha beta gamma; do
  echo "$n" > "$n.txt"
  git add "$n.txt"
  git commit -qm "add $n"
done
for s in left right; do
  git checkout -qb "conflict-$s" "$base"
  sed -i "15s/.*/$s side/" file.txt
  git commit -qam "$s conflict"
done
git checkout -q main
git clone --bare -q . "$T/remote.git"
git remote add origin "$T/remote.git"
git fetch -q origin
git branch --set-upstream-to=origin/main main > /dev/null
H="$T/submodule-host"
mkdir "$H"
git -C "$H" init -q -b main
git -C "$H" config user.name Test
git -C "$H" config user.email test@example.com
git -C "$H" config commit.gpgSign false
git -C "$H" -c protocol.file.allow=always submodule add -q "$T/remote.git" module
git -C "$H" commit -qm 'add submodule'
git -C "$H" submodule deinit -q module
M="$T/many-commits"
git init -q -b main "$M"
git -C "$M" config core.autocrlf false
for i in $(seq 1 230); do
  printf 'commit refs/heads/main\ncommitter Test <test@example.com> %d +0000\ndata 15\ncommit no. %03d\n\n' "$((1700000000 + i * 60))" "$i"
done | git -C "$M" fast-import --quiet
git -C "$M" reset -q --hard
sed -i '2s/.*/second changed/;25s/.*/twenty-five changed/' file.txt
printf 'untracked\n' > new.txt
