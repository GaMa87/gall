#!/usr/bin/env bash
# Builds the source RPM for Fedora COPR from the published GitHub release and, with
# --submit, uploads it to the COPR project.
#
#   scripts/copr-build.sh [<version>] [--submit] [--rebuild]
#
# Without <version> the version in packaging/copr/gall.spec is used. With a different one the
# spec is updated: Version, gall_sha256 (from the release's SHA256SUMS) and a changelog entry.
# --rebuild runs `rpmbuild --rebuild` locally on the SRPM (needs cpio and rpm2cpio).
# --submit needs `copr-cli` and an API token in ~/.config/copr (https://copr.fedorainfracloud.org/api/).
set -euo pipefail

repo=GaMa87/gall
copr_project=${COPR_PROJECT:-gama87/gall}
root=$(cd "$(dirname "$0")/.." && pwd)
spec=$root/packaging/copr/gall.spec
version=""
submit=0
rebuild=0
for arg in "$@"; do
  case $arg in
    --submit) submit=1 ;;
    --rebuild) rebuild=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) version=${arg#v} ;;
  esac
done
current=$(awk '/^Version:/ {print $2}' "$spec")
version=${version:-$current}

work=$(mktemp -d "${TMPDIR:-/tmp}/gall-copr.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir "$work"/SOURCES "$work"/SRPMS "$work"/BUILD "$work"/RPMS "$work"/SPECS

rpm_name=Gall-$version-1.x86_64.rpm
base=https://github.com/$repo/releases/download/v$version
curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS"
curl -fsSL -o "$work/SOURCES/$rpm_name" "$base/$rpm_name"
curl -fsSL -o "$work/SOURCES/LICENSE" "https://raw.githubusercontent.com/$repo/v$version/LICENSE"

sum=$(awk -v f="$rpm_name" '$2 == f {print $1}' "$work/SHA256SUMS")
[ -n "$sum" ] || { echo "$rpm_name is not listed in SHA256SUMS of v$version" >&2; exit 1; }
echo "$sum  $work/SOURCES/$rpm_name" | sha256sum -c -

cp "$spec" "$work/SPECS/gall.spec"
sed -i -e "s/^%global gall_sha256 .*/%global gall_sha256 $sum/" \
       -e "s/^Version:.*/Version:        $version/" "$work/SPECS/gall.spec"
if [ "$version" != "$current" ]; then
  entry="* $(LC_ALL=C date '+%a %b %d %Y') Matej Gajšek <37571659+GaMa87@users.noreply.github.com> - $version-1
- Repackage Gall $version from GitHub Releases
"
  # New entry directly below %changelog.
  awk -v e="$entry" '{print} /^%changelog/ {print e}' "$work/SPECS/gall.spec" > "$work/spec.new"
  mv "$work/spec.new" "$work/SPECS/gall.spec"
  cp "$work/SPECS/gall.spec" "$spec"
  echo "Updated $spec to $version"
fi

# The sources are already in SOURCES, so the SRPM is self-contained and COPR builds it
# without internet access.
rpmbuild -bs --define "_topdir $work" "$work/SPECS/gall.spec"
mkdir -p "$root/target/copr"
cp "$work"/SRPMS/gall-*.src.rpm "$root/target/copr/"
srpm=$(ls "$root"/target/copr/gall-"$version"-*.src.rpm)
echo "SRPM: $srpm"

if [ "$rebuild" = 1 ]; then
  rpmbuild --rebuild --define "_topdir $work" "$srpm"
  ls -l "$work"/RPMS/*/
  cp "$work"/RPMS/*/gall-*.rpm "$root/target/copr/"
fi
if [ "$submit" = 1 ]; then
  copr-cli build "$copr_project" "$srpm"
fi
