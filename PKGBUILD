# Maintainer: Alexander Birkner <alex.birkner@gmail.com>
pkgname=plasma6-wallpapers-aerial
pkgver=0.2.0 # x-release-please-version
pkgrel=1
pkgdesc="Plasma 6 wallpaper plugin that plays Apple TV Aerial screensaver videos"
arch=('x86_64' 'aarch64')
url="https://github.com/BirknerAlex/plasma6-wallpapers-aerial"
license=('GPL-2.0-or-later')
depends=('plasma-workspace' 'qt6-declarative' 'qt6-multimedia' 'qt6-base')
makedepends=('extra-cmake-modules' 'cmake' 'rust' 'corrosion' 'kpackage' 'libplasma')
source=("git+${url}.git#tag=${pkgname}-v${pkgver}")
sha256sums=('SKIP')

prepare() {
    cd "$srcdir/$pkgname"
    # Cargo.lock's own aerial_core entry tracks whatever version last ran
    # `cargo build`/`cargo update` locally, which can trail Cargo.toml's
    # release-please-bumped version between releases -- fix it up before
    # --locked below, which would otherwise refuse to touch the lock file.
    cargo update --manifest-path rust/Cargo.toml -p aerial_core --precise "$pkgver"
    # Fetch and vendor Rust dependencies at package-source time, so build()
    # never touches the network -- required for a reproducible/offline AUR
    # build, and this is the only makepkg() phase network access is allowed in.
    cargo fetch --locked --manifest-path rust/Cargo.toml
}

build() {
    cd "$srcdir/$pkgname"
    export CARGO_NET_OFFLINE=true
    cmake -B build -S . \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX=/usr \
        -Wno-dev
    cmake --build build
}

package() {
    cd "$srcdir/$pkgname"
    DESTDIR="$pkgdir" cmake --install build
}
