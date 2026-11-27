# Fedora COPR package "gall": repackages the official .rpm from GitHub Releases.
#
# The binary is built by the Release workflow (signed, with the updater enabled), so this
# spec only verifies and unpacks it. Bump with `scripts/copr-build.sh <version>`, which
# fills in `Version` and `gall_sha256` from the published SHA256SUMS.

%global gall_sha256 ed79fe60b5f81bbacf1eed0624405ba050aa000a59ceb5ff268f74b5ebe11564

# The payload is a prebuilt binary: nothing to strip and no debug sources to split off.
%global debug_package %{nil}
%global __strip /bin/true
%global _build_id_links none

Name:           gall
Version:        0.2.0
Release:        1%{?dist}
Summary:        Keyboard-first file manager with instant search and compare
License:        MIT
URL:            https://github.com/GaMa87/gall
Source0:        %{url}/releases/download/v%{version}/Gall-%{version}-1.x86_64.rpm
Source1:        https://raw.githubusercontent.com/GaMa87/gall/v%{version}/LICENSE

ExclusiveArch:  x86_64
BuildRequires:  cpio
BuildRequires:  rpm

Recommends:     git
Recommends:     gstreamer1-plugins-good

%description
Gall is a fast, keyboard-first file manager with Miller columns, two side-by-side browsers
and tabs. It finds any file on the computer as you type, searches text inside files,
compares text, spreadsheets, images and folders, and has Vim keys, previews, Git status
and undo for every file operation.

This package repackages the official build from GitHub Releases. In-app updates are for
the AppImage and installer builds; update this package with dnf.

%prep
echo '%{gall_sha256}  %{SOURCE0}' | sha256sum -c -
rm -rf payload && mkdir payload
cd payload
rpm2cpio %{SOURCE0} | cpio --quiet -idm --no-absolute-filenames

%build

%install
cp -a payload/usr %{buildroot}/usr
chmod 755 %{buildroot}%{_bindir}/gall
if [ -f "%{buildroot}%{_datadir}/applications/Gall.desktop" ] && [ ! -e "%{buildroot}%{_datadir}/applications/gall.desktop" ]; then
  mv "%{buildroot}%{_datadir}/applications/Gall.desktop" "%{buildroot}%{_datadir}/applications/gall.desktop"
fi
if [ -f "%{buildroot}%{_datadir}/applications/gall.desktop" ]; then
  sed -i 's/^Icon=.*/Icon=gall/' "%{buildroot}%{_datadir}/applications/gall.desktop"
  if grep -q '^StartupWMClass=' "%{buildroot}%{_datadir}/applications/gall.desktop"; then
    sed -i 's/^StartupWMClass=.*/StartupWMClass=gall/' "%{buildroot}%{_datadir}/applications/gall.desktop"
  else
    printf 'StartupWMClass=gall\n' >> "%{buildroot}%{_datadir}/applications/gall.desktop"
  fi
fi
install -Dpm644 %{SOURCE1} %{buildroot}%{_licensedir}/%{name}/LICENSE

: > packaged.files
if [ -f "%{buildroot}%{_datadir}/icons/hicolor/scalable/apps/gall.svg" ]; then
  echo '%{_datadir}/icons/hicolor/scalable/apps/gall.svg' >> packaged.files
fi
if [ -f "%{buildroot}%{_datadir}/dbus-1/services/org.gall.app.FileManager1.service" ]; then
  echo '%{_datadir}/dbus-1/services/org.gall.app.FileManager1.service' >> packaged.files
fi
if [ -d "%{buildroot}%{_datadir}/gall" ]; then
  echo '%dir %{_datadir}/gall' >> packaged.files
fi
if [ -d "%{buildroot}%{_datadir}/gall/actions.d" ]; then
  echo '%dir %{_datadir}/gall/actions.d' >> packaged.files
fi
if [ -f "%{buildroot}%{_datadir}/gall/actions.d/50-omarchy.json" ]; then
  echo '%{_datadir}/gall/actions.d/50-omarchy.json' >> packaged.files
fi

%files -f packaged.files
%license %{_licensedir}/%{name}/LICENSE
%{_bindir}/gall
%{_datadir}/applications/gall.desktop
%{_datadir}/icons/hicolor/32x32/apps/gall.png
%{_datadir}/icons/hicolor/128x128/apps/gall.png
%{_datadir}/icons/hicolor/256x256@2/apps/gall.png
%{_datadir}/metainfo/org.gall.app.metainfo.xml

%changelog
* Mon Nov 09 2026 Matej Gajšek <37571659+GaMa87@users.noreply.github.com> - 0.2.0-1
- Repackage Gall 0.2.0 from GitHub Releases
