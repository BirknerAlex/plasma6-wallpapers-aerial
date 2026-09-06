# Maintainer: Alexander Birkner <alex.birkner@gmail.com>
pkgname=plasma6-wallpapers-aerial
pkgver=0.1.0
pkgrel=1
pkgdesc="Plasma 6 wallpaper plugin that plays Apple TV Aerial screensaver videos"
arch=('x86_64' 'aarch64')
url="https://github.com/BirknerAlex/plasma6-wallpapers-aerial"
license=('GPL-2.0-or-later')
depends=('plasma6-workspace' 'qt6-declarative' 'qt6-multimedia' 'qt6-base')
makedepends=('extra-cmake-modules' 'cmake' 'rust' 'corrosion')
source=("git+${url}.git#tag=v${pkgver}")
sha256sums=('SKIP')

prepare() {
    cd "$srcdir/$pkgname"
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
